//! Canonical in-process selected admission. This is not native qualification.
use super::{IntentRecordRef, Ledger, Receipt, Record, SelectedDomainProjection};
use crate::storage::selection::{
    SelectedSnapshot, SelectionChange, SelectionMutation, SelectionPublication, SelectionScope,
    SelectionToken, SelectionTransaction,
};
use crate::storage::{Envelope, Namespace, Store, Uuid, MAX_ITEMS};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

type AdmissionKey = (usize, Uuid, String);
type Registry = BTreeMap<AdmissionKey, Weak<Mutex<()>>>;
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

/// Original persisted preparation only. Deliberately contains no live token.
pub struct HistoricalIntent {
    pub transaction: SelectionTransaction,
    pub receipt: Receipt,
    pub receipt_reference: IntentRecordRef,
    pub original_objects: BTreeMap<String, Vec<u8>>,
}

/// All handles for the same Store and aggregate share admission, including a
/// replacement binding. Callers still own qualified source/backend validation.
pub struct SelectedAdmission {
    store: Arc<Store>,
    scope: SelectionScope,
    gate: Arc<Mutex<()>>,
}
impl SelectedAdmission {
    /// Read evidence from one current selection; never returns effect authority.
    /// Token counting happens after admission is released, so replacement cannot
    /// mix new heads into this view or deadlock a caller reading another handle.
    pub fn context_range(
        &self,
        accepted: &SelectionToken,
        conversation: Uuid,
        selection: Option<super::TurnRange>,
        budget: &super::ContextBudget,
        token_count: impl Fn(&[super::Turn]) -> Result<usize>,
    ) -> Result<super::ContextView> {
        Ledger::context_token_limit(selection, budget)?;
        let (records, media) = self.with_current(accepted, |snapshot| {
            let projection = SelectedDomainProjection::from_snapshot(snapshot)?;
            let records = projection.inspect(conversation)?;
            // The selected inventory alone supplies availability; retained media
            // handles cannot make an intentionally omitted winner sample present.
            let media: BTreeMap<_, _> = snapshot
                .transaction
                .selected
                .media
                .iter()
                .map(|reference| (reference.sha256.clone(), reference.clone()))
                .collect();
            Ok((records, media))
        })?;
        Ledger::context_from_records(records, selection, budget, token_count, |item| {
            media
                .get(&item.sha256)
                .is_some_and(|reference| reference.bytes == item.bytes)
        })
    }
    /// Recover the original publication before allocating new preparation IDs.
    /// Replacement membership and lost acknowledgments never refresh authority.
    pub fn recover_original_intent(&self, operation: Uuid) -> Result<Option<HistoricalIntent>> {
        self.mutate(|store| recover_original(store, &self.scope, operation))
    }
    pub fn new(store: Arc<Store>, scope: SelectionScope) -> Result<Self> {
        scope.validate()?;
        let key = (
            Arc::as_ptr(&store) as usize,
            scope.group,
            scope.key_sha256.clone(),
        );
        let mut registry = REGISTRY
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| anyhow::anyhow!("domain admission registry unavailable"))?;
        registry.retain(|_, weak| weak.strong_count() > 0);
        let gate = registry
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let gate = Arc::new(Mutex::new(()));
                registry.insert(key, Arc::downgrade(&gate));
                gate
            });
        drop(registry);
        Ok(Self { store, scope, gate })
    }
    /// Domain lock precedes every Store read/mutation. No Store mutex is held
    /// during the supplied synchronous operation. Keep this closure through the
    /// actual backend submission; a returned historical value is not permission.
    pub fn with_current<T>(
        &self,
        accepted: &SelectionToken,
        operation: impl FnOnce(&SelectedSnapshot) -> Result<T>,
    ) -> Result<T> {
        self.with_current_store(accepted, |_, snapshot| operation(snapshot))
    }
    /// Private locked read seam for Reader closure/uncertainty validation.
    /// The callback must not reacquire admission. Store reads finish before
    /// synchronous backend I/O; admission remains held through that I/O.
    pub(super) fn with_current_store<T>(
        &self,
        accepted: &SelectionToken,
        operation: impl FnOnce(&Store, &SelectedSnapshot) -> Result<T>,
    ) -> Result<T> {
        let _admission = self
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("domain admission unavailable"))?;
        ensure!(
            accepted.scope() == &self.scope,
            "foreign selected admission scope"
        );
        let snapshot = self
            .store
            .selected_snapshot(&self.scope, MAX_ITEMS)?
            .context("selected admission absent")?;
        ensure!(
            &snapshot.token == accepted,
            "selected admission replaced or stale"
        );
        operation(&self.store, &snapshot)
    }
    /// Publication/activation must use the same gate as synchronous handoff.
    pub(super) fn mutate<T>(&self, operation: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let _admission = self
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("domain admission unavailable"))?;
        operation(&self.store)
    }
    pub(super) fn scope(&self) -> &SelectionScope {
        &self.scope
    }
    pub fn initialize(
        &self,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        self.mutate(|store| store.initialize_selected(&self.scope, change, objects))
    }
    pub fn activate(
        &self,
        accepted: &SelectionToken,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        ensure!(
            accepted.scope() == &self.scope,
            "foreign selected activation scope"
        );
        self.mutate(|store| store.activate_selected(accepted, change, objects))
    }
}

