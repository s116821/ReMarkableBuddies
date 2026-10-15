use remarkable_reader_buddy::{
    memory::*,
    storage::{self, Store, StorePaths, Uuid},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-memory-{}", Uuid::new_v4())))
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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fact(profile: Uuid, scope: Scope, claim: &str, value: &str) -> Fact {
    Fact {
        key: Key {
            profile,
            scope,
            fact: Uuid::new_v4(),
        },
        claim: canonical_claim(claim).unwrap(),
        value: value.into(),
        subject: "physics".into(),
        tags: vec!["gravity".into()],
        provenance: Provenance {
            basis: Basis::UserConfirmed,
            description: "Explicit test author assertion, not a native observation".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 10,
        expires_ms: None,
    }
}
fn put(memory: &LocalMemory, fact: Fact, parents: BTreeSet<Uuid>, operation: Uuid) -> Uuid {
    memory
        .put(Mutation {
            operation,
            expected: parents,
            fact,
        })
        .unwrap()
}
fn heads(memory: &LocalMemory, key: &Key) -> BTreeSet<Uuid> {
    memory
        .inspect(key)
        .unwrap()
        .iter()
        .map(|e| e.revision_id)
        .collect()
}
fn query() -> Query {
    Query::new("physics gravity units".into(), 100)
}

#[test]
fn offline_restart_unicode_exact_retry_edit_and_tombstone_do_not_resurrect() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let store = fixture.store();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let original = fact(
        profile,
        Scope::Global,
        "  Units\tPreference ",
        "Use SI units.\nα, β and °C are preserved.",
    );
    let operation = Uuid::new_v4();
    let receipt = put(&memory, original.clone(), BTreeSet::new(), operation);
    assert_eq!(
        receipt,
        put(&memory, original.clone(), BTreeSet::new(), operation)
    );
    let mut changed = original.clone();
    changed.value = "Use gravity SI units with kelvin.".into();
    assert!(memory
        .put(Mutation {
            operation,
            expected: BTreeSet::new(),
            fact: changed.clone()
        })
        .is_err());
    let parents = heads(&memory, &original.key);
    put(&memory, changed.clone(), parents, Uuid::new_v4());
    assert!(memory
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: changed.clone()
        })
        .is_err());
    let forget = Forget {
        operation: Uuid::new_v4(),
        expected: heads(&memory, &original.key),
        key: original.key.clone(),
    };
    let deleted = memory.forget(forget.clone()).unwrap();
    assert_eq!(deleted, memory.forget(forget).unwrap());
    assert_eq!(
        memory.query(&query()).unwrap().status,
        Status::CompleteEmpty
    );
    // A retry of the original operation succeeds without reviving its superseded value.
    assert_eq!(
        receipt,
        put(&memory, original.clone(), BTreeSet::new(), operation)
    );
    assert_eq!(
        memory.query(&query()).unwrap().status,
        Status::CompleteEmpty
    );
    assert!(memory
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: changed.clone()
        })
        .is_err());
    drop(memory);
    drop(store);
    let reopened = LocalMemory::new_local(fixture.store(), profile).unwrap();
    assert_eq!(
        reopened.query(&query()).unwrap().status,
        Status::CompleteEmpty
    );
    put(
        &reopened,
        changed.clone(),
        heads(&reopened, &changed.key),
        Uuid::new_v4(),
    );
    let result = reopened.query(&query()).unwrap();
    assert_eq!(result.status, Status::Complete);
    assert_eq!(result.context.facts[0].fact.value, changed.value);
    let mut wrong_claim = changed;
    wrong_claim.claim = "changed identity".into();
    assert!(reopened
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: heads(&reopened, &wrong_claim.key),
            fact: wrong_claim
        })
        .is_err());
}

