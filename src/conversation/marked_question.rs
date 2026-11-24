//! Ordinary interpretation evidence. A receipt carries no live erase authority.
use super::{
    reader_plan::checked_region, target, Acknowledgment, AttemptReason, ContextBudget,
    ContextPurpose, ContextView, ExpectedHeads, Ledger, Mode, Outcome, Receipt, Record, Role,
    SourceObservation, Turn,
};
use crate::storage::{
    captured::{Captured, ReadBudget, RecordRef},
    digest,
    sync::mode::OrdinaryModeLease,
    Envelope, Media, Namespace, ObjectRef, Uuid, MAX_METADATA, MAX_RECORD,
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
    ordinary: OrdinaryModeLease,
}

pub(crate) struct FreshMarkedQuestion {
    pub(super) current: Captured,
    pub(super) operation: Uuid,
    pub(super) root_revision: Uuid,
    pub(super) mark: Box<MarkedQuestion>,
    pub(super) ordinary: OrdinaryModeLease,
}

pub(crate) struct MarkedAnswer {
    mark: FreshMarkedQuestion,
    pinned: PinnedAnswer,
}

struct PinnedAnswer {
    draft: Turn,
    revisions: PairRevisions,
    target: crate::workflow::exchange_history::UserPresentationTarget,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PairRevisions {
    pub(crate) root: Uuid,
    pub(crate) binding: Uuid,
    pub(crate) user: Uuid,
    pub(crate) draft: Uuid,
}

pub(crate) struct MarkedAnswerOutput {
    pinned: PinnedAnswer,
    current: Captured,
    _ordinary: OrdinaryModeLease,
    pair_attempted: bool,
}

pub(crate) struct CompletedMarkedAnswer {
    revision: Uuid,
    current: Captured,
    _ordinary: OrdinaryModeLease,
}

impl CompletedMarkedAnswer {
    pub(crate) fn bind_metadata(self, ledger: &Ledger, bind: impl FnOnce(Uuid) -> bool) -> bool {
        self.current
            .with_current(ledger.store(), || Ok(bind(self.revision)))
            .unwrap_or(false)
    }
}

impl FreshMarkedQuestion {
    pub(crate) fn presentation_head(&self) -> (Uuid, Uuid) {
        (self.root_revision, self.mark.interpreted_turn.revision)
    }

    pub(crate) fn erase(
        self,
        ledger: &Ledger,
        device: &mut dyn crate::device::backend::DeviceBackend,
        binding: Option<&crate::device::backend::MarkedSourceBinding>,
    ) -> Result<EraseDisposition> {
        self.submit_erase(ledger, device, binding)
    }

    fn submit_erase(
        &self,
        ledger: &Ledger,
        device: &mut dyn crate::device::backend::DeviceBackend,
        binding: Option<&crate::device::backend::MarkedSourceBinding>,
    ) -> Result<EraseDisposition> {
        use crate::device::backend::MarkedEraseSubmission;
        let operation = self.operation;
        let mark = &self.mark;
        let effect = binding
            .filter(|binding| {
                binding.source == mark.source
                    && binding.overview_sha256 == mark.overview.sha256
                    && binding.dimensions == [768, 1024]
            })
            .and_then(|binding| {
                device.prepare_marked_question_erase(binding, operation, mark.region)
            });
        // The mode lease's Drop also locks Store. Keep it outside the callback.
        let submitted = self.current.with_current(ledger.store(), || {
            Ok(effect.map_or(MarkedEraseSubmission::Refused, |effect| effect.submit()))
        });
        Ok(match submitted? {
            MarkedEraseSubmission::Refused => EraseDisposition::Refused,
            MarkedEraseSubmission::Accepted {
                operation: returned,
                readback,
            } if returned == operation => {
                if readback() {
                    EraseDisposition::Verified
                } else {
                    EraseDisposition::Uncertain
                }
            }
            MarkedEraseSubmission::Accepted { .. } | MarkedEraseSubmission::Uncertain => {
                EraseDisposition::Uncertain
            }
        })
    }