pub(super) fn recover_original(
    store: &Store,
    scope: &SelectionScope,
    operation: Uuid,
) -> Result<Option<HistoricalIntent>> {
    let Some(transaction) = store.selected_receipt(scope, operation)? else {
        return Ok(None);
    };
    ensure!(
        transaction.mutation == SelectionMutation::Commit,
        "operation is not an intent publication"
    );
    let mut envelopes = Vec::new();
    let mut original_objects = BTreeMap::new();
    for reference in &transaction.selected.records {
        let bytes = store.read_object(reference)?;
        envelopes.push(serde_json::from_slice::<Envelope>(&bytes)?);
        original_objects.insert(reference.sha256.clone(), bytes);
    }
    let projection =
        SelectedDomainProjection::from_pinned_records(&envelopes, &transaction.selected.records)?;
    let (envelope, Some(Record::Receipt(receipt))) = projection
        .head(Namespace::Conversation, Ledger::receipt_id(operation))
        .context("original intent receipt missing")?
    else {
        anyhow::bail!("original intent receipt variant mismatch");
    };
    let intent = receipt
        .admitted_intent
        .as_ref()
        .context("historical receipt has no admitted intent")?;
    ensure!(
        intent.operation == operation,
        "original intent operation mismatch"
    );
    ensure!(
        intent.selection.group == transaction.scope.group
            && intent.selection.key_sha256 == transaction.scope.key_sha256
            && intent.selection.binding_sha256 == transaction.scope.binding_sha256
            && intent.selection.store_generation == transaction.store_generation
            && intent.selection.accepted_base_sha256 == transaction.accepted_base_sha256
            && Some(intent.selection.selection_sha256.as_str())
                == transaction.previous_sha256.as_deref(),
        "original intent selection differs from publication"
    );
    let predecessor = store
        .selected_predecessor(scope, operation)?
        .context("original intent predecessor absent")?;
    ensure!(
        intent.selection.aggregate_generation == predecessor.aggregate_generation
            && predecessor.scope == transaction.scope
            && predecessor.store_generation == transaction.store_generation
            && predecessor.accepted_base_sha256 == transaction.accepted_base_sha256,
        "original intent selection differs from predecessor"
    );
    ensure!(
        matches!(projection.exact_reference(&intent.root_revision)?,Record::Root(root) if root.id == intent.conversation),
        "original intent root mismatch"
    );
    ensure!(
        matches!(projection.exact_reference(&intent.turn_revision)?,Record::Turn(turn) if turn.id == intent.turn && turn.conversation == intent.conversation),
        "original intent turn mismatch"
    );
    for reference in &intent.evidence {
        projection.exact_reference(reference)?;
    }
    projection.document_ownership()?;
    let index = envelopes
        .iter()
        .position(|record| record.revision_id == envelope.revision_id)
        .unwrap();
    Ok(Some(HistoricalIntent {
        receipt: receipt.clone(),
        receipt_reference: IntentRecordRef {
            namespace: envelope.namespace,
            record_id: envelope.record_id,
            revision_id: envelope.revision_id,
            object: transaction.selected.records[index].clone(),
        },
        transaction,
        original_objects,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::digest;
    #[test]
    fn multiple_handles_and_replacement_bindings_share_only_their_aggregate_gate() {
        // Identity registry behavior does not require native operations.
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture =
            Fixture(std::env::temp_dir().join(format!("buddy-admission-{}", Uuid::new_v4())));
        let store = Arc::new(
            Store::open(crate::storage::StorePaths {
                data: fixture.0.join("data"),
                cache: fixture.0.join("cache"),
                credentials: fixture.0.join("secrets"),
            })
            .unwrap(),
        );
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"document"),
            binding_sha256: digest(b"first binding"),
        };
        let first = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let mut replaced = scope.clone();
        replaced.binding_sha256 = digest(b"replacement");
        let second = SelectedAdmission::new(store.clone(), replaced).unwrap();
        assert!(Arc::ptr_eq(&first.gate, &second.gate));
        let held = first.gate.lock().unwrap();
        assert!(second.gate.try_lock().is_err());
        let mut unrelated = scope;
        unrelated.key_sha256 = digest(b"other document");
        let other = SelectedAdmission::new(store, unrelated).unwrap();
        assert!(other.gate.try_lock().is_ok());
        drop(held);
        assert!(second.gate.try_lock().is_ok());
    }
    #[test]
    fn synchronous_handoff_allows_store_reads_but_serializes_activation_and_refuses_stale_token() {
        use crate::storage::{
            Envelope, Kind, Manifest, Namespace, ObjectRef, Scope, StorePaths, FORMAT,
        };
        use std::sync::Barrier;
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(
            std::env::temp_dir().join(format!("buddy-admission-current-{}", Uuid::new_v4())),
        );
        let store = Arc::new(
            Store::open(StorePaths {
                data: fixture.0.join("data"),
                cache: fixture.0.join("cache"),
                credentials: fixture.0.join("secrets"),
            })
            .unwrap(),
        );
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"document"),
            binding_sha256: digest(b"binding"),
        };
        let first = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let second = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let envelope = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::json!({"domain-opaque":"admission mechanism fixture"}),
            media_descriptors: vec![],
        };
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let hash = digest(&bytes);
        let mut change = SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"base"),
            retained: vec![],
            selected: Manifest {
                format: FORMAT,
                transaction_id: Uuid::new_v4(),
                scope: Scope::SelectedRecords,
                records: vec![ObjectRef {
                    sha256: hash.clone(),
                    bytes: bytes.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
                media: vec![],
                media_coverage: vec![],
            },
        };
        let accepted = first
            .initialize(change.clone(), BTreeMap::from([(hash, bytes)]))
            .unwrap();
        change.operation = Uuid::new_v4();
        change.selected.transaction_id = Uuid::new_v4();
        let barrier = Arc::new(Barrier::new(2));
        let mut worker = None;
        first
            .with_current(&accepted.token, |snapshot| {
                assert_eq!(
                    store.selected_snapshot(&scope, MAX_ITEMS)?.unwrap().token,
                    snapshot.token
                );
                assert!(second.gate.try_lock().is_err());
                let started = barrier.clone();
                let token = accepted.token.clone();
                worker = Some(std::thread::spawn(move || {
                    started.wait();
                    second.activate(&token, change, BTreeMap::new())
                }));
                barrier.wait();
                // This represents the entire synchronous backend interval. Domain
                // admission is still held; the Store mutex is independently usable.
                assert_eq!(
                    store.selected_snapshot(&scope, MAX_ITEMS)?.unwrap().token,
                    snapshot.token
                );
                Ok(())
            })
            .unwrap();
        let replaced = worker.unwrap().join().unwrap().unwrap();
        assert_ne!(replaced.token, accepted.token);
        assert_eq!(
            replaced.transaction.selected.records,
            accepted.transaction.selected.records
        );
        let mut submitted = false;
        assert!(first
            .with_current(&accepted.token, |_| {
                submitted = true;
                Ok(())
            })
            .is_err());
        assert!(!submitted);
    }
    #[test]
    fn lost_ack_recovery_preserves_original_ids_and_bytes_after_restart_and_replacement() {
        recovery_fixture(None);
    }
    #[test]
    fn recovery_refuses_foreign_publication_pins_after_restart() {
        for pin in 0..7 {
            recovery_fixture(Some(pin));
        }
    }
    fn recovery_fixture(foreign_pin: Option<usize>) {
        use super::super::{
            Acknowledgment, AdmittedIntent, IntentSelectionEvidence, Mode, Outcome, Role, Root,
            SourceObservation, Turn,
        };
        use crate::storage::{digest, Fault, Kind, Manifest, ObjectRef, Scope, StorePaths, FORMAT};
        struct Fixture(std::path::PathBuf);
        impl Fixture {
            fn paths(&self) -> StorePaths {
                StorePaths {
                    data: self.0.join("data"),
                    cache: self.0.join("cache"),
                    credentials: self.0.join("secrets"),
                }
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        fn reference(envelope: &Envelope) -> IntentRecordRef {
            let bytes = serde_json::to_vec(envelope).unwrap();
            IntentRecordRef {
                namespace: envelope.namespace,
                record_id: envelope.record_id,
                revision_id: envelope.revision_id,
                object: ObjectRef {
                    sha256: digest(&bytes),
                    bytes: bytes.len() as u64,
                },
            }
        }
        let fixture =
            Fixture(std::env::temp_dir().join(format!("buddy-intent-recovery-{}", Uuid::new_v4())));
        let store = Arc::new(Store::open(fixture.paths()).unwrap());
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"document"),
            binding_sha256: digest(b"binding"),
        };
        let handle = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let conversation = Uuid::new_v4();
        let turn_id = Uuid::new_v4();
        let root_value = Root {
            id: conversation,
            next_sequence: 1,
            binding: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let turn_value = Turn {
            id: turn_id,
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
        };
        let mut root = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: conversation,
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::Root(root_value.clone())).unwrap(),
            media_descriptors: vec![],
        };
        let mut turn = root.clone();
        turn.record_id = turn_id;
        turn.revision_id = Uuid::new_v4();
        turn.operation_id = Uuid::new_v4();
        turn.payload = serde_json::to_value(Record::Turn(turn_value.clone())).unwrap();
        let mut objects = BTreeMap::new();
        let mut references = vec![];
        let mut namespaces = BTreeMap::new();
        for envelope in [&root, &turn] {
            let pin = reference(envelope);
            objects.insert(
                pin.object.sha256.clone(),
                serde_json::to_vec(envelope).unwrap(),
            );
            namespaces.insert(pin.object.sha256.clone(), envelope.namespace);
            references.push(pin.object);
        }
        let initial = handle
            .initialize(
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"base"),
                    retained: vec![],
                    selected: Manifest {
                        format: FORMAT,
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::SelectedRecords,
                        records: references,
                        record_namespaces: namespaces,
                        media: vec![],
                        media_coverage: vec![],
                    },
                },
                objects,
            )
            .unwrap();
        let evidence = reference(&root);
        root.parents.insert(root.revision_id);
        root.revision_id = Uuid::new_v4();
        root.operation_id = Uuid::new_v4();
        let mut updated_root = root_value;
        updated_root.updated_ms = 2;
        root.payload = serde_json::to_value(Record::Root(updated_root)).unwrap();
        turn.parents.insert(turn.revision_id);
        turn.revision_id = Uuid::new_v4();
        turn.operation_id = Uuid::new_v4();
        let mut updated_turn = turn_value;
        updated_turn.updated_ms = 2;
        updated_turn.outcome = Outcome::ReconcileRequired;
        turn.payload = serde_json::to_value(Record::Turn(updated_turn)).unwrap();
        let operation = Uuid::new_v4();
        let fingerprint = digest(b"mechanical fixture request; no native qualification");
        let mut intent = AdmittedIntent {
            operation,
            request_fingerprint: fingerprint.clone(),
            conversation,
            turn: turn_id,
            root_revision: reference(&root),
            turn_revision: reference(&turn),
            selection: IntentSelectionEvidence {
                group: scope.group,
                key_sha256: scope.key_sha256.clone(),
                binding_sha256: scope.binding_sha256.clone(),
                store_generation: initial.token.store_generation(),
                aggregate_generation: initial.token.aggregate_generation(),
                accepted_base_sha256: initial.token.accepted_base_sha256().into(),
                selection_sha256: initial.token.selection_sha256().into(),
            },
            source: SourceObservation {
                document: Uuid::new_v4(),
                page: Uuid::new_v4(),
                session: "synthetic".into(),
                visit: "synthetic".into(),
                content_revision: None,
                order_revision: None,
                capability_revision: "synthetic".into(),
                evidence_procedure: "mechanical-only".into(),
            },
            evidence: vec![evidence],
            media: vec![],
        };
        if let Some(pin) = foreign_pin {
            match pin {
                0 => intent.selection.group = Uuid::new_v4(),
                1 => intent.selection.key_sha256 = digest(b"foreign key"),
                2 => intent.selection.binding_sha256 = digest(b"foreign binding"),
                3 => intent.selection.store_generation = Uuid::new_v4(),
                4 => intent.selection.accepted_base_sha256 = digest(b"foreign base"),
                5 => intent.selection.selection_sha256 = digest(b"foreign predecessor"),
                _ => intent.selection.aggregate_generation = Uuid::new_v4(),
            }
        }
        let mut receipt = root.clone();
        receipt.record_id = Ledger::receipt_id(operation);
        receipt.revision_id = Uuid::new_v4();
        receipt.operation_id = Uuid::new_v4();
        receipt.parents.clear();
        receipt.domain_schema_version = 2;
        receipt.payload = serde_json::to_value(Record::Receipt(Receipt {
            fingerprint,
            acknowledgment: Acknowledgment {
                operation,
                conversation,
                records: vec![turn_id],
                applied_root_revision: root.revision_id,
                binding: None,
            },
            admitted_intent: Some(intent),
        }))
        .unwrap();
        let saved = reference(&receipt);
        let saved_bytes = serde_json::to_vec(&receipt).unwrap();
        store.set_fault(Fault::AfterCommit).unwrap();
        assert!(store
            .commit_selected(
                &initial.token,
                operation,
                vec![root, turn, receipt],
                BTreeMap::new()
            )
            .is_err());
        drop(handle);
        drop(store);
        let store = Arc::new(Store::open(fixture.paths()).unwrap());
        let handle = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        if foreign_pin.is_some() {
            assert!(handle.recover_original_intent(operation).is_err());
            return;
        }
        let recovered = handle.recover_original_intent(operation).unwrap().unwrap();
        assert_eq!(recovered.receipt_reference, saved);
        assert_eq!(
            recovered.original_objects[&saved.object.sha256],
            saved_bytes
        );
        let snapshot = store.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        let mut change = SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"replacement"),
            selected: snapshot.transaction.selected.clone(),
            retained: vec![],
        };
        change.selected.transaction_id = Uuid::new_v4();
        handle
            .activate(&snapshot.token, change, BTreeMap::new())
            .unwrap();
        let historical = handle.recover_original_intent(operation).unwrap().unwrap();
        assert_eq!(historical.receipt_reference, saved);
        assert_eq!(
            historical.transaction.aggregate_generation,
            recovered.transaction.aggregate_generation
        );
        assert_ne!(
            store
                .selected_snapshot(&scope, MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token
                .aggregate_generation(),
            historical.transaction.aggregate_generation
        );
    }
}