#[test]
fn profile_and_document_scope_never_borrow_other_records_and_known_facts_suppress() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let profile = Uuid::new_v4();
    let document = Uuid::new_v4();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let global = fact(
        profile,
        Scope::Global,
        "gravity units",
        "Global physics SI units.",
    );
    let scoped = fact(
        profile,
        Scope::Document { document },
        "document units",
        "Document gravity uses cgs units.",
    );
    let unrelated = fact(
        profile,
        Scope::Document {
            document: Uuid::new_v4(),
        },
        "other units",
        "Other physics gravity units.",
    );
    for value in [global.clone(), scoped.clone(), unrelated] {
        put(&memory, value, BTreeSet::new(), Uuid::new_v4());
    }
    let other_profile = Uuid::new_v4();
    let foreign = LocalMemory::new_local(store, other_profile).unwrap();
    put(
        &foreign,
        fact(
            other_profile,
            Scope::Global,
            "private units",
            "Other user's gravity units.",
        ),
        BTreeSet::new(),
        Uuid::new_v4(),
    );
    let result = memory.query(&query()).unwrap();
    assert_eq!(result.context.facts.len(), 1);
    assert_eq!(result.context.facts[0].fact, global);
    let mut selected = query();
    selected.document = Some(document);
    assert_eq!(memory.query(&selected).unwrap().context.facts.len(), 2);
    selected.current_claims = vec![global.claim];
    let result = memory.query(&selected).unwrap();
    assert_eq!(result.context.facts.len(), 1);
    assert_eq!(result.diagnostics.suppressed, 1);
    assert_eq!(result.context.facts[0].fact, scoped);
    assert!(memory
        .inspect(&fact(other_profile, Scope::Global, "x", "x").key)
        .is_err());
}

#[test]
fn cross_scope_contradictions_are_withheld_same_value_duplicates_coalesce() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let document = Uuid::new_v4();
    let memory = LocalMemory::new_local(fixture.store(), profile).unwrap();
    for scope in [Scope::Global, Scope::Document { document }] {
        put(
            &memory,
            fact(profile, scope, "gravity units", "SI physics units"),
            BTreeSet::new(),
            Uuid::new_v4(),
        );
    }
    let mut selected = query();
    selected.document = Some(document);
    assert_eq!(memory.query(&selected).unwrap().context.facts.len(), 1);
    put(
        &memory,
        fact(
            profile,
            Scope::Document { document },
            "gravity units",
            "cgs physics units",
        ),
        BTreeSet::new(),
        Uuid::new_v4(),
    );
    let result = memory.query(&selected).unwrap();
    assert_eq!(result.status, Status::Partial);
    assert!(result.context.facts.is_empty());
    assert_eq!(result.diagnostics.contradicted_claims, 1);
    selected.current_claims = vec!["gravity units".into()];
    assert_eq!(
        memory.query(&selected).unwrap().status,
        Status::CompleteEmpty
    );
}

fn import_fork(store: &Store, original: &storage::Envelope, tombstone: bool) {
    let mut fork = original.clone();
    fork.parents = BTreeSet::from([original.revision_id]);
    fork.revision_id = Uuid::new_v4();
    fork.operation_id = Uuid::new_v4();
    fork.actor_id = Uuid::new_v4();
    if tombstone {
        fork.kind = storage::Kind::Tombstone;
        fork.payload = serde_json::Value::Null;
    } else {
        fork.payload["value"] = serde_json::json!("Concurrent gravity units");
    }
    let bytes = serde_json::to_vec(&fork).unwrap();
    let sha = storage::digest(&bytes);
    let manifest = storage::Manifest {
        format: 1,
        transaction_id: Uuid::new_v4(),
        scope: storage::Scope::LocalTransaction,
        records: vec![storage::ObjectRef {
            sha256: sha.clone(),
            bytes: bytes.len() as u64,
        }],
        record_namespaces: BTreeMap::from([(sha.clone(), storage::Namespace::SubjectMemory)]),
        media: vec![],
        media_coverage: vec![],
    };
    store
        .import(manifest, BTreeMap::from([(sha, bytes)]))
        .unwrap();
}

