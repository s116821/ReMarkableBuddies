//! Persist uncertainty before submission. Publication itself invokes no effect.
use super::*;
use crate::storage::selection::{SelectionPublication, SelectionToken};
use crate::storage::{Kind, MAX_RECORD};
use serde::{Deserialize, Serialize};
pub(crate) mod sealed {
    pub trait Sealed {}
}
/// Live qualified adapter checks cannot be reconstructed from stored strings.
/// No production implementation exists while SDK native qualification is open.
pub trait SourceAdmission: sealed::Sealed {
    fn verify_current(&self, request: &PendingIntentRequest) -> Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PendingIntentRequest {
    pub operation: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub updated_ms: u64,
    pub selection: IntentSelectionEvidence,
    pub source: SourceObservation,
    pub evidence: Vec<IntentRecordRef>,
    pub media: Vec<Media>,
}
pub enum PendingIntentPublication {
    /// Actual publication token must be revalidated for every later handoff.
    Published {
        publication: SelectionPublication,
        receipt: Receipt,
        reference: IntentRecordRef,
    },
    /// Deliberately contains no live token and never starts an effect.
    Historical(HistoricalIntent),
}
impl IntentSelectionEvidence {
    pub fn from_token(token: &SelectionToken) -> Self {
        Self {
            group: token.scope().group,
            key_sha256: token.scope().key_sha256.clone(),
            binding_sha256: token.scope().binding_sha256.clone(),
            store_generation: token.store_generation(),
            aggregate_generation: token.aggregate_generation(),
            accepted_base_sha256: token.accepted_base_sha256().into(),
            selection_sha256: token.selection_sha256().into(),
        }
    }
}
fn pending_id(operation: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:remarkable-buddies:outcome:v1:{operation}").as_bytes(),
    )
}
fn pin(envelope: &Envelope) -> Result<IntentRecordRef> {
    let bytes = serde_json::to_vec(envelope)?;
    ensure!(
        bytes.len() <= MAX_RECORD,
        "prepared intent record exceeds bound"
    );
    Ok(IntentRecordRef {
        namespace: envelope.namespace,
        record_id: envelope.record_id,
        revision_id: envelope.revision_id,
        object: ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        },
    })
}
fn fingerprint(
    request: &PendingIntentRequest,
    root: &IntentRecordRef,
    turn: &IntentRecordRef,
    fact: &IntentRecordRef,
) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        "selected-output-pending-v1",
        request,
        root,
        turn,
        fact,
    ))?))
}
fn new_record(
    actor_id: Uuid,
    id: Uuid,
    parents: BTreeSet<Uuid>,
    record: Record,
    media: Vec<Media>,
) -> Result<Envelope> {
    let envelope = Envelope {
        envelope_version: FORMAT,
        namespace: Namespace::Conversation,
        domain_schema_version: if matches!(&record, Record::Receipt(_)) {
            INTENT_SCHEMA
        } else {
            SCHEMA
        },
        record_id: id,
        revision_id: Uuid::new_v4(),
        parents,
        operation_id: Uuid::new_v4(),
        actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(record)?,
        media_descriptors: media,
    };
    envelope.validate()?;
    Ledger::decode(&envelope)?;
    Ok(envelope)
}
impl SelectedAdmission {
    /// Look up an original operation before preparing new UUIDs/fingerprints.
    /// None for `accepted` permits historical recovery only. No backend is called.
    pub fn publish_pending(
        &self,
        accepted: Option<&SelectionToken>,
        request: PendingIntentRequest,
        source_admission: Option<&dyn SourceAdmission>,
    ) -> Result<PendingIntentPublication> {
        ensure!(!request.operation.is_nil(), "nil pending operation");
        ensure!(
            request.evidence.len() <= MAX_ITEMS
                && request.media.len() <= MAX_ITEMS
                && serde_json::to_vec(&request)?.len() <= MAX_RECORD,
            "pending request exceeds bound"
        );
        self.mutate(|store| {
            publish_in_store(store, self.scope(), accepted, request, source_admission)
        })
    }
}
fn publish_in_store(
    store: &Store,
    scope: &crate::storage::selection::SelectionScope,
    accepted: Option<&SelectionToken>,
    request: PendingIntentRequest,
    source_admission: Option<&dyn SourceAdmission>,
) -> Result<PendingIntentPublication> {
    if let Some(original) = admission::recover_original(store, scope, request.operation)? {
        let intent = original
            .receipt
            .admitted_intent
            .as_ref()
            .context("original admitted intent absent")?;
        let root_bytes = original
            .original_objects
            .get(&intent.root_revision.object.sha256)
            .context("original prepared root bytes absent")?;
        let root_envelope: Envelope = serde_json::from_slice(root_bytes)?;
        let Record::Root(root) = Ledger::decode(&root_envelope)? else {
            bail!("original prepared root variant mismatch")
        };
        let turn_bytes = original
            .original_objects
            .get(&intent.turn_revision.object.sha256)
            .context("original prepared turn bytes absent")?;
        let turn_envelope: Envelope = serde_json::from_slice(turn_bytes)?;
        let Record::Turn(turn) = Ledger::decode(&turn_envelope)? else {
            bail!("original prepared turn variant mismatch")
        };
        ensure!(
            turn.updated_ms == root.updated_ms && turn.outcome == Outcome::ReconcileRequired,
            "original pending chronology or state mismatch"
        );
        let original_request = PendingIntentRequest {
            operation: intent.operation,
            conversation: intent.conversation,
            turn: intent.turn,
            updated_ms: root.updated_ms,
            selection: intent.selection.clone(),
            source: intent.source.clone(),
            evidence: intent.evidence.clone(),
            media: intent.media.clone(),
        };
        ensure!(
            request == original_request,
            "pending original payload or pins conflict"
        );
        let fact_id = pending_id(request.operation);
        let fact = original
            .transaction
            .selected
            .records
            .iter()
            .find_map(|reference| {
                let bytes = original.original_objects.get(&reference.sha256)?;
                let envelope = serde_json::from_slice::<Envelope>(bytes).ok()?;
                (envelope.namespace == Namespace::Conversation && envelope.record_id == fact_id)
                    .then_some((reference, envelope))
            })
            .context("original pending fact absent")?;
        ensure!(
            matches!(Ledger::decode(&fact.1)?,Record::OutcomeFact(ref f)
                    if f.conversation==request.conversation && f.turn==request.turn && f.reason==AttemptReason::OutputPending
                    && f.outcome==Outcome::ReconcileRequired && f.settlement.is_none()),
            "original pending fact mismatch"
        );
        let fact_pin = IntentRecordRef {
            namespace: fact.1.namespace,
            record_id: fact.1.record_id,
            revision_id: fact.1.revision_id,
            object: fact.0.clone(),
        };
        let expected = fingerprint(
            &request,
            &intent.root_revision,
            &intent.turn_revision,
            &fact_pin,
        )?;
        ensure!(
            original.receipt.fingerprint == expected && intent.request_fingerprint == expected,
            "pending operation payload or pins conflict"
        );
        return Ok(PendingIntentPublication::Historical(original));
    }
    let accepted = accepted.context("new pending intent requires a live token")?;
    let source_admission =
        source_admission.context("new pending intent requires qualified live source admission")?;
    ensure!(
        accepted.scope() == scope
            && IntentSelectionEvidence::from_token(accepted) == request.selection,
        "pending original selection pins mismatch"
    );
    let snapshot = store
        .selected_snapshot(scope, MAX_ITEMS)?
        .context("pending selected scope absent")?;
    ensure!(
        snapshot.token == *accepted,
        "pending selection replaced or stale"
    );
    let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
    let owners = projection.document_ownership()?;
    ensure!(
        owners.get(&request.conversation)
            == Some(&DocumentOwnership::Document(request.source.document)),
        "pending document ownership deferred or foreign"
    );
    Ledger::validate_observation(&request.source)?;
    let (root_envelope, Some(Record::Root(root))) = projection
        .head(Namespace::Conversation, request.conversation)
        .context("pending root missing")?
    else {
        bail!("pending root deleted or wrong variant")
    };
    let (turn_envelope, Some(Record::Turn(turn))) = projection
        .head(Namespace::Conversation, request.turn)
        .context("pending turn missing")?
    else {
        bail!("pending turn deleted or wrong variant")
    };
    ensure!(
        turn.conversation == request.conversation
            && turn.role == Role::Assistant
            && turn.outcome == Outcome::Generated,
        "pending turn is not an unsubmitted generated draft"
    );
    ensure!(
        request.updated_ms >= root.updated_ms && request.updated_ms >= turn.updated_ms,
        "pending chronology moved backward"
    );
    let mut source_link = false;
    for reference in &request.evidence {
        let record = projection.exact_reference(reference)?;
        ensure!(
            selected::conversation_id(record) == request.conversation,
            "foreign pending evidence"
        );
        match record {
            Record::Source(source)
                if source.conversation == request.conversation
                    && source.observation == request.source =>
            {
                source_link = true
            }
            Record::Binding(binding)
                if binding.conversation == request.conversation
                    && binding.receipt.source == request.source =>
            {
                source_link = true
            }
            _ => {}
        }
    }
    ensure!(
        source_link,
        "pending exact source observation evidence absent"
    );
    for media in &request.media {
        media.validate()?;
        ensure!(
            snapshot
                .media
                .get(&media.sha256)
                .is_some_and(|source| source.reference()
                    == ObjectRef {
                        sha256: media.sha256.clone(),
                        bytes: media.bytes
                    }),
            "pending media outside pinned closure"
        );
        ensure!(
            snapshot
                .selected_records
                .iter()
                .any(|record| record.media_descriptors.contains(media)),
            "pending media descriptor differs from immutable provenance"
        );
    }
    source_admission.verify_current(&request)?;
    let mut root = root.clone();
    root.updated_ms = request.updated_ms;
    let mut turn = turn.clone();
    turn.updated_ms = request.updated_ms;
    turn.outcome = Outcome::ReconcileRequired;
    let root = new_record(
        store.actor_id,
        request.conversation,
        BTreeSet::from([root_envelope.revision_id]),
        Record::Root(root),
        vec![],
    )?;
    let turn = new_record(
        store.actor_id,
        request.turn,
        BTreeSet::from([turn_envelope.revision_id]),
        Record::Turn(turn),
        vec![],
    )?;
    let fact = new_record(
        store.actor_id,
        pending_id(request.operation),
        BTreeSet::new(),
        Record::OutcomeFact(OutcomeFact {
            id: pending_id(request.operation),
            conversation: request.conversation,
            turn: request.turn,
            outcome: Outcome::ReconcileRequired,
            reason: AttemptReason::OutputPending,
            settlement: None,
        }),
        vec![],
    )?;
    let root_pin = pin(&root)?;
    let turn_pin = pin(&turn)?;
    let fact_pin = pin(&fact)?;
    let fingerprint = fingerprint(&request, &root_pin, &turn_pin, &fact_pin)?;
    source_admission.verify_current(&request)?;
    let receipt = Receipt {
        fingerprint: fingerprint.clone(),
        acknowledgment: Acknowledgment {
            operation: request.operation,
            conversation: request.conversation,
            records: vec![request.turn, fact.record_id],
            applied_root_revision: root.revision_id,
            binding: root_envelope_binding(root_envelope)?,
        },
        admitted_intent: Some(AdmittedIntent {
            operation: request.operation,
            request_fingerprint: fingerprint,
            conversation: request.conversation,
            turn: request.turn,
            root_revision: root_pin,
            turn_revision: turn_pin,
            selection: request.selection,
            source: request.source,
            evidence: request.evidence,
            media: request.media.clone(),
        }),
    };
    let envelope = new_record(
        store.actor_id,
        Ledger::receipt_id(request.operation),
        BTreeSet::new(),
        Record::Receipt(receipt.clone()),
        request.media,
    )?;
    let reference = pin(&envelope)?;
    let publication = store.commit_selected(
        accepted,
        request.operation,
        vec![root, turn, fact, envelope],
        BTreeMap::new(),
    )?;
    Ok(PendingIntentPublication::Published {
        publication,
        receipt,
        reference,
    })
}

