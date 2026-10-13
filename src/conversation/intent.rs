//! Persisted admission evidence. These values never constitute a live capability.
use super::{Acknowledgment, OutcomeFact, Receipt, SourceObservation};
use crate::storage::{valid_digest, Media, Namespace, ObjectRef, Uuid, MAX_ITEMS, MAX_RECORD};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const INTENT_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntentRecordRef {
    pub namespace: Namespace,
    pub record_id: Uuid,
    pub revision_id: Uuid,
    pub object: ObjectRef,
}
impl IntentRecordRef {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.record_id.is_nil() && !self.revision_id.is_nil(),
            "nil intent reference"
        );
        ensure!(
            valid_digest(&self.object.sha256)
                && self.object.bytes > 0
                && self.object.bytes <= MAX_RECORD as u64,
            "invalid intent object reference"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntentSelectionEvidence {
    pub group: Uuid,
    pub key_sha256: String,
    pub binding_sha256: String,
    pub store_generation: Uuid,
    pub aggregate_generation: Uuid,
    pub accepted_base_sha256: String,
    pub selection_sha256: String,
}
impl IntentSelectionEvidence {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.group.is_nil()
                && !self.store_generation.is_nil()
                && !self.aggregate_generation.is_nil(),
            "nil selection identity"
        );
        ensure!(
            [
                &self.key_sha256,
                &self.binding_sha256,
                &self.accepted_base_sha256,
                &self.selection_sha256
            ]
            .into_iter()
            .all(|s| valid_digest(s)),
            "invalid selection digest"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdmittedIntent {
    pub operation: Uuid,
    pub request_fingerprint: String,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub root_revision: IntentRecordRef,
    pub turn_revision: IntentRecordRef,
    pub selection: IntentSelectionEvidence,
    pub source: SourceObservation,
    pub evidence: Vec<IntentRecordRef>,
    pub media: Vec<Media>,
}
impl AdmittedIntent {
    pub(super) fn validate(&self, acknowledgment: &Acknowledgment) -> Result<()> {
        ensure!(
            !self.operation.is_nil()
                && !self.conversation.is_nil()
                && !self.turn.is_nil()
                && valid_digest(&self.request_fingerprint),
            "invalid admitted intent identity"
        );
        self.root_revision.validate()?;
        self.turn_revision.validate()?;
        self.selection.validate()?;
        super::Ledger::validate_observation(&self.source)?;
        ensure!(
            self.root_revision.namespace == Namespace::Conversation
                && self.root_revision.record_id == self.conversation
                && self.turn_revision.namespace == Namespace::Conversation
                && self.turn_revision.record_id == self.turn,
            "intent revision identity mismatch"
        );
        ensure!(
            acknowledgment.operation == self.operation
                && acknowledgment.conversation == self.conversation
                && acknowledgment.applied_root_revision == self.root_revision.revision_id
                && acknowledgment.records.contains(&self.turn),
            "intent acknowledgment mismatch"
        );
        validate_references(&self.evidence, &self.media)?;
        ensure!(!self.evidence.is_empty(), "missing intent source evidence");
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IntentSettlementState {
    Unknown,
    VerifiedSubmitted,
    VerifiedNoEffect,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntentSettlement {
    pub operation: Uuid,
    pub request_fingerprint: String,
    pub original_receipt: IntentRecordRef,
    pub original_root: IntentRecordRef,
    pub original_turn: IntentRecordRef,
    pub original_selection: IntentSelectionEvidence,
    pub state: IntentSettlementState,
    pub procedure: String,
    pub origin: String,
    pub evidence: Vec<IntentRecordRef>,
    pub media: Vec<Media>,
}
impl IntentSettlement {
    pub(super) fn validate(&self, fact: &OutcomeFact) -> Result<()> {
        ensure!(
            !self.operation.is_nil() && valid_digest(&self.request_fingerprint),
            "invalid settlement request"
        );
        for reference in [
            &self.original_receipt,
            &self.original_root,
            &self.original_turn,
        ] {
            reference.validate()?;
            ensure!(
                reference.namespace == Namespace::Conversation,
                "foreign settlement namespace"
            );
        }
        ensure!(
            self.original_receipt.record_id == super::Ledger::receipt_id(self.operation)
                && self.original_root.record_id == fact.conversation
                && self.original_turn.record_id == fact.turn,
            "settlement original intent mismatch"
        );
        self.original_selection.validate()?;
        ensure!(
            !self.procedure.trim().is_empty()
                && self.procedure.len() <= 1024
                && !self.origin.trim().is_empty()
                && self.origin.len() <= 128,
            "missing settlement evidence origin"
        );
        validate_references(&self.evidence, &self.media)?;
        ensure!(
            self.state == IntentSettlementState::Unknown || !self.evidence.is_empty(),
            "verified settlement missing evidence"
        );
        Ok(())
    }
}
fn validate_references(references: &[IntentRecordRef], media: &[Media]) -> Result<()> {
    ensure!(
        references.len() <= MAX_ITEMS && media.len() <= MAX_ITEMS,
        "intent evidence exceeds bound"
    );
    let mut seen = BTreeSet::new();
    for reference in references {
        reference.validate()?;
        ensure!(
            seen.insert((
                reference.namespace,
                reference.record_id,
                reference.revision_id
            )),
            "duplicate intent evidence"
        );
    }
    let mut seen = BTreeSet::new();
    for descriptor in media {
        descriptor.validate()?;
        ensure!(seen.insert(&descriptor.sha256), "duplicate intent media");
    }
    Ok(())
}

pub(super) fn validate_receipt_schema(schema: u32, receipt: &Receipt) -> Result<()> {
    match (schema, &receipt.admitted_intent) {
        (super::SCHEMA, None) => Ok(()),
        (INTENT_SCHEMA, Some(intent)) => intent.validate(&receipt.acknowledgment),
        _ => anyhow::bail!("receipt schema/intent mismatch"),
    }
}
pub(super) fn validate_settlement_schema(schema: u32, fact: &OutcomeFact) -> Result<()> {
    match (schema, &fact.settlement) {
        (super::SCHEMA, None) => Ok(()),
        (INTENT_SCHEMA, Some(settlement)) => settlement.validate(fact),
        _ => anyhow::bail!("outcome schema/settlement mismatch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{AttemptReason, Ledger, Outcome, Record, Root};
    use crate::storage::{Envelope, Kind, FORMAT};

    fn reference(namespace: Namespace, record_id: Uuid) -> IntentRecordRef {
        IntentRecordRef {
            namespace,
            record_id,
            revision_id: Uuid::new_v4(),
            object: ObjectRef {
                sha256: "a".repeat(64),
                bytes: 120,
            },
        }
    }
    fn receipt() -> Receipt {
        let operation = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        let turn = Uuid::new_v4();
        let root_revision = reference(Namespace::Conversation, conversation);
        let acknowledgment = Acknowledgment {
            operation,
            conversation,
            records: vec![turn],
            applied_root_revision: root_revision.revision_id,
            binding: None,
        };
        Receipt {
            fingerprint: "b".repeat(64),
            acknowledgment,
            admitted_intent: Some(AdmittedIntent {
                operation,
                request_fingerprint: "c".repeat(64),
                conversation,
                turn,
                root_revision,
                turn_revision: reference(Namespace::Conversation, turn),
                selection: IntentSelectionEvidence {
                    group: Uuid::new_v4(),
                    key_sha256: "d".repeat(64),
                    binding_sha256: "e".repeat(64),
                    store_generation: Uuid::new_v4(),
                    aggregate_generation: Uuid::new_v4(),
                    accepted_base_sha256: "f".repeat(64),
                    selection_sha256: "1".repeat(64),
                },
                source: SourceObservation {
                    document: Uuid::new_v4(),
                    page: Uuid::new_v4(),
                    session: "session".into(),
                    visit: "visit".into(),
                    content_revision: Some("content".into()),
                    order_revision: Some("order".into()),
                    capability_revision: "capability".into(),
                    evidence_procedure: "synthetic-codec-fixture".into(),
                },
                evidence: vec![reference(Namespace::Source, Uuid::new_v4())],
                media: vec![],
            }),
        }
    }
    fn envelope(record: Record, schema: u32, id: Uuid) -> Envelope {
        Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: schema,
            record_id: id,
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: Uuid::new_v4(),
            kind: Kind::Value,
            payload: serde_json::to_value(record).unwrap(),
            media_descriptors: vec![],
        }
    }
    #[test]
    fn receipt_version_boundary_and_exact_identity() {
        let original = receipt();
        let id = Ledger::receipt_id(original.acknowledgment.operation);
        let good = envelope(Record::Receipt(original.clone()), INTENT_SCHEMA, id);
        assert!(Ledger::decode(&good).is_ok());
        for schema in [0, 1, 3, u32::MAX] {
            let mut bad = good.clone();
            bad.domain_schema_version = schema;
            assert!(Ledger::decode(&bad).is_err());
        }
        let mut old = original.clone();
        old.admitted_intent = None;
        let legacy = envelope(Record::Receipt(old.clone()), 1, id);
        assert!(Ledger::decode(&legacy).is_ok());
        assert!(legacy.payload["record"].get("admitted_intent").is_none());
        assert!(Ledger::decode(&envelope(Record::Receipt(old), INTENT_SCHEMA, id)).is_err());
        for change in 0..9 {
            let mut bad = original.clone();
            let intent = bad.admitted_intent.as_mut().unwrap();
            match change {
                0 => intent.operation = Uuid::new_v4(),
                1 => intent.turn_revision.record_id = Uuid::new_v4(),
                2 => intent.root_revision.revision_id = Uuid::new_v4(),
                3 => intent.selection.aggregate_generation = Uuid::nil(),
                4 => intent.selection.selection_sha256 = "X".repeat(64),
                5 => intent.evidence.clear(),
                6 => intent.evidence.push(intent.evidence[0].clone()),
                7 => intent.root_revision.object.bytes = MAX_RECORD as u64 + 1,
                _ => intent.source.document = Uuid::nil(),
            }
            assert!(
                Ledger::decode(&envelope(Record::Receipt(bad), INTENT_SCHEMA, id)).is_err(),
                "mutation {change}"
            );
        }
        let mut foreign = good;
        foreign.namespace = Namespace::Source;
        assert!(Ledger::decode(&foreign).is_err());
    }
    #[test]
    fn schema_two_cannot_promote_other_variants() {
        let id = Uuid::new_v4();
        let record = Record::Root(Root {
            id,
            next_sequence: 1,
            binding: None,
            created_ms: 0,
            updated_ms: 0,
        });
        assert!(Ledger::decode(&envelope(record.clone(), 1, id)).is_ok());
        assert!(Ledger::decode(&envelope(record, INTENT_SCHEMA, id)).is_err());
    }
    #[test]
    fn settlement_is_bound_to_original_receipt_and_bounded_evidence() {
        let receipt = receipt();
        let admitted = receipt.admitted_intent.unwrap();
        let settlement = IntentSettlement {
            operation: admitted.operation,
            request_fingerprint: admitted.request_fingerprint,
            original_receipt: reference(
                Namespace::Conversation,
                Ledger::receipt_id(admitted.operation),
            ),
            original_root: admitted.root_revision,
            original_turn: admitted.turn_revision,
            original_selection: admitted.selection,
            state: IntentSettlementState::Unknown,
            procedure: "synthetic-codec-fixture".into(),
            origin: "host-fixture".into(),
            evidence: vec![],
            media: vec![],
        };
        let fact = OutcomeFact {
            id: Uuid::new_v4(),
            conversation: admitted.conversation,
            turn: admitted.turn,
            outcome: Outcome::ReconcileRequired,
            reason: AttemptReason::DeviceUncertain,
            settlement: Some(settlement),
        };
        let good = envelope(Record::OutcomeFact(fact.clone()), INTENT_SCHEMA, fact.id);
        assert!(Ledger::decode(&good).is_ok());
        for change in 0..5 {
            let mut bad = fact.clone();
            let settlement = bad.settlement.as_mut().unwrap();
            match change {
                0 => settlement.original_turn.record_id = Uuid::new_v4(),
                1 => settlement.original_receipt.record_id = Uuid::new_v4(),
                2 => settlement.state = IntentSettlementState::VerifiedSubmitted,
                3 => settlement.procedure.clear(),
                _ => settlement.original_root.namespace = Namespace::Source,
            }
            assert!(
                Ledger::decode(&envelope(Record::OutcomeFact(bad), INTENT_SCHEMA, fact.id))
                    .is_err()
            );
        }
        let mut old = fact;
        old.settlement = None;
        assert!(Ledger::decode(&envelope(Record::OutcomeFact(old.clone()), 1, old.id)).is_ok());
        assert!(Ledger::decode(&envelope(
            Record::OutcomeFact(old.clone()),
            INTENT_SCHEMA,
            old.id
        ))
        .is_err());
    }
}