#[test]
fn concurrent_value_tombstone_heads_withhold_key_without_losing_independent_fact() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let store = fixture.store();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let original = fact(
        profile,
        Scope::Global,
        "gravity units",
        "Original SI physics units.",
    );
    put(&memory, original.clone(), BTreeSet::new(), Uuid::new_v4());
    let first = memory.inspect(&original.key).unwrap().remove(0);
    let mut edit = original.clone();
    edit.value = "Edited gravity physics units.".into();
    put(&memory, edit, heads(&memory, &original.key), Uuid::new_v4());
    import_fork(&store, &first, true);
    put(
        &memory,
        fact(
            profile,
            Scope::Global,
            "independent units",
            "Independent gravity units remain.",
        ),
        BTreeSet::new(),
        Uuid::new_v4(),
    );
    let result = memory.query(&query()).unwrap();
    assert_eq!(result.status, Status::Partial);
    assert_eq!(result.diagnostics.conflicted_keys, 1);
    assert_eq!(result.context.facts.len(), 1);
    assert_eq!(result.context.facts[0].fact.claim, "independent units");
    assert!(memory
        .forget(Forget {
            operation: Uuid::new_v4(),
            expected: BTreeSet::from([first.revision_id]),
            key: original.key.clone()
        })
        .is_err());
    memory
        .forget(Forget {
            operation: Uuid::new_v4(),
            expected: heads(&memory, &original.key),
            key: original.key,
        })
        .unwrap();
    assert_eq!(memory.query(&query()).unwrap().status, Status::Complete);
}

#[test]
fn expiry_boundary_and_encoded_budget_are_explicit_not_zero_memory_success() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let memory = LocalMemory::new_local(fixture.store(), profile).unwrap();
    let mut expired = fact(
        profile,
        Scope::Global,
        "expired units",
        "Physics gravity units old.",
    );
    expired.expires_ms = Some(100);
    put(&memory, expired, BTreeSet::new(), Uuid::new_v4());
    let mut huge = fact(
        profile,
        Scope::Global,
        "gravity units",
        "Physics gravity units.",
    );
    huge.value.push_str(&"\n\t\"\\λ".repeat(450));
    put(&memory, huge, BTreeSet::new(), Uuid::new_v4());
    let mut bounded = query();
    bounded.max_context_bytes = 256;
    let result = memory.query(&bounded).unwrap();
    assert_eq!(result.status, Status::Partial);
    assert_eq!(result.diagnostics.expired, 1);
    assert_eq!(result.diagnostics.budget_omitted, 1);
    assert!(result.context.facts.is_empty());
    assert!(result.encoded().unwrap().len() <= 256);
    bounded.max_context_bytes = MAX_CONTEXT;
    let result = memory.query(&bounded).unwrap();
    assert_eq!(result.status, Status::Complete);
    assert_eq!(result.context.facts.len(), 1);
    assert!(result.encoded().unwrap().len() <= MAX_CONTEXT);
    for change in 0..4 {
        let mut invalid = query();
        match change {
            0 => invalid.max_context_bytes = 255,
            1 => invalid.max_matches = 17,
            2 => invalid.current_claims = vec!["unbounded".into(); 65],
            _ => invalid.text = "x".repeat(2049),
        }
        assert!(memory.query(&invalid).is_err());
    }
}

#[test]
fn unsupported_payload_is_preserved_and_not_silently_queried_as_empty() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let profile = Uuid::new_v4();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let unknown = storage::Envelope {
        envelope_version: 1,
        namespace: storage::Namespace::SubjectMemory,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: storage::Kind::Value,
        payload: serde_json::json!({"future-private-value":"do not echo"}),
        media_descriptors: vec![],
    };
    store
        .commit(vec![unknown.clone()], BTreeMap::new())
        .unwrap();
    let result = memory.query(&query()).unwrap();
    assert_eq!(result.status, Status::Partial);
    assert_eq!(result.diagnostics.unsupported, 1);
    assert!(result.context.facts.is_empty());
    assert!(!result.encoded().unwrap().contains("do not echo"));
    assert!(
        store
            .heads(storage::Namespace::SubjectMemory, unknown.record_id)
            .unwrap()[0]
            == unknown
    );
}

