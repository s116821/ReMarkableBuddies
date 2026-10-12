//! Real selected Store/domain controls. UUID associations are historical fixtures only.
use remarkable_reader_buddy::{
    memory::*,
    storage::{self, Envelope, Store, StorePaths, Uuid},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-selected-memory-{}", Uuid::new_v4())))
    }
    fn store(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn identity() -> catalog::Identity {
    catalog::Identity {
        group: Uuid::new_v4(),
        profile: Uuid::new_v4(),
        binding_sha256: storage::digest(b"local fixture binding"),
    }
}
fn fact(profile: Uuid, scope: Scope) -> Fact {
    Fact {
        key: Key {
            profile,
            scope,
            fact: Uuid::new_v4(),
        },
        claim: "gravity convention".into(),
        value: "SI gravity units".into(),
        subject: "gravity".into(),
        tags: vec![],
        provenance: Provenance {
            basis: Basis::UserConfirmed,
            description: "Explicit historical fixture".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 1,
        expires_ms: None,
    }
}
fn initialize(memory: &selected::SelectedMemory, fact: Fact) -> catalog::Registration {
    memory
        .initialize_aggregate(selected::InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"aggregate descriptor"),
            fact,
        })
        .unwrap()
        .registration
        .unwrap()
}
fn register(memory: &selected::SelectedMemory, registration: catalog::Registration) {
    memory
        .initialize_selection(selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"catalog descriptor"),
            registrations: vec![registration],
        })
        .unwrap();
}
fn parents(memory: &selected::SelectedMemory, key: &Key) -> BTreeSet<Uuid> {
    memory
        .inspect(key)
        .unwrap()
        .into_iter()
        .map(|e| e.revision_id)
        .collect()
}

#[test]
fn stale_result_cannot_consume_and_exact_retry_after_tombstone_keeps_original_receipt() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
    let first = fact(id.profile, Scope::Global);
    let registration = initialize(&memory, first.clone());
    register(&memory, registration.clone());
    let old = memory.query(&Query::new("gravity".into(), 10)).unwrap();
    let mut changed = first.clone();
    changed.value = "Corrected gravity convention".into();
    let mutation = Mutation {
        operation: Uuid::new_v4(),
        expected: parents(&memory, &first.key),
        fact: changed,
    };
    let receipt = memory.put(mutation.clone()).unwrap();
    let mut consumed = false;
    assert!(memory
        .with_current(&old, &mut || {
            consumed = true;
            Ok(())
        })
        .is_err());
    assert!(!consumed);
    memory
        .forget(Forget {
            operation: Uuid::new_v4(),
            expected: parents(&memory, &first.key),
            key: first.key.clone(),
        })
        .unwrap();
    let scope = id.aggregate_scope(&registration.owner).unwrap();
    let before = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    assert_eq!(memory.put(mutation.clone()).unwrap(), receipt);
    let after = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    assert_eq!(before.token, after.token);
    assert_eq!(before.transaction.selected, after.transaction.selected);
    assert_eq!(
        memory
            .query(&Query::new("gravity".into(), 10))
            .unwrap()
            .status,
        Status::CompleteEmpty
    );
    let mut collision = mutation;
    collision.expected.clear();
    assert!(memory.put(collision).is_err());
}