    pub(crate) fn publish_answer_draft(
        mut self,
        ledger: &Ledger,
        draft: Turn,
        target: crate::workflow::exchange_history::UserPresentationTarget,
        binding_revision: Uuid,
        with_about: bool,
    ) -> Result<(super::WriteResult, MarkedAnswer)> {
        let user_id = self.mark.interpreted_turn.record;
        let user = captured_turn(&self.current, user_id)?.0;
        ensure!(
            draft.role == Role::Assistant
                && draft.mode == Mode::Reader
                && draft.outcome == Outcome::Generated
                && draft.conversation == user.conversation
                && draft.conversation == target.conversation
                && draft.exchange == user.exchange
                && draft.sources == user.sources,
            "marked answer draft differs from original User"
        );
        let operation = crate::writer::stage(user.exchange, "reader-marked-answer-draft");
        let about_user = with_about.then_some(user.id);
        let fingerprint = Ledger::fingerprint(&("reader-marked-answer-draft", &draft, about_user))?;
        let id = draft.id;
        let write = ledger.append_with_capture(
            operation,
            heads(self.root_revision),
            draft,
            vec![],
            fingerprint,
            about_user,
            Some(&mut self.current),
            true,
        )?;
        self.current.with_current(ledger.store(), || Ok(()))?;
        self.root_revision = captured_root(&self.current, &write)?;
        let (draft, draft_revision) = captured_turn(&self.current, id)?;
        ensure!(
            write.historical.records.contains(&draft.id) && draft.outcome == Outcome::Generated,
            "marked answer draft acknowledgment differs"
        );
        let pinned = PinnedAnswer {
            draft,
            revisions: PairRevisions {
                draft: draft_revision,
                user: self.mark.interpreted_turn.revision,
                root: self.root_revision,
                binding: binding_revision,
            },
            target,
        };
        Ok((write, MarkedAnswer { mark: self, pinned }))
    }
}

fn heads(revision: Uuid) -> ExpectedHeads {
    ExpectedHeads([revision].into_iter().collect())
}

fn captured_turn(current: &Captured, id: Uuid) -> Result<(Turn, Uuid)> {
    let (envelope, reference) = current
        .records
        .iter()
        .find(|(_, reference)| {
            reference.namespace == Namespace::Conversation && reference.record == id
        })
        .context("captured answer turn absent")?;
    let Record::Turn(turn) = Ledger::decode(envelope)? else {
        anyhow::bail!("captured answer turn changed");
    };
    Ok((turn, reference.revision))
}

fn captured_root(current: &Captured, write: &super::WriteResult) -> Result<Uuid> {
    let revision = write.historical.applied_root_revision;
    ensure!(
        current.records.iter().any(|(_, reference)| {
            reference.namespace == Namespace::Conversation
                && reference.record == write.historical.conversation
                && reference.revision == revision
        }),
        "marked answer applied root differs"
    );
    Ok(revision)
}

impl MarkedAnswer {
    pub(crate) fn draft(&self) -> &Turn {
        &self.pinned.draft
    }

    pub(crate) fn target(&self) -> crate::workflow::exchange_history::UserPresentationTarget {
        self.pinned.target
    }

    pub(crate) fn refuse_output(self, ledger: &Ledger) -> Result<()> {
        self.into_output().record_uncompleted(
            ledger,
            Outcome::Failed,
            AttemptReason::NativeOutputUnavailable,
        )
    }

    pub(crate) fn erase(
        self,
        ledger: &Ledger,
        device: &mut dyn crate::device::backend::DeviceBackend,
        binding: Option<&crate::device::backend::MarkedSourceBinding>,
    ) -> Result<(EraseDisposition, Option<MarkedAnswerOutput>)> {
        let disposition = self.mark.submit_erase(ledger, device, binding)?;
        let output = (disposition != EraseDisposition::Uncertain).then(|| self.into_output());
        Ok((disposition, output))
    }

    fn into_output(self) -> MarkedAnswerOutput {
        MarkedAnswerOutput {
            pinned: self.pinned,
            current: self.mark.current,
            _ordinary: self.mark.ordinary,
            pair_attempted: false,
        }
    }
}

impl MarkedAnswerOutput {
    pub(crate) fn target(&self) -> crate::workflow::exchange_history::UserPresentationTarget {
        self.pinned.target
    }

    pub(crate) fn revisions(&self) -> PairRevisions {
        self.pinned.revisions
    }

    pub(crate) fn draft(&self) -> &Turn {
        &self.pinned.draft
    }

    fn with_current<T>(&self, ledger: &Ledger, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.current.with_current(ledger.store(), f)
    }