fn root_envelope_binding(envelope: &Envelope) -> Result<Option<Uuid>> {
    match Ledger::decode(envelope)? {
        Record::Root(root) => Ok(root.binding),
        _ => bail!("pending root variant mismatch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::selection::{SelectionChange, SelectionScope};
    use crate::storage::{Manifest, Scope, StorePaths};
    use std::cell::Cell;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("buddy-pending-{}", Uuid::new_v4())))
        }
        fn paths(&self) -> StorePaths {
            StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("secrets"),
            }
        }
        fn setup(
            &self,
        ) -> (
            Arc<Store>,
            SelectedAdmission,
            SelectionToken,
            PendingIntentRequest,
        ) {
            let store = Arc::new(Store::open(self.paths()).unwrap());
            let scope = SelectionScope {
                group: Uuid::new_v4(),
                key_sha256: digest(b"document"),
                binding_sha256: digest(b"binding"),
            };
            let handle = SelectedAdmission::new(store.clone(), scope).unwrap();
            let conversation = Uuid::new_v4();
            let turn = Uuid::new_v4();
            let document = Uuid::new_v4();
            let page = Uuid::new_v4();
            let source = SourceObservation {
                document,
                page,
                session: "synthetic fixture".into(),
                visit: "synthetic fixture".into(),
                content_revision: None,
                order_revision: None,
                capability_revision: "synthetic fixture".into(),
                evidence_procedure: "no native qualification".into(),
            };
            let binding_id = Ledger::binding_id(document, page);
            let mut template = Envelope {
                envelope_version: FORMAT,
                namespace: Namespace::Conversation,
                domain_schema_version: SCHEMA,
                record_id: conversation,
                revision_id: Uuid::new_v4(),
                parents: BTreeSet::new(),
                operation_id: Uuid::new_v4(),
                actor_id: Uuid::new_v4(),
                kind: Kind::Value,
                payload: serde_json::Value::Null,
                media_descriptors: vec![],
            };
            template.payload = serde_json::to_value(Record::Root(Root {
                id: conversation,
                next_sequence: 1,
                binding: Some(binding_id),
                created_ms: 1,
                updated_ms: 1,
            }))
            .unwrap();
            let mut draft = template.clone();
            draft.record_id = turn;
            draft.revision_id = Uuid::new_v4();
            draft.operation_id = Uuid::new_v4();
            draft.payload = serde_json::to_value(Record::Turn(Turn {
                id: turn,
                conversation,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::Assistant,
                mode: Mode::Reader,
                outcome: Outcome::Generated,
                text: Some("draft".into()),
                sources: vec![],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            }))
            .unwrap();
            let mut binding = template.clone();
            binding.record_id = binding_id;
            binding.revision_id = Uuid::new_v4();
            binding.operation_id = Uuid::new_v4();
            binding.payload = serde_json::to_value(Record::Binding(Binding {
                id: binding_id,
                conversation,
                receipt: BindingReceipt {
                    operation: Uuid::new_v4(),
                    conversation,
                    request_fingerprint: digest(b"binding fixture"),
                    adapter_fingerprint: "synthetic".into(),
                    source: source.clone(),
                    intended_target: Some(page),
                    observed_document: document,
                    observed_page: page,
                    expected_order: vec![page],
                    observed_order: vec![page],
                    persisted_revision: "synthetic".into(),
                    observed_ms: 1,
                    evidence_origin: "UnqualifiedSyntheticModel".into(),
                },
            }))
            .unwrap();
            let evidence = pin(&binding).unwrap();
            let mut objects = BTreeMap::new();
            let mut refs = vec![];
            let mut namespaces = BTreeMap::new();
            for envelope in [template, draft, binding] {
                Ledger::decode(&envelope).unwrap();
                let reference = pin(&envelope).unwrap();
                objects.insert(
                    reference.object.sha256.clone(),
                    serde_json::to_vec(&envelope).unwrap(),
                );
                namespaces.insert(reference.object.sha256.clone(), envelope.namespace);
                refs.push(reference.object);
            }
            let publication = handle
                .initialize(
                    SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: digest(b"base"),
                        retained: vec![],
                        selected: Manifest {
                            format: FORMAT,
                            transaction_id: Uuid::new_v4(),
                            scope: Scope::SelectedRecords,
                            records: refs,
                            record_namespaces: namespaces,
                            media: vec![],
                            media_coverage: vec![],
                        },
                    },
                    objects,
                )
                .unwrap();
            let request = PendingIntentRequest {
                operation: Uuid::new_v4(),
                conversation,
                turn,
                updated_ms: 2,
                selection: IntentSelectionEvidence::from_token(&publication.token),
                source,
                evidence: vec![evidence],
                media: vec![],
            };
            (store, handle, publication.token, request)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    // Test-only live-check simulation. There is no production native implementer.
    struct MockSource {
        source: SourceObservation,
        calls: Cell<usize>,
        refuse_at: Option<usize>,
    }
    impl sealed::Sealed for MockSource {}
    impl SourceAdmission for MockSource {
        fn verify_current(&self, request: &PendingIntentRequest) -> Result<()> {
            self.calls.set(self.calls.get() + 1);
            ensure!(
                self.source == request.source && self.refuse_at != Some(self.calls.get()),
                "fixture source revoked"
            );
            Ok(())
        }
    }
    #[test]
    fn original_replay_uses_prepared_refs_before_current_heads_and_rejects_changed_payload_or_pins()
    {
        let fixture = Fixture::new();
        let (store, handle, token, request) = fixture.setup();
        let check = MockSource {
            source: request.source.clone(),
            calls: Cell::new(0),
            refuse_at: None,
        };
        let PendingIntentPublication::Published {
            publication,
            reference,
            ..
        } = handle
            .publish_pending(Some(&token), request.clone(), Some(&check))
            .unwrap()
        else {
            panic!("first publication must be new")
        };
        assert_eq!(check.calls.get(), 2);
        assert_ne!(publication.token, token);
        let selected = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        for record in &selected.selected_records {
            if record.parents.len() == 1
                || record.record_id == reference.record_id
                || record.record_id == pending_id(request.operation)
            {
                assert_eq!(record.actor_id, store.actor_id);
            }
        }
        let before = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token;
        let PendingIntentPublication::Historical(recovered) = handle
            .publish_pending(Some(&token), request.clone(), None)
            .unwrap()
        else {
            panic!("identical retry must be historical")
        };
        assert_eq!(recovered.receipt_reference, reference);
        assert_eq!(check.calls.get(), 2);
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            before
        );
        let snapshot = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let mut selected = snapshot.transaction.selected.clone();
        selected.transaction_id = Uuid::new_v4();
        handle
            .activate(
                &snapshot.token,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"replacement"),
                    selected,
                    retained: vec![],
                },
                BTreeMap::new(),
            )
            .unwrap();
        let PendingIntentPublication::Historical(replaced) =
            handle.publish_pending(None, request.clone(), None).unwrap()
        else {
            panic!("later replacement cannot refresh replay authority")
        };
        assert_eq!(replaced.receipt_reference, reference);
        let before = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token;
        for field in 0..3 {
            let mut changed = request.clone();
            match field {
                0 => changed.source.visit.push_str(" changed"),
                1 => changed.updated_ms += 1,
                _ => changed.selection.aggregate_generation = Uuid::new_v4(),
            }
            assert!(handle.publish_pending(None, changed, None).is_err());
        }
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
    fn missing_or_revoked_live_source_never_publishes_new_pending_intent() {
        let fixture = Fixture::new();
        let (store, handle, token, request) = fixture.setup();
        assert!(handle
            .publish_pending(Some(&token), request.clone(), None)
            .is_err());
        let check = MockSource {
            source: request.source.clone(),
            calls: Cell::new(0),
            refuse_at: Some(2),
        };
        assert!(handle
            .publish_pending(Some(&token), request.clone(), Some(&check))
            .is_err());
        assert_eq!(check.calls.get(), 2);
        assert!(store
            .selected_receipt(handle.scope(), request.operation)
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            token
        );
    }
    #[test]
    fn committed_lost_ack_restart_returns_historical_preparation_without_source_or_token() {
        let fixture = Fixture::new();
        let (store, handle, token, request) = fixture.setup();
        let check = MockSource {
            source: request.source.clone(),
            calls: Cell::new(0),
            refuse_at: None,
        };
        store.set_fault(crate::storage::Fault::AfterCommit).unwrap();
        assert!(handle
            .publish_pending(Some(&token), request.clone(), Some(&check))
            .is_err());
        let scope = handle.scope().clone();
        drop(handle);
        drop(store);
        let store = Arc::new(Store::open(fixture.paths()).unwrap());
        let handle = SelectedAdmission::new(store.clone(), scope).unwrap();
        let original = handle
            .recover_original_intent(request.operation)
            .unwrap()
            .unwrap();
        let PendingIntentPublication::Historical(replayed) =
            handle.publish_pending(None, request, None).unwrap()
        else {
            panic!("restart must not prepare new intent")
        };
        assert_eq!(replayed.receipt_reference, original.receipt_reference);
        assert_eq!(replayed.original_objects, original.original_objects);
    }
    #[test]
    fn stale_token_refuses_before_live_source_check_and_preserves_selected_membership() {
        let fixture = Fixture::new();
        let (store, handle, token, request) = fixture.setup();
        let snapshot = store
            .selected_snapshot(handle.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let mut selected = snapshot.transaction.selected.clone();
        selected.transaction_id = Uuid::new_v4();
        let replaced = handle
            .activate(
                &token,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"replacement"),
                    selected,
                    retained: vec![],
                },
                BTreeMap::new(),
            )
            .unwrap();
        let check = MockSource {
            source: request.source.clone(),
            calls: Cell::new(0),
            refuse_at: None,
        };
        assert!(handle
            .publish_pending(Some(&token), request.clone(), Some(&check))
            .is_err());
        assert_eq!(check.calls.get(), 0);
        assert!(store
            .selected_receipt(handle.scope(), request.operation)
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            replaced.token
        );
    }
}