#[test]
fn normalized_token_does_not_hide_changed_raw_genesis_at_consumption() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
    let first = fact(id.profile, Scope::Global);
    let registration = initialize(&memory, first);
    register(&memory, registration.clone());
    let view = memory.query(&Query::new("gravity".into(), 10)).unwrap();
    assert_eq!(view.status, Status::Complete);
    let scope = id.aggregate_scope(&registration.owner).unwrap();
    let before = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    let file = store
        .paths
        .data
        .join("generations")
        .join(before.token.store_generation().to_string())
        .join("selections")
        .join(format!(
            "{}.json",
            storage::digest(&serde_json::to_vec(&scope).unwrap())
        ));
    let original = fs::read(&file).unwrap();
    let pretty = serde_json::to_vec_pretty(&before.transaction).unwrap();
    assert_ne!(original, pretty);
    fs::write(&file, pretty).unwrap();
    assert_eq!(
        store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token,
        before.token
    );
    let mut consumed = false;
    assert!(memory
        .with_current(&view, &mut || {
            consumed = true;
            Ok(())
        })
        .is_err());
    assert!(!consumed);
    assert_eq!(
        memory
            .query(&Query::new("gravity".into(), 10))
            .unwrap()
            .status,
        Status::Unavailable
    );
    fs::write(&file, original).unwrap();
    assert_eq!(
        memory
            .query(&Query::new("gravity".into(), 10))
            .unwrap()
            .status,
        Status::Complete
    );
}

#[test]
fn receiving_revalidation_shares_the_original_budget_and_never_consumes_after_exhaustion() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
    let mut registrations = Vec::new();
    for index in 0..32 {
        let mut value = fact(id.profile, Scope::Global);
        value.claim = format!("gravity convention {index}");
        registrations.push(initialize(&memory, value));
    }
    memory
        .initialize_selection(selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"explicit catalogue"),
            registrations,
        })
        .unwrap();
    let view = memory.query(&Query::new("gravity".into(), 10)).unwrap();
    assert!(!view.context.facts.is_empty());
    let mut consumed = false;
    assert!(memory
        .with_current(&view, &mut || {
            consumed = true;
            Ok(())
        })
        .is_err());
    assert!(!consumed);
    // Reusing the same result cannot reset its request budget or mint authority.
    assert!(memory
        .with_current(&view, &mut || {
            consumed = true;
            Ok(())
        })
        .is_err());
    assert!(!consumed);
}

#[test]
fn catalog_activation_cannot_move_accepted_registration_into_retained_only() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
    let registration = initialize(&memory, fact(id.profile, Scope::Global));
    register(&memory, registration);
    let before = store
        .selected_snapshot(&id.scope().unwrap(), 4096)
        .unwrap()
        .unwrap();
    let root = before
        .selected_records
        .iter()
        .position(|e| e.record_id == id.record_id().unwrap())
        .unwrap();
    let reference = before.transaction.selected.records[root].clone();
    let mut selected = before.transaction.selected.clone();
    selected.transaction_id = Uuid::new_v4();
    selected.records = vec![reference.clone()];
    selected.record_namespaces =
        BTreeMap::from([(reference.sha256, storage::Namespace::SubjectMemory)]);
    let admission = remarkable_reader_buddy::conversation::SelectedAdmission::new(
        store.clone(),
        id.scope().unwrap(),
    )
    .unwrap();
    admission
        .activate(
            &before.token,
            storage::selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"replacement"),
                selected,
                retained: vec![before.transaction.selected],
            },
            BTreeMap::new(),
        )
        .unwrap();
    let result = memory.query(&Query::new("gravity".into(), 10)).unwrap();
    assert_eq!(result.status, Status::Unavailable);
    assert!(result.context.facts.is_empty());
}