#[test]
fn global_metadata_limit_reports_unavailable_before_parsing_or_echoing_large_unknown_data() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let profile = Uuid::new_v4();
    for _ in 0..3 {
        let records = (0..3).map(|_| storage::Envelope {
            envelope_version: 1, namespace: storage::Namespace::SubjectMemory, domain_schema_version: 1,
            record_id: Uuid::new_v4(), revision_id: Uuid::new_v4(), parents: BTreeSet::new(), operation_id: Uuid::new_v4(), actor_id: store.actor_id,
            kind: storage::Kind::Value, payload: serde_json::json!({"large-private-future-payload":"x".repeat(storage::MAX_RECORD - 2048)}), media_descriptors: vec![],
        }).collect();
        store.commit(records, BTreeMap::new()).unwrap();
    }
    let memory = LocalMemory::new_local(store, profile).unwrap();
    let result = memory.query(&query()).unwrap();
    assert_eq!(result.status, Status::Unavailable);
    assert!(result.diagnostics.snapshot_unavailable);
    assert!(result.context.facts.is_empty());
    assert!(!result.encoded().unwrap().contains("large-private"));
}

#[test]
fn count_cap_is_explicit_and_ties_are_stable_across_restart() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let store = fixture.store();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    for claim in ["gravity a", "gravity b", "gravity c"] {
        put(
            &memory,
            fact(profile, Scope::Global, claim, "physics gravity units"),
            BTreeSet::new(),
            Uuid::new_v4(),
        );
    }
    let mut capped = query();
    capped.max_matches = 1;
    let first = memory.query(&capped).unwrap();
    assert_eq!(first.status, Status::Partial);
    assert_eq!(first.diagnostics.budget_omitted, 2);
    let encoded = first.encoded().unwrap();
    drop(memory);
    drop(store);
    let reopened = LocalMemory::new_local(fixture.store(), profile).unwrap();
    assert_eq!(reopened.query(&capped).unwrap().encoded().unwrap(), encoded);
}

#[test]
fn optional_configuration_is_default_off_and_refuses_unqualified_sync_pairing() {
    let mut config = remarkable_reader_buddy::config::Config::default();
    assert!(!config.memory.enabled);
    config.memory.enabled = true;
    assert!(config.validate_values().is_err());
    config.memory.profile = Some(Uuid::new_v4());
    config.validate_values().unwrap();
    let bytes = serde_json::to_vec(&config).unwrap();
    let decoded: remarkable_reader_buddy::config::Config = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded.memory.profile, config.memory.profile);
    assert!(decoded.memory.enabled);
    config.sync.enabled = true;
    config
        .sync
        .namespaces
        .insert(storage::Namespace::SubjectMemory);
    assert!(config.validate_values().is_err());
    config.memory.enabled = false;
    config.validate_values().unwrap();
    let mut value = fact(
        Uuid::new_v4(),
        Scope::Global,
        "invalid provenance",
        "gravity",
    );
    value.provenance.turn = Some(Uuid::new_v4());
    assert!(value.validate().is_err());
    value.provenance.conversation = Some(Uuid::new_v4());
    value.validate().unwrap();
}

