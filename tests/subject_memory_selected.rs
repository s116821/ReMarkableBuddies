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
fn altered_query_or_inspection_payload_cannot_use_the_original_pins() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let memory = selected::SelectedMemory::new(store, id.clone()).unwrap();
    let first = fact(id.profile, Scope::Global);
    register(&memory, initialize(&memory, first.clone()));
    let original = memory.query(&Query::new("gravity".into(), 10)).unwrap();
    for mode in 0..6 {
        let mut altered = original.clone();
        match mode {
            0 => altered.context.facts[0].fact.value = "Forged assertion".into(),
            1 => altered.context.facts[0].fact.key.profile = Uuid::new_v4(),
            2 => altered.context.facts[0].fact.key.fact = Uuid::new_v4(),
            3 => altered.context.facts[0].fact.confidence_basis_points = 1,
            4 => altered.status = Status::Partial,
            _ => altered.diagnostics.expired = 100,
        }
        let mut consumed = false;
        assert!(memory
            .with_current(&altered, &mut || {
                consumed = true;
                Ok(())
            })
            .is_err());
        assert!(!consumed);
    }
    let unchanged = original.clone();
    let mut count = 0;
    memory
        .with_current(&unchanged, &mut || {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert!(memory
        .with_current(&original, &mut || {
            count += 1;
            Ok(())
        })
        .is_err());
    assert_eq!(count, 1);
    let mut inspected = memory.inspect_for_handoff(&first.key).unwrap();
    inspected.envelopes[0].payload["value"] = serde_json::json!("Forged inspection value");
    let mut consumed = false;
    assert!(inspected
        .with_current(&memory, &mut || {
            consumed = true;
            Ok(())
        })
        .is_err());
    assert!(!consumed);
}

#[test]
fn a_later_root_cannot_repair_a_rootless_accepted_catalog_initializer() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = identity();
    let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
    let registration = initialize(&memory, fact(id.profile, Scope::Global));
    let record = catalog::Record::registration(registration)
        .envelope(&id, store.actor_id, Uuid::new_v4())
        .unwrap();
    let bytes = serde_json::to_vec(&record).unwrap();
    let sha256 = storage::digest(&bytes);
    let manifest = storage::Manifest {
        format: 1,
        transaction_id: Uuid::new_v4(),
        scope: storage::Scope::SelectedRecords,
        records: vec![storage::ObjectRef {
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        }],
        record_namespaces: BTreeMap::from([(sha256.clone(), storage::Namespace::SubjectMemory)]),
        media: vec![],
        media_coverage: vec![],
    };
    let publication = store
        .initialize_selected(
            &id.scope().unwrap(),
            storage::selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"rootless catalog fixture"),
                selected: manifest,
                retained: vec![],
            },
            BTreeMap::from([(sha256, bytes)]),
        )
        .unwrap();
    let root = catalog::Record::root(&id)
        .unwrap()
        .envelope(&id, store.actor_id, Uuid::new_v4())
        .unwrap();
    store
        .commit_selected(
            &publication.token,
            Uuid::new_v4(),
            vec![root],
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(
        memory
            .query(&Query::new("gravity".into(), 10))
            .unwrap()
            .status,
        Status::Unavailable
    );
}