fn envelope(
    store: &Store,
    namespace: storage::Namespace,
    id: Uuid,
    payload: serde_json::Value,
) -> Envelope {
    Envelope {
        envelope_version: 1,
        namespace,
        domain_schema_version: 1,
        record_id: id,
        revision_id: Uuid::new_v4(),
        parents: Default::default(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: storage::Kind::Value,
        payload,
        media_descriptors: vec![],
    }
}

#[test]
fn mixed_document_edit_preserves_exact_other_profile_and_noncanonical_conversation_references() {
    use remarkable_reader_buddy::conversation::{
        Binding, BindingReceipt, Ledger, Record, Root, SourceObservation,
    };
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let document = Uuid::new_v4();
    let page = Uuid::new_v4();
    let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
    let first = fact(id.profile, Scope::Document { document });
    let registration = initialize(&memory, first.clone());
    let scope = id.aggregate_scope(&registration.owner).unwrap();
    let before = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    let conversation = Uuid::new_v4();
    let binding = Ledger::binding_id(document, page);
    let root = envelope(
        &store,
        storage::Namespace::Conversation,
        conversation,
        serde_json::to_value(Record::Root(Root {
            id: conversation,
            next_sequence: 0,
            binding: Some(binding),
            created_ms: 1,
            updated_ms: 1,
        }))
        .unwrap(),
    );
    let bound = envelope(
        &store,
        storage::Namespace::Conversation,
        binding,
        serde_json::to_value(Record::Binding(Binding {
            id: binding,
            conversation,
            receipt: BindingReceipt {
                operation: Uuid::new_v4(),
                conversation,
                request_fingerprint: "historical-fixture-request".into(),
                adapter_fingerprint: "historical-fixture-adapter".into(),
                source: SourceObservation {
                    document,
                    page,
                    session: "synthetic".into(),
                    visit: "synthetic".into(),
                    content_revision: None,
                    order_revision: None,
                    capability_revision: "fixture".into(),
                    evidence_procedure: "historical-fixture-only".into(),
                },
                intended_target: None,
                observed_document: document,
                observed_page: page,
                expected_order: vec![page],
                observed_order: vec![page],
                persisted_revision: "fixture".into(),
                observed_ms: 1,
                evidence_origin: "synthetic".into(),
            },
        }))
        .unwrap(),
    );
    let other = fact(Uuid::new_v4(), Scope::Document { document });
    let mut foreign = envelope(
        &store,
        storage::Namespace::SubjectMemory,
        other.key.record_id().unwrap(),
        serde_json::to_value(&other).unwrap(),
    );
    foreign.revision_id = Uuid::new_v5(
        &foreign.operation_id,
        b"remarkable-buddies:subject-memory:revision:v1",
    );
    let mut manifest = before.transaction.selected.clone();
    manifest.transaction_id = Uuid::new_v4();
    let mut objects = BTreeMap::new();
    let mut exact = Vec::new();
    for record in [root, bound, foreign] {
        let bytes = serde_json::to_vec_pretty(&record).unwrap();
        let sha256 = storage::digest(&bytes);
        let reference = storage::ObjectRef {
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        };
        exact.push(reference.clone());
        manifest.records.push(reference);
        manifest
            .record_namespaces
            .insert(sha256.clone(), record.namespace);
        objects.insert(sha256, bytes);
    }
    let admission =
        remarkable_reader_buddy::conversation::SelectedAdmission::new(store.clone(), scope.clone())
            .unwrap();
    admission
        .activate(
            &before.token,
            storage::selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"mixed fixture"),
                selected: manifest,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    register(&memory, registration);
    let mut query = Query::new("gravity".into(), 10);
    query.document = Some(document);
    let selected = memory.query(&query).unwrap();
    assert_eq!(selected.status, Status::Complete);
    assert_eq!(selected.context.facts.len(), 1);
    assert_eq!(selected.context.facts[0].fact.key.profile, id.profile);
    let mut corrected = first.clone();
    corrected.value = "Corrected gravity SI units".into();
    memory
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: parents(&memory, &first.key),
            fact: corrected,
        })
        .unwrap();
    let after = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    for reference in exact {
        assert!(after.transaction.selected.records.contains(&reference));
    }
    let unsupported = envelope(
        &store,
        storage::Namespace::Handwriting,
        Uuid::new_v4(),
        serde_json::json!({"unknown":true}),
    );
    admission
        .with_current(&after.token, |_| {
            store.commit_selected(
                &after.token,
                Uuid::new_v4(),
                vec![unsupported],
                BTreeMap::new(),
            )
        })
        .unwrap();
    assert_eq!(memory.query(&query).unwrap().status, Status::Unavailable);
}