#[test]
fn conflicted_claim_cannot_escape_through_an_independent_same_claim_key() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let profile = Uuid::new_v4();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let original = fact(profile, Scope::Global, "gravity units", "SI gravity units");
    put(&memory, original.clone(), BTreeSet::new(), Uuid::new_v4());
    let prior = memory.inspect(&original.key).unwrap().remove(0);
    let mut changed = original.clone();
    changed.value = "Local gravity units".into();
    put(
        &memory,
        changed,
        heads(&memory, &original.key),
        Uuid::new_v4(),
    );
    import_fork(&store, &prior, false);
    put(
        &memory,
        fact(
            profile,
            Scope::Global,
            "gravity units",
            "Local gravity units",
        ),
        BTreeSet::new(),
        Uuid::new_v4(),
    );
    let result = memory.query(&query()).unwrap();
    assert_eq!(result.status, Status::Partial);
    assert_eq!(result.diagnostics.conflicted_keys, 1);
    assert_eq!(result.diagnostics.contradicted_claims, 1);
    assert!(result.context.facts.is_empty());
}

#[test]
fn same_value_duplicates_score_metadata_before_choosing_stable_representative() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let memory = LocalMemory::new_local(fixture.store(), profile).unwrap();
    let mut first = fact(profile, Scope::Global, "preference", "Use this convention");
    let mut second = first.clone();
    second.key.fact = Uuid::new_v4();
    if first.key.record_id().unwrap() > second.key.record_id().unwrap() {
        std::mem::swap(&mut first, &mut second);
    }
    first.subject = "unrelated".into();
    first.tags.clear();
    second.subject = "spectroscopy".into();
    second.tags.clear();
    put(&memory, first, BTreeSet::new(), Uuid::new_v4());
    put(&memory, second.clone(), BTreeSet::new(), Uuid::new_v4());
    let query = Query::new("spectroscopy".into(), 100);
    let result = memory.query(&query).unwrap();
    assert_eq!(result.status, Status::Complete);
    assert_eq!(result.context.facts.len(), 1);
    assert_eq!(result.context.facts[0].fact.key, second.key);
}

#[test]
fn missing_committed_object_is_unavailable_not_empty_or_permission_to_overwrite() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let store = fixture.store();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let original = fact(profile, Scope::Global, "gravity units", "SI gravity units");
    put(&memory, original.clone(), BTreeSet::new(), Uuid::new_v4());
    let manifest = store.manifests().unwrap().remove(0);
    let reference = &manifest.records[0];
    let object = std::fs::read_dir(store.paths.data.join("generations"))
        .unwrap()
        .map(|e| e.unwrap().path().join("objects").join(&reference.sha256))
        .find(|p| p.exists())
        .unwrap();
    drop(memory);
    drop(store);
    std::fs::remove_file(object).unwrap();
    let reopened_store = fixture.store();
    assert!(reopened_store.unavailable_commits().unwrap() > 0);
    let reopened = LocalMemory::new_local(reopened_store.clone(), profile).unwrap();
    assert_eq!(
        reopened.query(&query()).unwrap().status,
        Status::Unavailable
    );
    assert!(reopened.inspect(&original.key).is_err());
    assert!(reopened
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: original.clone()
        })
        .is_err());
    assert!(reopened
        .forget(Forget {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            key: original.key
        })
        .is_err());
    assert_eq!(reopened_store.manifests().unwrap().len(), 0);
}

#[test]
fn damaged_newer_tombstone_does_not_turn_recovered_original_into_current_memory() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let store = fixture.store();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    let original = fact(
        profile,
        Scope::Global,
        "gravity units",
        "Original gravity units must stay forgotten",
    );
    put(&memory, original.clone(), BTreeSet::new(), Uuid::new_v4());
    let deleted = memory
        .forget(Forget {
            operation: Uuid::new_v4(),
            expected: heads(&memory, &original.key),
            key: original.key.clone(),
        })
        .unwrap();
    let commit = std::fs::read_dir(store.paths.data.join("generations"))
        .unwrap()
        .map(|e| {
            e.unwrap()
                .path()
                .join("commits")
                .join(format!("{deleted}.json"))
        })
        .find(|p| p.exists())
        .unwrap();
    drop(memory);
    drop(store);
    std::fs::write(commit, b"corrupt private newer manifest").unwrap();
    let reopened_store = fixture.store();
    assert!(reopened_store.unavailable_commits().unwrap() > 0);
    // Existing generic recovery exposes the surviving old head; the memory domain refuses it.
    assert_eq!(
        reopened_store
            .heads(
                storage::Namespace::SubjectMemory,
                original.key.record_id().unwrap()
            )
            .unwrap()
            .len(),
        1
    );
    let reopened = LocalMemory::new_local(reopened_store, profile).unwrap();
    let result = reopened.query(&query()).unwrap();
    assert_eq!(result.status, Status::Unavailable);
    assert!(result.context.facts.is_empty());
    assert!(reopened.inspect(&original.key).is_err());
}

