//! Ordinary interpretation evidence. A receipt carries no live erase authority.
use super::{
    reader_plan::checked_region, target, Acknowledgment, ExpectedHeads, Ledger, Mode, Outcome,
    Receipt, Record, Role, SourceObservation, Turn,
};
use crate::storage::{
    captured::{Captured, ReadBudget, RecordRef},
    digest, Envelope, Media, Namespace, ObjectRef, Uuid, MAX_METADATA, MAX_RECORD,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 5;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkedQuestion {
    interpreted_turn: RecordRef,
    capture: RecordRef,
    source: SourceObservation,
    overview: Media,
    region: [i32; 4],
}

pub(super) struct PreparedMark {
    capture: RecordRef,
    source: SourceObservation,
    overview: Media,
    region: [i32; 4],
}

fn reference(envelope: &Envelope) -> Result<RecordRef> {
    let bytes = serde_json::to_vec(envelope)?;
    Ok(RecordRef {
        namespace: envelope.namespace,
        record: envelope.record_id,
        revision: envelope.revision_id,
        object: ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        },
    })
}

fn valid_reference(reference: &RecordRef, namespace: Namespace) -> bool {
    reference.namespace == namespace
        && !reference.record.is_nil()
        && !reference.revision.is_nil()
        && crate::storage::valid_digest(&reference.object.sha256)
        && (1..=MAX_RECORD as u64).contains(&reference.object.bytes)
}

impl PreparedMark {
    pub(super) fn bind(self, interpreted: &Envelope) -> Result<MarkedQuestion> {
        Ok(MarkedQuestion {
            interpreted_turn: reference(interpreted)?,
            capture: self.capture,
            source: self.source,
            overview: self.overview,
            region: self.region,
        })
    }
}

impl MarkedQuestion {
    pub(super) fn fingerprint(&self, acknowledgment: &Acknowledgment) -> Result<String> {
        Ledger::fingerprint(&("interpret-marked-question-v1", acknowledgment, self))
    }
}

pub(super) fn validate_receipt_shape(envelope: &Envelope, receipt: &Receipt) -> Result<()> {
    let fields = &envelope.payload["record"];
    if envelope.domain_schema_version != SCHEMA {
        ensure!(
            fields.get("marked_question").is_none(),
            "marked question needs schema 5"
        );
        return super::intent::validate_receipt_schema(envelope.domain_schema_version, receipt);
    }
    ensure!(
        fields.get("admitted_intent").is_none(),
        "selected intent and marked question are exclusive"
    );
    let mark = receipt
        .marked_question
        .as_ref()
        .context("marked question association absent")?;
    let acknowledgment = &receipt.acknowledgment;
    ensure!(
        valid_reference(&mark.interpreted_turn, Namespace::Conversation)
            && valid_reference(&mark.capture, Namespace::Source)
            && acknowledgment
                .records
                .iter()
                .filter(|id| **id == mark.interpreted_turn.record)
                .count()
                == 1
            && receipt.fingerprint == mark.fingerprint(acknowledgment)?,
        "marked question receipt references differ"
    );
    Ledger::validate_observation(&mark.source)?;
    checked_region(mark.region)?;
    mark.overview.validate()?;
    ensure!(
        (1..=MAX_METADATA as u64).contains(&mark.overview.bytes)
            && matches!(
                mark.overview.media_type.as_str(),
                "image/png" | "image/jpeg"
            ),
        "invalid marked question overview"
    );
    Ok(())
}

pub(crate) fn modeled_source(
    capture: &super::capture_facts::HistoricalCapture,
) -> SourceObservation {
    let source = &capture.source;
    SourceObservation {
        document: source.document.value(),
        page: source.page.value(),
        session: source.session.value().to_string(),
        visit: source.visit.value().to_string(),
        content_revision: Some(source.revision.value().to_string()),
        order_revision: None,
        capability_revision: capture.procedure_revision.clone(),
        evidence_procedure: capture.conversion_procedure.clone(),
    }
}

fn capture_overview(capture: Record, user: &Turn, source: &SourceObservation) -> Result<Media> {
    let Record::Capture(capture) = capture else {
        anyhow::bail!("capture cannot establish marked question source identity");
    };
    ensure!(
        user.id == capture.turn
            && user.conversation == capture.conversation
            && user.sources.contains(&capture.id),
        "marked question capture link differs"
    );
    ensure!(
        modeled_source(&capture.facts) == *source,
        "marked question capture source differs"
    );
    let overview = capture
        .facts
        .images
        .first()
        .context("captured overview absent")?;
    ensure!(
        overview.role == super::capture_facts::Role::Overview
            && overview.dimensions == [768, 1024]
            && overview.full_frame_of(&capture.facts.native_parent)
    );
    Ok(overview.media())
}

fn checked_overview(ledger: &Ledger, overview: &Media) -> Result<()> {
    ensure!(
        (1..=MAX_METADATA as u64).contains(&overview.bytes),
        "marked question overview exceeds read bound"
    );
    let bytes = ledger.store.read_object(&ObjectRef {
        sha256: overview.sha256.clone(),
        bytes: overview.bytes,
    })?;
    Ledger::validate_image_bytes(overview, &bytes, 768, 1024)
}