#[test]
fn accepted_claim_custody_survives_parentless_replacement_with_or_without_retained_copies() {
    for (retained, changed_claim) in [(false, true), (true, true), (true, false)] {
        let fixture = Fixture::new();
        let store = fixture.store();
        let id = identity();
        let document = Uuid::new_v4();
        let memory = selected::SelectedMemory::new(store.clone(), id.clone()).unwrap();
        let seed = fact(id.profile, Scope::Document { document });
        let registration = initialize(&memory, seed.clone());
        register(&memory, registration.clone());
        let mut later = fact(id.profile, Scope::Document { document });
        later.claim = "gravity alternative".into();
        later.value = "Original gravity alternative".into();
        memory
            .put(Mutation {
                operation: Uuid::new_v4(),
                expected: Default::default(),
                fact: later.clone(),
            })
            .unwrap();
        let scope = id.aggregate_scope(&registration.owner).unwrap();
        let before = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
        let mut winner = later.clone();
        winner.value = "Winning gravity alternative".into();
        if changed_claim {
            winner.claim = "changed gravity claim".into();
        }
        let mut replacement = envelope(
            &store,
            storage::Namespace::SubjectMemory,
            later.key.record_id().unwrap(),
            serde_json::to_value(winner).unwrap(),
        );
        replacement.revision_id = Uuid::new_v5(
            &replacement.operation_id,
            b"remarkable-buddies:subject-memory:revision:v1",
        );
        let bytes = serde_json::to_vec(&replacement).unwrap();
        let sha256 = storage::digest(&bytes);
        let mut manifest = before.transaction.selected.clone();
        manifest.transaction_id = Uuid::new_v4();
        manifest.records = before
            .selected_records
            .iter()
            .zip(&before.transaction.selected.records)
            .filter(|(e, _)| e.record_id != later.key.record_id().unwrap())
            .map(|(_, r)| r.clone())
            .collect();
        manifest
            .record_namespaces
            .retain(|hash, _| manifest.records.iter().any(|r| &r.sha256 == hash));
        manifest.records.push(storage::ObjectRef {
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        });
        manifest
            .record_namespaces
            .insert(sha256.clone(), storage::Namespace::SubjectMemory);
        let admission =
            remarkable_reader_buddy::conversation::SelectedAdmission::new(store.clone(), scope)
                .unwrap();
        admission
            .activate(
                &before.token,
                storage::selection::SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: storage::digest(b"explicit local winning fork"),
                    selected: manifest,
                    retained: if retained {
                        vec![before.transaction.selected]
                    } else {
                        vec![]
                    },
                },
                BTreeMap::from([(sha256, bytes)]),
            )
            .unwrap();
        let mut query = Query::new("gravity".into(), 10);
        query.document = Some(document);
        let result = memory.query(&query).unwrap();
        if changed_claim {
            assert_eq!(result.status, Status::Unavailable);
            assert!(result.context.facts.is_empty());
            assert!(memory
                .put(Mutation {
                    operation: Uuid::new_v4(),
                    expected: BTreeSet::from([replacement.revision_id]),
                    fact: later
                })
                .is_err());
        } else {
            assert_eq!(result.status, Status::Complete);
            assert!(result
                .context
                .facts
                .iter()
                .any(|item| item.fact.key == later.key
                    && item.fact.value == "Winning gravity alternative"));
        }
    }
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
            next_sequence: 1,
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
                acquisition: Default::default(),
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
    let Record::Binding(binding_record) = serde_json::from_value(bound.payload.clone()).unwrap()
    else {
        unreachable!()
    };
    let turn_id = Uuid::new_v4();
    let mut source_ids = Vec::new();
    let mut source_records = Vec::new();
    let mut media_objects = BTreeMap::new();
    let mut media_refs = Vec::new();
    let mut coverage = Vec::new();
    for (ordinal, color) in [[255u8, 0, 0], [0, 0, 255], [0, 255, 0]]
        .into_iter()
        .enumerate()
    {
        let image = image::RgbImage::from_pixel(1, 1, image::Rgb(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let bytes = bytes.into_inner();
        let media = storage::Media {
            sha256: storage::digest(&bytes),
            bytes: bytes.len() as u64,
            media_type: "image/png".into(),
        };
        let source_id = Uuid::new_v4();
        source_ids.push(source_id);
        let source = remarkable_reader_buddy::conversation::ImageUse {
            id: source_id,
            conversation,
            turn: turn_id,
            image: media.clone(),
            observation: binding_record.receipt.source.clone(),
            width: 1,
            height: 1,
            purpose: "historical-fixture".into(),
            ordinal: ordinal as u32,
            parent: None,
            parent_dimensions: None,
            crop: None,
            transform: [1., 0., 0., 1., 0., 0.],
            target: None,
            captured_ms: 1,
        };
        let mut record = envelope(
            &store,
            storage::Namespace::Source,
            source_id,
            serde_json::to_value(Record::Source(source)).unwrap(),
        );
        record.media_descriptors = vec![media.clone()];
        source_records.push(record);
        if ordinal == 2 {
            coverage.push(storage::Coverage::Omitted {
                sha256: media.sha256,
                reason: storage::Omission::PolicyDisabled,
            });
        } else {
            media_refs.push(storage::ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            });
            coverage.push(storage::Coverage::Included {
                sha256: media.sha256.clone(),
            });
            media_objects.insert(media.sha256, bytes);
        }
    }
    let turn = envelope(
        &store,
        storage::Namespace::Conversation,
        turn_id,
        serde_json::to_value(Record::Turn(remarkable_reader_buddy::conversation::Turn {
            id: turn_id,
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: remarkable_reader_buddy::conversation::Role::User,
            mode: remarkable_reader_buddy::conversation::Mode::Reader,
            outcome: remarkable_reader_buddy::conversation::Outcome::Interpreted,
            text: Some("Historical fixture question".into()),
            sources: source_ids,
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        }))
        .unwrap(),
    );
    let mut manifest = before.transaction.selected.clone();
    manifest.transaction_id = Uuid::new_v4();
    media_refs.sort_by(|a, b| b.sha256.cmp(&a.sha256));
    coverage.sort_by(|a, b| b.hash().cmp(a.hash()));
    manifest.media = media_refs.clone();
    manifest.media_coverage = coverage.clone();
    let mut objects = media_objects;
    let mut exact = Vec::new();
    let mut records = vec![root, bound, foreign, turn];
    records.extend(source_records);
    for record in records {
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
    let mutation = Mutation {
        operation: Uuid::new_v4(),
        expected: parents(&memory, &first.key),
        fact: corrected,
    };
    let receipt = memory.put(mutation.clone()).unwrap();
    let after = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    for reference in exact {
        assert!(after.transaction.selected.records.contains(&reference));
    }
    assert_ne!(after.transaction.selected.media, media_refs);
    memory
        .forget(Forget {
            operation: Uuid::new_v4(),
            expected: parents(&memory, &first.key),
            key: first.key.clone(),
        })
        .unwrap();
    let tombstone = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    assert_eq!(memory.put(mutation).unwrap(), receipt);
    assert_eq!(
        store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token,
        tombstone.token
    );
    assert_eq!(
        tombstone
            .transaction
            .selected
            .media_coverage
            .iter()
            .map(|item| (item.hash(), item))
            .collect::<BTreeMap<_, _>>(),
        coverage
            .iter()
            .map(|item| (item.hash(), item))
            .collect::<BTreeMap<_, _>>()
    );
    let unsupported = envelope(
        &store,
        storage::Namespace::Handwriting,
        Uuid::new_v4(),
        serde_json::json!({"unknown":true}),
    );
    admission
        .with_current(&tombstone.token, |_| {
            store.commit_selected(
                &tombstone.token,
                Uuid::new_v4(),
                vec![unsupported],
                BTreeMap::new(),
            )
        })
        .unwrap();
    assert_eq!(memory.query(&query).unwrap().status, Status::Unavailable);
}