#[test]
fn local_adapter_lease_refuses_selected_storage_even_when_sync_flags_are_off() {
    use storage::selection::{SelectionChange, SelectionScope};
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let store = fixture.store();
    let memory = LocalMemory::new_local(store.clone(), profile).unwrap();
    put(
        &memory,
        fact(
            profile,
            Scope::Global,
            "gravity units",
            "Physics gravity units",
        ),
        BTreeSet::new(),
        Uuid::new_v4(),
    );
    let mut selected = store.manifests().unwrap().remove(0);
    selected.scope = storage::Scope::SelectedRecords;
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: storage::digest(b"synthetic-memory-scope"),
        binding_sha256: storage::digest(b"synthetic-binding"),
    };
    let change = SelectionChange {
        operation: Uuid::new_v4(),
        accepted_base_sha256: storage::digest(b"synthetic-accepted-base"),
        selected,
        retained: vec![],
    };
    assert!(store
        .initialize_selected(&scope, change.clone(), BTreeMap::new())
        .is_err());
    assert!(store
        .selected_snapshot(&scope, storage::MAX_ITEMS)
        .unwrap()
        .is_none());
    drop(memory);
    store
        .initialize_selected(&scope, change, BTreeMap::new())
        .unwrap();
    assert!(LocalMemory::new_local(store.clone(), profile).is_err());
    drop(store);
    assert!(LocalMemory::new_local(fixture.store(), profile).is_err());
}

#[test]
fn explicit_inspection_refuses_oversized_competing_head_metadata() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let profile = Uuid::new_v4();
    let key = Key {
        profile,
        scope: Scope::Global,
        fact: Uuid::new_v4(),
    };
    for _ in 0..3 {
        let mut objects = BTreeMap::new();
        let mut references = vec![];
        let mut namespaces = BTreeMap::new();
        for _ in 0..3 {
            let envelope = storage::Envelope {
                envelope_version: 1,
                namespace: storage::Namespace::SubjectMemory,
                domain_schema_version: 1,
                record_id: key.record_id().unwrap(),
                revision_id: Uuid::new_v4(),
                parents: BTreeSet::new(),
                operation_id: Uuid::new_v4(),
                actor_id: Uuid::new_v4(),
                kind: storage::Kind::Value,
                payload: serde_json::json!({"future-private-data":"x".repeat(storage::MAX_RECORD - 2048)}),
                media_descriptors: vec![],
            };
            let bytes = serde_json::to_vec(&envelope).unwrap();
            let hash = storage::digest(&bytes);
            references.push(storage::ObjectRef {
                sha256: hash.clone(),
                bytes: bytes.len() as u64,
            });
            namespaces.insert(hash.clone(), storage::Namespace::SubjectMemory);
            objects.insert(hash, bytes);
        }
        store
            .import(
                storage::Manifest {
                    format: 1,
                    transaction_id: Uuid::new_v4(),
                    scope: storage::Scope::LocalTransaction,
                    records: references,
                    record_namespaces: namespaces,
                    media: vec![],
                    media_coverage: vec![],
                },
                objects,
            )
            .unwrap();
    }
    let memory = LocalMemory::new_local(store, profile).unwrap();
    assert!(memory.inspect(&key).is_err());
}