    pub(crate) fn accept_pair(
        &mut self,
        ledger: &Ledger,
        apply: impl FnOnce() -> crate::workflow::exchange_history::Effect,
    ) -> Result<crate::workflow::exchange_history::Effect> {
        ensure!(
            !std::mem::replace(&mut self.pair_attempted, true),
            "marked answer pair already attempted"
        );
        self.with_current(ledger, || Ok(apply()))
    }

    pub(crate) fn presentation_context(
        &self,
        ledger: &Ledger,
        budget: &ContextBudget,
        count: impl Fn(&[Turn]) -> Result<usize>,
    ) -> Result<ContextView> {
        self.with_current(ledger, || Ok(()))?;
        ledger.presentation_context_for(
            self.pinned.target.conversation,
            budget,
            count,
            ContextPurpose::Pair(self.draft()),
        )
    }

    pub(crate) fn complete_answer(
        self,
        ledger: &Ledger,
        receipt: crate::workflow::exchange_history::VerifiedPairReceipt,
    ) -> Result<CompletedMarkedAnswer> {
        ensure!(self.pair_attempted, "marked answer has no pair acceptance");
        let evidence = receipt.consume(self.pinned.target, self.revisions(), self.draft())?;
        let operation =
            crate::writer::stage(evidence.native_operation, "reader-marked-answer-complete");
        let mut turn = self.pinned.draft;
        turn.outcome = Outcome::Completed;
        turn.completion = Some(evidence);
        let id = turn.id;
        let mut published = Vec::new();
        let (write, _) = ledger.advance_with_capture(
            operation,
            heads(self.pinned.revisions.root),
            heads(self.pinned.revisions.draft),
            turn,
            None,
            None,
            None,
            Some(&self.current),
            None,
            Some(&mut published),
        )?;
        let mut current = self.current;
        current.advance(published)?;
        current.with_current(ledger.store(), || Ok(()))?;
        captured_root(&current, &write)?;
        let (completed, revision) = captured_turn(&current, id)?;
        ensure!(
            completed.outcome == Outcome::Completed && write.historical.records.contains(&id),
            "marked answer completion differs"
        );
        Ok(CompletedMarkedAnswer {
            revision,
            current,
            _ordinary: self._ordinary,
        })
    }

    pub(crate) fn record_uncompleted(
        self,
        ledger: &Ledger,
        outcome: Outcome,
        reason: AttemptReason,
    ) -> Result<()> {
        ensure!(
            matches!(outcome, Outcome::Failed | Outcome::ReconcileRequired),
            "marked answer outcome must remain uncompleted"
        );
        let operation = crate::writer::stage(self.draft().exchange, "reader-marked-answer-outcome");
        let mut turn = self.pinned.draft;
        turn.outcome = outcome;
        ledger.advance_with_capture(
            operation,
            heads(self.pinned.revisions.root),
            heads(self.pinned.revisions.draft),
            turn,
            Some(reason),
            None,
            None,
            Some(&self.current),
            None,
            None,
        )?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EraseDisposition {
    Refused,
    Verified,
    Uncertain,
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
    pub(super) fn bind(
        self,
        interpreted: &Envelope,
    ) -> Result<(MarkedQuestion, OrdinaryModeLease)> {
        Ok((
            MarkedQuestion {
                interpreted_turn: reference(interpreted)?,
                capture: self.capture,
                source: self.source,
                overview: self.overview,
                region: self.region,
            },
            self.ordinary,
        ))
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
    ) -> Result<(super::WriteResult, FreshMarkedQuestion)> {
        self.complete_store()?;
        ensure!(!operation.is_nil(), "marked question operation absent");
        let mode = self.store.sync_mode_snapshot()?;
        let ordinary = self.store.ordinary_mode_lease(&mode.token)?;
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
            ordinary,
        };
        // Keep the prepared turn's exact capture relation and CAS its revision.
        let expected_turn = ExpectedHeads([original.revision_id].into_iter().collect());
        let (result, fresh) = self.advance_with_capture(
            operation,
            expected_root,
            expected_turn,
            turn,
            None,
            target,
            None,
            Some(captured),
            Some(mark),
            None,
        )?;
        Ok((
            result,
            fresh.context("fresh marked question continuation absent")?,
        ))
    }

    pub(super) fn validate_marked_question(
        &self,
        envelope: &Envelope,
        conversation: Uuid,
        budget: &mut ReadBudget,
    ) -> Result<Box<MarkedQuestion>> {
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
        checked_overview(self, &overview)?;
        Ok(mark)
    }
}
