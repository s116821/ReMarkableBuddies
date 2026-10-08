//! Late retained facts and uncertainty. Historical state never grants an effect.
use super::*;
use crate::storage::selection::{
    RetainedCommit, SelectionPublication, SelectionToken, SelectionTransaction,
};
use crate::storage::MAX_RECORD;
use serde::{Deserialize, Serialize};

/// Only an owning verified backend can implement this sealed capability.
/// No production implementer exists until backend qualification is complete.
pub trait SettlementVerification: pending::sealed::Sealed {
    fn verify(
        &self,
        original: &HistoricalIntent,
        request: &RetainedSettlementRequest,
    ) -> Result<()>;
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetainedSettlementRequest {
    pub operation: Uuid,
    pub original_operation: Uuid,
    pub state: IntentSettlementState,
    pub reason: AttemptReason,
    pub procedure: String,
    pub origin: String,
    pub evidence: Vec<IntentRecordRef>,
    pub media: Vec<Media>,
}
pub enum RetainedSettlementPublication {
    Published(SelectionPublication),
    /// Original metadata and fact only, deliberately no Store-minted token.
    Historical {
        transaction: SelectionTransaction,
        fact: Box<OutcomeFact>,
        reference: IntentRecordRef,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentUncertainty {
    Unresolved,
    VerifiedSubmitted,
    VerifiedNoEffect,
}
fn outcome(state: &IntentSettlementState) -> Outcome {
    match state {
        IntentSettlementState::Unknown => Outcome::ReconcileRequired,
        IntentSettlementState::VerifiedSubmitted => Outcome::Completed,
        IntentSettlementState::VerifiedNoEffect => Outcome::Failed,
    }
}
fn fact(original: &HistoricalIntent, request: &RetainedSettlementRequest) -> Result<OutcomeFact> {
    let intent = original
        .receipt
        .admitted_intent
        .as_ref()
        .context("original intent absent")?;
    let fact = OutcomeFact {
        id: pending::pending_id(request.original_operation),
        conversation: intent.conversation,
        turn: intent.turn,
        outcome: outcome(&request.state),
        reason: request.reason.clone(),
        settlement: Some(IntentSettlement {
            operation: intent.operation,
            request_fingerprint: intent.request_fingerprint.clone(),
            original_receipt: original.receipt_reference.clone(),
            original_root: intent.root_revision.clone(),
            original_turn: intent.turn_revision.clone(),
            original_selection: intent.selection.clone(),
            state: request.state.clone(),
            procedure: request.procedure.clone(),
            origin: request.origin.clone(),
            evidence: request.evidence.clone(),
            media: request.media.clone(),
        }),
    };
    fact.settlement.as_ref().unwrap().validate(&fact)?;
    Ok(fact)
}
fn pinned(
    store: &Store,
    original: &HistoricalIntent,
    references: &[ObjectRef],
) -> Result<SelectedDomainProjection> {
    let records = references
        .iter()
        .map(|reference| {
            let bytes = store.read_object(reference)?;
            Ok(serde_json::from_slice::<Envelope>(&bytes)?)
        })
        .collect::<Result<Vec<_>>>()?;
    let projection = SelectedDomainProjection::from_pinned_records(&records, references)?;
    projection.document_ownership()?;
    // Reject a substituted receipt even when other historical record IDs agree.
    ensure!(
        matches!(projection.exact_reference(&original.receipt_reference)?, Record::Receipt(r)
        if r == &original.receipt),
        "retained original receipt differs"
    );
    Ok(projection)
}
fn current_fact(
    store: &Store,
    snapshot: &crate::storage::selection::SelectedSnapshot,
    original: &HistoricalIntent,
) -> Result<(Envelope, OutcomeFact, Vec<ObjectRef>)> {
    ensure!(
        !snapshot
            .transaction
            .selected
            .records
            .contains(&original.receipt_reference.object),
        "intent is still active rather than exclusively retained"
    );
    let retained = snapshot
        .transaction
        .retained
        .iter()
        .flat_map(|m| &m.records)
        .collect::<Vec<_>>();
    for reference in &original.transaction.selected.records {
        ensure!(
            retained.contains(&reference),
            "original intent closure is not fully retained"
        );
    }
    let id = pending::pending_id(original.receipt.acknowledgment.operation);
    let mut refs = original.transaction.selected.records.clone();
    for (envelope, reference) in snapshot.retained_records.iter().zip(
        snapshot
            .transaction
            .retained
            .iter()
            .flat_map(|m| &m.records),
    ) {
        if envelope.namespace == Namespace::Conversation
            && envelope.record_id == id
            && !refs.contains(reference)
        {
            refs.push(reference.clone());
        }
    }
    let projection = pinned(store, original, &refs)?;
    let (head, Some(Record::OutcomeFact(fact))) = projection
        .head(Namespace::Conversation, id)
        .context("retained pending fact absent")?
    else {
        bail!("retained pending fact deleted or foreign");
    };
    ensure!(
        fact.conversation == original.receipt.acknowledgment.conversation
            && fact.turn == original.receipt.admitted_intent.as_ref().unwrap().turn,
        "retained fact original identity mismatch"
    );
    if let Some(settlement) = &fact.settlement {
        let intent = original.receipt.admitted_intent.as_ref().unwrap();
        ensure!(
            settlement.operation == intent.operation
                && settlement.original_receipt == original.receipt_reference
                && settlement.request_fingerprint == intent.request_fingerprint
                && settlement.original_root == intent.root_revision
                && settlement.original_turn == intent.turn_revision
                && settlement.original_selection == intent.selection,
            "retained settlement differs from this original intent"
        );
        ensure!(
            fact.outcome == outcome(&settlement.state),
            "settlement state/outcome mismatch"
        );
        if settlement.state != IntentSettlementState::Unknown {
            ensure!(
                head.actor_id == store.actor_id,
                "foreign verification cannot release local uncertainty"
            );
        }
        let publication = store
            .selected_receipt(snapshot.token.scope(), head.operation_id)?
            .context("retained settlement publication absent")?;
        let reference = projection_reference(&projection, head, &refs, store)?;
        ensure!(
            publication.mutation == crate::storage::selection::SelectionMutation::Retained
                && publication
                    .retained
                    .iter()
                    .any(|m| m.records.contains(&reference.object)),
            "fact is not an original retained publication"
        );
    } else {
        ensure!(
            fact.reason == AttemptReason::OutputPending
                && fact.outcome == Outcome::ReconcileRequired,
            "original pending fact state mismatch"
        );
    }
    Ok((head.clone(), fact.clone(), refs))
}
fn projection_reference(
    projection: &SelectedDomainProjection,
    envelope: &Envelope,
    refs: &[ObjectRef],
    store: &Store,
) -> Result<IntentRecordRef> {
    for reference in refs {
        let bytes = store.read_object(reference)?;
        let item: Envelope = serde_json::from_slice(&bytes)?;
        if item.revision_id == envelope.revision_id {
            let pin = IntentRecordRef {
                namespace: item.namespace,
                record_id: item.record_id,
                revision_id: item.revision_id,
                object: reference.clone(),
            };
            projection.exact_reference(&pin)?;
            return Ok(pin);
        }
    }
    bail!("retained exact head reference absent")
}
impl SelectedAdmission {
    /// Read-only reconstruction; errors must keep caller uncertainty in place.
    pub fn retained_intent_uncertainty(
        &self,
        original_operation: Uuid,
    ) -> Result<IntentUncertainty> {
        self.mutate(|store| {
            let original = admission::recover_original(store, self.scope(), original_operation)?
                .context("original intent absent")?;
            let snapshot = store
                .selected_snapshot(self.scope(), MAX_ITEMS)?
                .context("selected scope absent")?;
            let (_, fact, _) = current_fact(store, &snapshot, &original)?;
            Ok(match fact.settlement.map(|s| s.state) {
                None | Some(IntentSettlementState::Unknown) => IntentUncertainty::Unresolved,
                Some(IntentSettlementState::VerifiedSubmitted) => {
                    IntentUncertainty::VerifiedSubmitted
                }
                Some(IntentSettlementState::VerifiedNoEffect) => {
                    IntentUncertainty::VerifiedNoEffect
                }
            })
        })
    }
    pub fn settle_retained(
        &self,
        accepted: Option<&SelectionToken>,
        request: RetainedSettlementRequest,
        verification: Option<&dyn SettlementVerification>,
    ) -> Result<RetainedSettlementPublication> {
        ensure!(
            !request.operation.is_nil()
                && !request.original_operation.is_nil()
                && request.operation != request.original_operation,
            "invalid retained request operations"
        );
        ensure!(
            serde_json::to_vec(&request)?.len() <= MAX_RECORD,
            "retained request exceeds bound"
        );
        self.mutate(|store| {
            let original =
                admission::recover_original(store, self.scope(), request.original_operation)?
                    .context("original intent absent")?;
            let expected_fact = fact(&original, &request)?;
            let original_projection =
                pinned(store, &original, &original.transaction.selected.records)?;
            for reference in &request.evidence {
                ensure!(
                    selected::conversation_id(original_projection.exact_reference(reference)?)
                        == expected_fact.conversation,
                    "foreign settlement evidence"
                );
            }
            for media in &request.media {
                ensure!(
                    original
                        .receipt
                        .admitted_intent
                        .as_ref()
                        .unwrap()
                        .media
                        .contains(media),
                    "foreign settlement media"
                );
            }
            if let Some(transaction) = store.selected_receipt(self.scope(), request.operation)? {
                ensure!(
                    transaction.mutation == crate::storage::selection::SelectionMutation::Retained,
                    "settlement operation conflicts with another publication"
                );
                let mut found = None;
                for reference in transaction.retained.iter().flat_map(|m| &m.records) {
                    let envelope: Envelope =
                        serde_json::from_slice(&store.read_object(reference)?)?;
                    if envelope.operation_id == request.operation
                        && envelope.record_id == expected_fact.id
                    {
                        let Record::OutcomeFact(actual) = Ledger::decode(&envelope)? else {
                            bail!("settlement replay variant mismatch");
                        };
                        ensure!(
                            actual == expected_fact,
                            "settlement replay request conflict"
                        );
                        let pin = IntentRecordRef {
                            namespace: envelope.namespace,
                            record_id: envelope.record_id,
                            revision_id: envelope.revision_id,
                            object: reference.clone(),
                        };
                        if let Some((_, previous)) = &found {
                            ensure!(previous == &pin, "settlement replay fork");
                        }
                        found = Some((actual, pin));
                    }
                }
                let (fact, reference) = found.context("settlement replay fact absent")?;
                return Ok(RetainedSettlementPublication::Historical {
                    transaction,
                    fact: Box::new(fact),
                    reference,
                });
            }
            let accepted = accepted.context("new retained settlement requires current token")?;
            ensure!(accepted.scope() == self.scope(), "foreign retained scope");
            let snapshot = store
                .selected_snapshot(self.scope(), MAX_ITEMS)?
                .context("selected scope absent")?;
            ensure!(
                snapshot.token == *accepted,
                "retained selection stale or replaced"
            );
            let (prior, previous_fact, _) = current_fact(store, &snapshot, &original)?;
            ensure!(
                previous_fact
                    .settlement
                    .as_ref()
                    .is_none_or(|s| s.state == IntentSettlementState::Unknown),
                "verified intent is already settled"
            );
            let check = || -> Result<()> {
                if request.state != IntentSettlementState::Unknown {
                    verification
                        .context("verified settlement requires qualified verification")?
                        .verify(&original, &request)?;
                }
                Ok(())
            };
            check()?;
            let envelope = Envelope {
                envelope_version: FORMAT,
                namespace: Namespace::Conversation,
                domain_schema_version: INTENT_SCHEMA,
                record_id: expected_fact.id,
                revision_id: Uuid::new_v4(),
                parents: BTreeSet::from([prior.revision_id]),
                operation_id: request.operation,
                actor_id: store.actor_id,
                kind: Kind::Value,
                payload: serde_json::to_value(Record::OutcomeFact(expected_fact))?,
                media_descriptors: request.media.clone(),
            };
            Ledger::decode(&envelope)?;
            let reference = pending::pin(&envelope)?;
            let mut retained = snapshot.transaction.retained.clone();
            let manifest = retained
                .iter_mut()
                .find(|m| m.records.contains(&original.receipt_reference.object))
                .context("original retained manifest absent")?;
            // The validated closure is already retained across these manifests.
            // Preserve its original namespaces, media and completeness metadata.
            manifest
                .record_namespaces
                .insert(reference.object.sha256.clone(), Namespace::Conversation);
            manifest.records.push(reference.object.clone());
            check()?;
            let publication = store.commit_retained(
                accepted,
                RetainedCommit {
                    operation: request.operation,
                    original_operation: request.original_operation,
                    intent: original.receipt_reference.object,
                    retained,
                },
                BTreeMap::from([(reference.object.sha256, serde_json::to_vec(&envelope)?)]),
            )?;
            Ok(RetainedSettlementPublication::Published(publication))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::selection::SelectionChange;
    use pending::tests::{Fixture, MockSource};
    use std::cell::Cell;

    // Synthetic domain fixture only. No native verification implementation.
    struct MockVerification {
        calls: Cell<usize>,
        operation: Uuid,
        refuse_at: Option<usize>,
    }
    impl pending::sealed::Sealed for MockVerification {}
    impl SettlementVerification for MockVerification {
        fn verify(
            &self,
            original: &HistoricalIntent,
            request: &RetainedSettlementRequest,
        ) -> Result<()> {
            self.calls.set(self.calls.get() + 1);
            ensure!(
                request.original_operation == self.operation
                    && original.receipt.acknowledgment.operation == self.operation
                    && self.refuse_at != Some(self.calls.get()),
                "synthetic verification refused"
            );
            Ok(())
        }
    }
    fn prepare(
        fixture: &Fixture,
    ) -> (
        Arc<Store>,
        SelectedAdmission,
        SelectionToken,
        RetainedSettlementRequest,
    ) {
        prepare_with_export(fixture, None)
    }
    fn prepare_with_export(
        fixture: &Fixture,
        split_export: Option<bool>,
    ) -> (
        Arc<Store>,
        SelectedAdmission,
        SelectionToken,
        RetainedSettlementRequest,
    ) {
        let (store, handle, mut token, mut pending) = fixture.setup();
        let mut export = None;
        if split_export.is_some() {
            let snapshot = store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap();
            let mut envelope = snapshot.selected_records[0].clone();
            envelope.namespace = Namespace::ExportAssociation;
            envelope.record_id = Uuid::new_v4();
            envelope.revision_id = Uuid::new_v4();
            envelope.operation_id = Uuid::new_v4();
            envelope.payload = serde_json::to_value(Record::Export(ExportAssociation {
                id: envelope.record_id,
                conversation: pending.conversation,
                backend: "synthetic backend".into(),
                native_note: None,
                source_scope: "all-turns".into(),
                source_revision: "synthetic source revision".into(),
                operation: envelope.operation_id,
                outcome: Outcome::ReconcileRequired,
            }))
            .unwrap();
            let pin = pending::pin(&envelope).unwrap();
            let mut selected = snapshot.transaction.selected;
            selected.records.push(pin.object.clone());
            selected
                .record_namespaces
                .insert(pin.object.sha256.clone(), Namespace::ExportAssociation);
            token = handle
                .activate(
                    &token,
                    SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: token.accepted_base_sha256().into(),
                        selected,
                        retained: snapshot.transaction.retained,
                    },
                    BTreeMap::from([(
                        pin.object.sha256.clone(),
                        serde_json::to_vec(&envelope).unwrap(),
                    )]),
                )
                .unwrap()
                .token;
            pending.selection = IntentSelectionEvidence::from_token(&token);
            export = Some(pin.object);
        }
        let source = MockSource {
            source: pending.source.clone(),
            calls: Cell::new(0),
            refuse_at: None,
        };
        handle
            .publish_pending(Some(&token), pending.clone(), Some(&source))
            .unwrap();
        let snapshot = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let mut winner = snapshot.selected_records[0].clone();
        let id = Uuid::new_v4();
        winner.record_id = id;
        winner.revision_id = Uuid::new_v4();
        winner.parents.clear();
        winner.operation_id = Uuid::new_v4();
        winner.payload = serde_json::to_value(Record::Root(Root {
            id,
            next_sequence: 0,
            binding: None,
            created_ms: 3,
            updated_ms: 3,
        }))
        .unwrap();
        let pin = pending::pin(&winner).unwrap();
        let mut manifest = snapshot.transaction.selected.clone();
        manifest.transaction_id = Uuid::new_v4();
        manifest.records = vec![pin.object.clone()];
        manifest.record_namespaces =
            BTreeMap::from([(pin.object.sha256.clone(), Namespace::Conversation)]);
        manifest.media.clear();
        manifest.media_coverage.clear();
        let mut retained = vec![snapshot.transaction.selected];
        if split_export == Some(true) {
            let export = export.unwrap();
            let mut export_manifest = retained[0].clone();
            export_manifest.records = vec![export.clone()];
            export_manifest.record_namespaces =
                BTreeMap::from([(export.sha256.clone(), Namespace::ExportAssociation)]);
            retained[0].records.retain(|reference| reference != &export);
            retained[0].record_namespaces.remove(&export.sha256);
            retained.push(export_manifest);
        }
        let publication = handle
            .activate(
                &snapshot.token,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"synthetic replacement"),
                    selected: manifest,
                    retained,
                },
                BTreeMap::from([(pin.object.sha256, serde_json::to_vec(&winner).unwrap())]),
            )
            .unwrap();
        let request = RetainedSettlementRequest {
            operation: Uuid::new_v4(),
            original_operation: pending.operation,
            state: IntentSettlementState::Unknown,
            reason: AttemptReason::DeviceUncertain,
            procedure: "synthetic procedure; no native qualification".into(),
            origin: "TestMock".into(),
            evidence: pending.evidence,
            media: pending.media,
        };
        (store, handle, publication.token, request)
    }
    #[test]
    fn split_export_closure_settles_without_copying_or_retyping_original_records() {
        for split in [false, true] {
            let fixture = Fixture::new();
            let (store, handle, token, request) = prepare_with_export(&fixture, Some(split));
            assert_eq!(
                handle
                    .retained_intent_uncertainty(request.original_operation)
                    .unwrap(),
                IntentUncertainty::Unresolved
            );
            let before = store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap();
            let export = before
                .transaction
                .retained
                .iter()
                .flat_map(|m| &m.records)
                .find(|r| {
                    before.transaction.retained.iter().any(|m| {
                        m.record_namespaces.get(&r.sha256) == Some(&Namespace::ExportAssociation)
                    })
                })
                .unwrap()
                .clone();
            let original_bytes = store.read_object(&export).unwrap();
            handle
                .settle_retained(Some(&token), request.clone(), None)
                .unwrap();
            let after = store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap();
            assert_eq!(original_bytes, store.read_object(&export).unwrap());
            assert_eq!(
                after
                    .transaction
                    .retained
                    .iter()
                    .flat_map(|m| &m.records)
                    .filter(|r| **r == export)
                    .count(),
                1
            );
            assert!(after
                .transaction
                .retained
                .iter()
                .any(|m| m.record_namespaces.get(&export.sha256)
                    == Some(&Namespace::ExportAssociation)));
            if split {
                assert_eq!(
                    serde_json::to_vec(&before.transaction.retained[1]).unwrap(),
                    serde_json::to_vec(&after.transaction.retained[1]).unwrap()
                );
            }
            assert_eq!(
                handle
                    .retained_intent_uncertainty(request.original_operation)
                    .unwrap(),
                IntentUncertainty::Unresolved
            );
        }
    }
    #[test]
    fn retained_unknown_verified_and_restart_keep_winner_bytes_and_exact_replay() {
        let fixture = Fixture::new();
        let (store, handle, token, request) = prepare(&fixture);
        let mut winner = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .transaction
            .selected;
        assert_eq!(
            handle
                .retained_intent_uncertainty(request.original_operation)
                .unwrap(),
            IntentUncertainty::Unresolved
        );
        let RetainedSettlementPublication::Published(unknown) = handle
            .settle_retained(Some(&token), request.clone(), None)
            .unwrap()
        else {
            panic!("new publication")
        };
        assert_eq!(
            handle
                .retained_intent_uncertainty(request.original_operation)
                .unwrap(),
            IntentUncertainty::Unresolved
        );
        let before = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        winner.transaction_id = before.transaction.selected.transaction_id;
        assert_eq!(
            serde_json::to_vec(&winner).unwrap(),
            serde_json::to_vec(&before.transaction.selected).unwrap()
        );
        assert!(matches!(
            handle.settle_retained(None, request.clone(), None).unwrap(),
            RetainedSettlementPublication::Historical { .. }
        ));
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            before.token
        );
        let mut verified = request.clone();
        verified.operation = Uuid::new_v4();
        verified.state = IntentSettlementState::VerifiedSubmitted;
        assert!(handle
            .settle_retained(Some(&unknown.token), verified.clone(), None)
            .is_err());
        let proof = MockVerification {
            calls: Cell::new(0),
            operation: request.original_operation,
            refuse_at: None,
        };
        let RetainedSettlementPublication::Published(published) = handle
            .settle_retained(Some(&unknown.token), verified.clone(), Some(&proof))
            .unwrap()
        else {
            panic!("verified publication")
        };
        assert_eq!(proof.calls.get(), 2);
        assert_eq!(
            handle
                .retained_intent_uncertainty(request.original_operation)
                .unwrap(),
            IntentUncertainty::VerifiedSubmitted
        );
        let after = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        winner.transaction_id = after.transaction.selected.transaction_id;
        assert_eq!(
            serde_json::to_vec(&winner).unwrap(),
            serde_json::to_vec(&after.transaction.selected).unwrap()
        );
        let mut reopen = request.clone();
        reopen.operation = Uuid::new_v4();
        assert!(handle
            .settle_retained(Some(&published.token), reopen, None)
            .is_err());
        let scope = handle.scope().clone();
        drop(handle);
        drop(store);
        let store = Arc::new(Store::open(fixture.paths()).unwrap());
        let handle = SelectedAdmission::new(store.clone(), scope).unwrap();
        assert_eq!(
            handle
                .retained_intent_uncertainty(request.original_operation)
                .unwrap(),
            IntentUncertainty::VerifiedSubmitted
        );
        let before = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token;
        assert!(matches!(
            handle
                .settle_retained(None, verified.clone(), None)
                .unwrap(),
            RetainedSettlementPublication::Historical { .. }
        ));
        verified.origin = "changed retry".into();
        assert!(handle.settle_retained(None, verified, None).is_err());
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            before
        );
    }
    #[test]
    fn stale_foreign_and_revoked_verification_never_publish_or_release() {
        let fixture = Fixture::new();
        let (store, handle, token, request) = prepare(&fixture);
        let mut foreign = request.clone();
        foreign.evidence[0].object.sha256 = digest(b"foreign");
        assert!(handle.settle_retained(Some(&token), foreign, None).is_err());
        let mut foreign = request.clone();
        foreign.original_operation = Uuid::new_v4();
        assert!(handle.settle_retained(Some(&token), foreign, None).is_err());
        let mut verified = request.clone();
        verified.state = IntentSettlementState::VerifiedNoEffect;
        let proof = MockVerification {
            calls: Cell::new(0),
            operation: request.original_operation,
            refuse_at: Some(2),
        };
        assert!(handle
            .settle_retained(Some(&token), verified.clone(), Some(&proof))
            .is_err());
        assert_eq!(proof.calls.get(), 2);
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            token
        );
        let RetainedSettlementPublication::Published(unknown) =
            handle.settle_retained(Some(&token), request, None).unwrap()
        else {
            panic!()
        };
        verified.operation = Uuid::new_v4();
        let proof = MockVerification {
            calls: Cell::new(0),
            operation: verified.original_operation,
            refuse_at: None,
        };
        assert!(handle
            .settle_retained(Some(&token), verified.clone(), Some(&proof))
            .is_err());
        assert_eq!(proof.calls.get(), 0);
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            unknown.token
        );
        handle
            .settle_retained(Some(&unknown.token), verified.clone(), Some(&proof))
            .unwrap();
        assert_eq!(
            handle
                .retained_intent_uncertainty(verified.original_operation)
                .unwrap(),
            IntentUncertainty::VerifiedNoEffect
        );
    }
    #[test]
    fn foreign_actor_verified_fact_cannot_release_retained_uncertainty() {
        let fixture = Fixture::new();
        let (store, handle, token, mut request) = prepare(&fixture);
        request.state = IntentSettlementState::VerifiedSubmitted;
        let original = handle
            .recover_original_intent(request.original_operation)
            .unwrap()
            .unwrap();
        let snapshot = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let (prior, _, _) = current_fact(&store, &snapshot, &original).unwrap();
        let envelope = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: INTENT_SCHEMA,
            record_id: prior.record_id,
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::from([prior.revision_id]),
            operation_id: request.operation,
            actor_id: Uuid::new_v4(),
            kind: Kind::Value,
            payload: serde_json::to_value(Record::OutcomeFact(fact(&original, &request).unwrap()))
                .unwrap(),
            media_descriptors: vec![],
        };
        // Completed is legal only for an explicit schema-2 verified submission.
        Ledger::decode(&envelope).unwrap();
        let mut invalid = envelope.clone();
        invalid.domain_schema_version = 1;
        assert!(Ledger::decode(&invalid).is_err());
        let mut completed = fact(&original, &request).unwrap();
        completed.settlement = None;
        invalid = envelope.clone();
        invalid.payload = serde_json::to_value(Record::OutcomeFact(completed)).unwrap();
        assert!(Ledger::decode(&invalid).is_err());
        let mut completed = fact(&original, &request).unwrap();
        completed.settlement.as_mut().unwrap().state = IntentSettlementState::Unknown;
        invalid.payload = serde_json::to_value(Record::OutcomeFact(completed)).unwrap();
        assert!(Ledger::decode(&invalid).is_err());
        let pin = pending::pin(&envelope).unwrap();
        let mut retained = snapshot.transaction.retained;
        retained[0].records.push(pin.object.clone());
        retained[0]
            .record_namespaces
            .insert(pin.object.sha256.clone(), Namespace::Conversation);
        store
            .commit_retained(
                &token,
                RetainedCommit {
                    operation: request.operation,
                    original_operation: request.original_operation,
                    intent: original.receipt_reference.object,
                    retained,
                },
                BTreeMap::from([(pin.object.sha256, serde_json::to_vec(&envelope).unwrap())]),
            )
            .unwrap();
        assert!(handle
            .retained_intent_uncertainty(request.original_operation)
            .is_err());
    }
}