impl Ledger {
    pub(crate) fn capture_marked_question(&self, conversation: Uuid) -> Result<Captured> {
        self.complete_store()?;
        self.store.capture(conversation, super::evidence::matches)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn interpret_marked_question(
        &self,
        operation: Uuid,
        turn: Turn,
        expected_root: ExpectedHeads,
        captured: &Captured,
        capture: Uuid,
        source: &SourceObservation,
        region: [i32; 4],
        target: Option<(&super::target_source::SourceFrame, &target::Candidate)>,
    ) -> Result<super::WriteResult> {
        self.complete_store()?;
        ensure!(!operation.is_nil(), "marked question operation absent");
        let mode = self.store.sync_mode_snapshot()?;
        let _ordinary = self.store.ordinary_mode_lease(&mode.token)?;
        checked_region(region)?;
        Self::validate_observation(source)?;
        ensure!(
            turn.role == Role::User
                && turn.mode == Mode::Reader
                && turn.outcome == Outcome::Interpreted
                && turn
                    .text
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty())
                && turn.sources.contains(&capture),
            "invalid marked question interpretation"
        );
        captured.with_current(&self.store, || Ok(()))?;
        let mut records = std::collections::BTreeSet::new();
        ensure!(
            captured
                .records
                .iter()
                .all(|(e, _)| records.insert((e.namespace, e.record_id))),
            "marked question source history is conflicted"
        );
        let (original, _) = captured
            .records
            .iter()
            .find(|(envelope, _)| {
                envelope.namespace == Namespace::Conversation && envelope.record_id == turn.id
            })
            .context("marked question prepared turn absent")?;
        ensure!(
            matches!(Self::decode(original)?, Record::Turn(original) if original.outcome == Outcome::Prepared),
            "marked question requires an original Prepared turn"
        );
        let (evidence, reference) = captured
            .records
            .iter()
            .find(|(envelope, _)| {
                envelope.namespace == Namespace::Source && envelope.record_id == capture
            })
            .context("marked question capture absent")?;
        let overview = capture_overview(Self::decode(evidence)?, &turn, source)?;
        ensure!(
            evidence.media_descriptors.contains(&overview),
            "marked question overview absent from capture media"
        );
        checked_overview(self, &overview)?;
        let mark = PreparedMark {
            capture: reference.clone(),
            source: source.clone(),
            overview,
            region,
        };
        // Keep the prepared turn's exact capture relation and CAS its revision.
        let expected_turn = ExpectedHeads([original.revision_id].into_iter().collect());
        self.advance_with_capture(
            operation,
            expected_root,
            expected_turn,
            turn,
            None,
            target,
            Some(captured),
            Some(mark),
        )
    }

    pub(super) fn validate_marked_question(
        &self,
        envelope: &Envelope,
        conversation: Uuid,
        budget: &mut ReadBudget,
    ) -> Result<()> {
        let Record::Receipt(receipt) = Self::decode(envelope)? else {
            anyhow::bail!("marked question receipt expected");
        };
        let mark = receipt
            .marked_question
            .context("marked question association absent")?;
        let acknowledgment = receipt.acknowledgment;
        ensure!(
            acknowledgment.conversation == conversation,
            "foreign marked question"
        );
        let interpreted = self.store.resolve_record(&mark.interpreted_turn, budget)?;
        let Record::Turn(user) = Self::decode(&interpreted)? else {
            anyhow::bail!("marked question user expected");
        };
        ensure!(
            user.conversation == conversation
                && user.role == Role::User
                && user.mode == Mode::Reader
                && user.outcome == Outcome::Interpreted
                && user
                    .text
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty()),
            "invalid marked question user"
        );
        let members = self
            .store
            .original_transaction(envelope.revision_id, 6, budget)?;
        ensure!(
            members.contains(envelope),
            "original marked question receipt differs"
        );
        let mut records = Vec::new();
        let mut roots = 0;
        for member in &members {
            if member == envelope {
                continue;
            }
            if member.record_id == conversation && member.namespace == Namespace::Conversation {
                ensure!(
                    member.revision_id == acknowledgment.applied_root_revision
                        && matches!(Self::decode(member)?, Record::Root(root) if root.binding == acknowledgment.binding),
                    "marked question applied root differs"
                );
                roots += 1;
            } else {
                records.push(member.record_id);
            }
        }
        records.sort_unstable();
        let mut acknowledged = acknowledgment.records;
        acknowledged.sort_unstable();
        ensure!(
            roots == 1 && records == acknowledged && members.contains(&interpreted),
            "marked question transaction membership differs"
        );
        let capture = self.store.resolve_record(&mark.capture, budget)?;
        ensure!(
            capture.media_descriptors.contains(&mark.overview),
            "marked question overview absent from capture media"
        );
        let overview = capture_overview(Self::decode(&capture)?, &user, &mark.source)?;
        ensure!(
            overview == mark.overview,
            "marked question overview differs"
        );
        budget.charge(overview.bytes)?;
        checked_overview(self, &overview)
    }
}
