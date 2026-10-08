use super::*;
use serde_json::json;
use std::sync::{Arc, Barrier};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-selection-write-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("secrets"),
        }
    }
    fn store(&self) -> Store {
        Store::open(self.paths()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn scope() -> SelectionScope {
    SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"owned scope"),
        binding_sha256: digest(b"owned binding"),
    }
}
fn record(store: &Store) -> Envelope {
    Envelope {
        envelope_version: FORMAT,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"fixture":"domain opaque"}),
        media_descriptors: vec![],
    }
}
fn edit(source: &Envelope) -> Envelope {
    let mut next = source.clone();
    next.revision_id = Uuid::new_v4();
    next.operation_id = Uuid::new_v4();
    next.parents = BTreeSet::from([source.revision_id]);
    next.payload = json!({"fixture":"selected edit"});
    next
}
fn closure(
    records: &[Envelope],
    media: BTreeMap<String, Vec<u8>>,
) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut objects = media;
    let mut refs = Vec::new();
    let mut namespaces = BTreeMap::new();
    let mut descriptors = BTreeMap::new();
    for record in records {
        let bytes = serde_json::to_vec(record).unwrap();
        let hash = digest(&bytes);
        refs.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(hash.clone(), record.namespace);
        objects.insert(hash, bytes);
        for d in &record.media_descriptors {
            descriptors.insert(d.sha256.clone(), d.bytes);
        }
    }
    (
        Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: refs,
            record_namespaces: namespaces,
            media: descriptors
                .iter()
                .map(|(h, b)| ObjectRef {
                    sha256: h.clone(),
                    bytes: *b,
                })
                .collect(),
            media_coverage: descriptors
                .keys()
                .map(|h| Coverage::Included { sha256: h.clone() })
                .collect(),
        },
        objects,
    )
}
fn change(records: &[Envelope]) -> (SelectionChange, BTreeMap<String, Vec<u8>>) {
    let (selected, objects) = closure(records, BTreeMap::new());
    (
        SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"accepted descriptor"),
            selected,
            retained: vec![],
        },
        objects,
    )
}
fn initialize(store: &Store, scope: &SelectionScope, records: &[Envelope]) -> SelectionPublication {
    let (change, objects) = change(records);
    store.initialize_selected(scope, change, objects).unwrap()
}
fn selected(store: &Store, scope: &SelectionScope) -> SelectedSnapshot {
    store.selected_snapshot(scope, MAX_ITEMS).unwrap().unwrap()
}

#[test]
fn all_activation_faults_reopen_old_or_complete_new_winner_and_retained_media() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::BeforeActivation,
        Fault::AfterCommit,
        Fault::AfterActivation,
        Fault::None,
    ] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let old = record(&store);
        let original = initialize(&store, &s, std::slice::from_ref(&old));
        let winner = record(&store);
        let mut intent = record(&store);
        let bytes = b"owned unresolved evidence".to_vec();
        let hash = digest(&bytes);
        intent.media_descriptors.push(Media {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
            media_type: "application/octet-stream".into(),
        });
        let (mut change, mut objects) = change(std::slice::from_ref(&winner));
        let (retained, evidence) = closure(
            std::slice::from_ref(&intent),
            BTreeMap::from([(hash.clone(), bytes.clone())]),
        );
        change.retained.push(retained);
        objects.extend(evidence);
        store.set_fault(fault).unwrap();
        let result = store.activate_selected(&original.token, change.clone(), objects.clone());
        assert_eq!(result.is_ok(), fault == Fault::None);
        drop(store);
        let store = f.store();
        let snapshot = selected(&store, &s);
        let accepted = matches!(
            fault,
            Fault::AfterCommit | Fault::AfterActivation | Fault::None
        );
        assert!(
            snapshot.selected_records
                == if accepted {
                    vec![winner.clone()]
                } else {
                    vec![old.clone()]
                }
        );
        assert!(
            snapshot.retained_records
                == if accepted {
                    vec![intent.clone()]
                } else {
                    vec![]
                }
        );
        if accepted {
            let mut snapshot = snapshot;
            let source = snapshot.media.get_mut(&hash).unwrap();
            assert_eq!(source.chunk(0, source.len()).unwrap(), bytes);
            let replay = store
                .activate_selected(&original.token, change, objects)
                .unwrap();
            assert!(replay.replayed);
            assert_eq!(replay.transaction.operation, snapshot.transaction.operation);
        } else {
            assert!(
                store
                    .heads(winner.namespace, winner.record_id)
                    .unwrap()
                    .is_empty(),
                "orphan staging is not a commit"
            );
            assert!(store
                .activate_selected(&original.token, change, objects)
                .is_ok());
        }
    }
}
#[test]
fn guarded_commit_faults_reopen_once_and_original_replay_never_appends_to_replacement() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::BeforeActivation,
        Fault::AfterCommit,
        Fault::AfterActivation,
        Fault::None,
    ] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let old = record(&store);
        let initial = initialize(&store, &s, std::slice::from_ref(&old));
        let next = edit(&old);
        let operation = Uuid::new_v4();
        store.set_fault(fault).unwrap();
        let result = store.commit_selected(
            &initial.token,
            operation,
            vec![next.clone()],
            BTreeMap::new(),
        );
        assert_eq!(result.is_ok(), fault == Fault::None);
        drop(store);
        let store = f.store();
        let accepted = matches!(
            fault,
            Fault::AfterCommit | Fault::AfterActivation | Fault::None
        );
        assert_eq!(
            selected(&store, &s).selected_records.len(),
            if accepted { 2 } else { 1 }
        );
        let receipt = store
            .commit_selected(
                &initial.token,
                operation,
                vec![next.clone()],
                BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(receipt.replayed, accepted);
        let replacement = record(&store);
        let (change, objects) = change(std::slice::from_ref(&replacement));
        store
            .activate_selected(&receipt.token, change, objects)
            .unwrap();
        let replacement_token = selected(&store, &s).token;
        drop(store);
        let store = f.store();
        let replay = store
            .commit_selected(
                &initial.token,
                operation,
                vec![next.clone()],
                BTreeMap::new(),
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.transaction.operation, operation);
        assert_eq!(replay.token, receipt.token);
        assert!(selected(&store, &s).selected_records == vec![replacement]);
        assert_eq!(selected(&store, &s).token, replacement_token);
        assert!(store
            .commit_selected(
                &replay.token,
                Uuid::new_v4(),
                vec![edit(&next)],
                BTreeMap::new()
            )
            .is_err());
        let mut collision = next;
        collision.payload = json!({"changed":"request"});
        assert!(store
            .commit_selected(&initial.token, operation, vec![collision], BTreeMap::new())
            .is_err());
    }
}
#[test]
fn concurrent_handles_publish_one_winner_and_preserve_unrelated_scopes() {
    let f = Fixture::new();
    let store = Arc::new(f.store());
    let s = scope();
    let other = scope();
    let old = record(&store);
    let unrelated = record(&store);
    let initial = initialize(&store, &s, &[old]);
    let unaffected = initialize(&store, &other, std::slice::from_ref(&unrelated));
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let (change, objects) = change(&[record(&store)]);
        let store = store.clone();
        let barrier = barrier.clone();
        let token = initial.token.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            store.activate_selected(&token, change, objects)
        }));
    }
    barrier.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(selected(&store, &other).token, unaffected.token);
    assert!(selected(&store, &other).selected_records == vec![unrelated]);
    assert!(
        Store::open(f.paths()).is_err(),
        "new independent Store handles must not evade canonical lease"
    );
}
#[test]
fn stale_generation_refuses_even_when_new_winner_has_same_causal_heads() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let old = record(&store);
    let first = initialize(&store, &s, std::slice::from_ref(&old));
    let (change, objects) = change(std::slice::from_ref(&old));
    let second = store
        .activate_selected(&first.token, change, objects)
        .unwrap();
    assert!(store
        .commit_selected(
            &first.token,
            Uuid::new_v4(),
            vec![edit(&old)],
            BTreeMap::new()
        )
        .is_err());
    let mut bad_parent = edit(&old);
    bad_parent.parents.clear();
    assert!(store
        .commit_selected(
            &second.token,
            Uuid::new_v4(),
            vec![bad_parent],
            BTreeMap::new()
        )
        .is_err());
    assert!(store
        .commit_selected(
            &second.token,
            Uuid::new_v4(),
            vec![edit(&old)],
            BTreeMap::new()
        )
        .is_ok());
}
#[test]
fn causal_closure_tombstones_and_all_history_losers_remain_distinct() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = record(&store);
    let first = initialize(&store, &s, std::slice::from_ref(&original));
    let loser = edit(&original);
    store.commit(vec![loser.clone()], BTreeMap::new()).unwrap();
    let next = edit(&original);
    let committed = store
        .commit_selected(
            &first.token,
            Uuid::new_v4(),
            vec![next.clone()],
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(
        store
            .heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        2
    );
    assert!(!selected(&store, &s)
        .selected_records
        .iter()
        .any(|r| r.revision_id == loser.revision_id));
    let mut deletion = edit(&next);
    deletion.kind = Kind::Tombstone;
    deletion.payload = serde_json::Value::Null;
    store
        .commit_selected(
            &committed.token,
            Uuid::new_v4(),
            vec![deletion.clone()],
            BTreeMap::new(),
        )
        .unwrap();
    let current = selected(&store, &s);
    let records: Vec<_> = current
        .selected_records
        .iter()
        .map(|r| (r.clone(), digest(&serde_json::to_vec(r).unwrap())))
        .collect();
    let index = publication::closure_index(&records, true).unwrap();
    assert_eq!(
        index.heads[&(deletion.namespace, deletion.record_id)],
        BTreeSet::from([deletion.revision_id])
    );
}
#[test]
fn required_media_failure_refuses_but_explicit_omission_is_visible() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let initial = initialize(&store, &s, &[record(&store)]);
    let mut winner = record(&store);
    let bytes = b"included fixture".to_vec();
    let hash = digest(&bytes);
    winner.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let (mut update, objects) = change(std::slice::from_ref(&winner));
    assert!(store
        .activate_selected(&initial.token, update.clone(), objects.clone())
        .is_err());
    assert_eq!(selected(&store, &s).token, initial.token);
    update.selected.media.clear();
    update.selected.media_coverage = vec![Coverage::Omitted {
        sha256: hash.clone(),
        reason: Omission::PolicyDisabled,
    }];
    store
        .activate_selected(&initial.token, update, objects)
        .unwrap();
    let snapshot = selected(&store, &s);
    assert!(snapshot.media.is_empty());
    assert!(matches!(
        snapshot.transaction.selected.media_coverage[0],
        Coverage::Omitted { .. }
    ));
    let (mut included, mut objects) = change(&[winner]);
    included.operation = Uuid::new_v4();
    objects.insert(hash.clone(), b"corrupt".to_vec());
    assert!(store
        .activate_selected(&snapshot.token, included, objects)
        .is_err());
}
#[test]
fn pinned_snapshot_and_media_handle_survive_replacement() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let mut original = record(&store);
    let bytes = b"pinned media".to_vec();
    let hash = digest(&bytes);
    original.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let (selected_manifest, objects) = closure(
        std::slice::from_ref(&original),
        BTreeMap::from([(hash.clone(), bytes.clone())]),
    );
    store
        .initialize_selected(
            &s,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected: selected_manifest,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    let mut pinned = selected(&store, &s);
    let (change, objects) = change(&[record(&store)]);
    store
        .activate_selected(&pinned.token, change, objects)
        .unwrap();
    assert!(pinned.selected_records == vec![original]);
    let source = pinned.media.get_mut(&hash).unwrap();
    assert_eq!(source.chunk(0, source.len()).unwrap(), bytes);
}
#[test]
fn incomplete_conflicted_or_unknown_schema_closures_cannot_activate() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = record(&store);
    let first = initialize(&store, &s, std::slice::from_ref(&original));
    for records in [
        vec![edit(&original)],
        vec![original.clone(), edit(&original), edit(&original)],
    ] {
        let (change, objects) = change(&records);
        assert!(store
            .activate_selected(&first.token, change, objects)
            .is_err());
    }
    let mut future = record(&store);
    future.domain_schema_version = 3;
    let (change, objects) = change(&[future]);
    assert!(store
        .activate_selected(&first.token, change, objects)
        .is_err());
    assert_eq!(selected(&store, &s).token, first.token);
}
#[test]
fn accepted_history_corruption_refuses_reopen_and_unreachable_staging_is_ignored() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let initial = initialize(&store, &s, &[record(&store)]);
    let (change, objects) = change(&[record(&store)]);
    store
        .activate_selected(&initial.token, change, objects)
        .unwrap();
    let generation = initial.token.store_generation();
    let history_dir = store.generation(generation).join("selection-history");
    fs::write(history_dir.join("unreachable-orphan"), b"not a receipt").unwrap();
    drop(store);
    let store = f.store();
    assert_eq!(selected(&store, &s).transaction.history_depth, 2);
    drop(store);
    fs::write(
        history_dir.join(initial.token.selection_sha256()),
        b"corrupt accepted history",
    )
    .unwrap();
    assert!(Store::open(f.paths()).is_err());
}
#[test]
fn unsupported_maintenance_never_silently_drops_selected_metadata() {
    use crate::storage::migration::{GenerationMigration, MigrationRegistry};
    struct Migration;
    impl GenerationMigration for Migration {
        fn source_version(&self) -> u32 {
            1
        }
        fn target_version(&self) -> u32 {
            2
        }
        fn transform(&self, _: &Path) -> Result<()> {
            panic!("must refuse before migration")
        }
        fn validate(&self, _: &Path) -> Result<()> {
            Ok(())
        }
    }
    let f = Fixture::new();
    let store = f.store();
    let source = Fixture::new();
    let source_store = source.store();
    source_store
        .commit(vec![record(&source_store)], BTreeMap::new())
        .unwrap();
    let backup = source.0.join("backup");
    source_store.export(&backup).unwrap();
    let s = scope();
    let initial = initialize(&store, &s, &[record(&store)]);
    let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
    assert!(store.export(&f.0.join("refused-backup")).is_err());
    assert!(!f.0.join("refused-backup/backup.json").exists());
    assert!(store.restore(&backup).is_err());
    let mut registry = MigrationRegistry::default();
    registry.register(Migration).unwrap();
    assert!(store.migrate(&registry, 2).is_err());
    assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
    assert_eq!(selected(&store, &s).token, initial.token);
}

#[cfg(unix)]
#[test]
fn inherited_descriptor_does_not_extend_a_dropped_owners_lease() {
    let f = Fixture::new();
    let store = f.store();
    // A duplicate descriptor models the same open-file description inherited in
    // the fork-to-exec window of another test's subprocess spawn.
    let inherited = store._lease.try_clone().unwrap();
    drop(store);
    let reopened = f.store();
    drop(inherited);
    assert!(
        Store::open(f.paths()).is_err(),
        "dropping old descriptor cannot unlock the new owner"
    );
    drop(reopened);
    let _owner = f.store();
}

#[test]
fn selection_crash_child() {
    let Some(root) = std::env::var_os("BUDDY_SELECTION_CRASH_FIXTURE") else {
        return;
    };
    let f = Fixture(PathBuf::from(root));
    let store = f.store();
    let scope: SelectionScope = files::json(&f.0.join("scope.json"), MAX_RECORD as u64).unwrap();
    let snapshot = selected(&store, &scope);
    let fault: String = std::env::var("BUDDY_SELECTION_CRASH_POINT").unwrap();
    let fault = if fault == "before" {
        Fault::BeforeActivation
    } else {
        Fault::AfterActivation
    };
    let encoded = files::read(&f.0.join("winner.json"), MAX_RECORD as u64).unwrap();
    let winner: Envelope = serde_json::from_slice(&encoded).unwrap();
    let (change, objects) = change(&[winner]);
    store.set_fault(fault).unwrap();
    assert!(store
        .activate_selected(&snapshot.token, change, objects)
        .is_err());
    // Exit without running Store/Fixture destructors: actual process death after
    // the injected publication boundary, not a graceful close/reopen simulation.
    std::process::exit(73);
}

#[test]
fn process_death_at_activation_boundary_preserves_one_complete_selection() {
    for point in ["before", "after"] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let original = record(&store);
        let winner = record(&store);
        initialize(&store, &s, std::slice::from_ref(&original));
        files::atomic_json(&f.0.join("scope.json"), &s).unwrap();
        files::atomic_json(&f.0.join("winner.json"), &winner).unwrap();
        drop(store);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::selection::publication_tests::selection_crash_child",
            ])
            .env("BUDDY_SELECTION_CRASH_FIXTURE", &f.0)
            .env("BUDDY_SELECTION_CRASH_POINT", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        let store = f.store();
        assert!(
            selected(&store, &s).selected_records
                == if point == "before" {
                    vec![original]
                } else {
                    vec![winner]
                }
        );
        assert_eq!(store.unavailable_commits().unwrap(), 0);
    }
}

#[test]
fn retained_intent_can_share_immutable_context_without_activating_its_loser_head() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = record(&store);
    let winner = edit(&original);
    let intent = edit(&original);
    let (mut change, mut objects) = change(&[original.clone(), winner.clone()]);
    let (retained, retained_objects) =
        closure(&[original.clone(), intent.clone()], BTreeMap::new());
    change.retained.push(retained);
    objects.extend(retained_objects);
    store.initialize_selected(&s, change, objects).unwrap();
    drop(store);
    let store = f.store();
    let snapshot = selected(&store, &s);
    assert!(snapshot
        .selected_records
        .iter()
        .any(|r| r.revision_id == winner.revision_id));
    assert!(!snapshot
        .selected_records
        .iter()
        .any(|r| r.revision_id == intent.revision_id));
    assert!(snapshot
        .retained_records
        .iter()
        .any(|r| r.revision_id == intent.revision_id));
    assert_eq!(
        store
            .heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn namespace_schema_boundary_survives_commit_import_selection_and_reopen() {
    for namespace in [
        Namespace::Conversation,
        Namespace::Source,
        Namespace::ExportAssociation,
        Namespace::SubjectMemory,
        Namespace::Handwriting,
    ] {
        for version in [1, 2, 3, 99] {
            let f = Fixture::new();
            let store = f.store();
            let mut envelope = record(&store);
            envelope.namespace = namespace;
            envelope.domain_schema_version = version;
            // Deliberately opaque: domain-owner tests validate actual v2 variants.
            let supported = version == 1 || (version == 2 && namespace == Namespace::Conversation);
            let (manifest, objects) = closure(std::slice::from_ref(&envelope), BTreeMap::new());
            let result = store.import(manifest, objects);
            assert_eq!(result.is_ok(), supported);
            let s = scope();
            let (change, objects) = change(std::slice::from_ref(&envelope));
            assert_eq!(
                store.initialize_selected(&s, change, objects).is_ok(),
                supported
            );
            drop(store);
            let store = f.store();
            assert_eq!(store.unavailable_commits().unwrap(), 0);
            if supported {
                assert_eq!(
                    store.heads(namespace, envelope.record_id).unwrap()[0].domain_schema_version,
                    version
                );
                assert_eq!(
                    selected(&store, &s).selected_records[0].domain_schema_version,
                    version
                );
            } else {
                assert!(store
                    .heads(namespace, envelope.record_id)
                    .unwrap()
                    .is_empty());
                assert!(store.selected_snapshot(&s, MAX_ITEMS).unwrap().is_none());
            }
        }
    }
}

#[test]
fn restart_receipt_lookup_recovers_original_evidence_without_a_persisted_live_token() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = record(&store);
    let initial = initialize(&store, &s, std::slice::from_ref(&original));
    let op = Uuid::new_v4();
    let committed = store
        .commit_selected(&initial.token, op, vec![edit(&original)], BTreeMap::new())
        .unwrap();
    let original_generation = committed.transaction.aggregate_generation;
    let (change, objects) = change(&[record(&store)]);
    store
        .activate_selected(&committed.token, change, objects)
        .unwrap();
    drop(committed);
    drop(initial);
    drop(store);
    let store = f.store();
    let current_token = selected(&store, &s).token;
    let receipt = store.selected_receipt(&s, op).unwrap().unwrap();
    assert_eq!(receipt.aggregate_generation, original_generation);
    assert_eq!(receipt.operation, op);
    assert_ne!(
        receipt.aggregate_generation,
        current_token.aggregate_generation()
    );
    assert_eq!(selected(&store, &s).token, current_token);
    assert!(store
        .selected_receipt(&s, Uuid::new_v4())
        .unwrap()
        .is_none());
    assert!(store.selected_receipt(&scope(), op).unwrap().is_none());
}

#[test]
fn history_saturation_refuses_without_discarding_original_receipt() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let mut envelope = record(&store);
    for n in 0u32..2048 {
        envelope.media_descriptors.push(Media {
            sha256: digest(&n.to_be_bytes()),
            bytes: 1,
            media_type: "application/octet-stream".into(),
        });
    }
    let (mut update, objects) = change(&[envelope]);
    update.selected.media.clear();
    update.selected.media_coverage = update
        .selected
        .media_coverage
        .iter()
        .map(|c| Coverage::Omitted {
            sha256: c.hash().into(),
            reason: Omission::PolicyDisabled,
        })
        .collect();
    let initial = store
        .initialize_selected(&s, update.clone(), objects)
        .unwrap();
    let original_operation = update.operation;
    let mut token = initial.token;
    let mut refused = false;
    for _ in 0..100 {
        update.operation = Uuid::new_v4();
        match store.activate_selected(&token, update.clone(), BTreeMap::new()) {
            Ok(publication) => token = publication.token,
            Err(error) => {
                assert!(error.to_string().contains("history exceeds bound"));
                refused = true;
                break;
            }
        }
    }
    assert!(refused);
    assert_eq!(selected(&store, &s).token, token);
    assert!(store
        .selected_receipt(&s, original_operation)
        .unwrap()
        .is_some());
    drop(store);
    let store = f.store();
    assert_eq!(selected(&store, &s).token, token);
    assert!(store
        .selected_receipt(&s, original_operation)
        .unwrap()
        .is_some());
}

#[test]
fn racing_selected_commit_and_activation_do_not_both_accept_one_token() {
    let f = Fixture::new();
    let store = Arc::new(f.store());
    let s = scope();
    let original = record(&store);
    let initial = initialize(&store, &s, std::slice::from_ref(&original));
    let barrier = Arc::new(Barrier::new(3));
    let a = store.clone();
    let a_barrier = barrier.clone();
    let a_token = initial.token.clone();
    let commit = std::thread::spawn(move || {
        a_barrier.wait();
        a.commit_selected(
            &a_token,
            Uuid::new_v4(),
            vec![edit(&original)],
            BTreeMap::new(),
        )
    });
    let a = store.clone();
    let a_barrier = barrier.clone();
    let a_token = initial.token;
    let (change, objects) = change(&[record(&store)]);
    let activate = std::thread::spawn(move || {
        a_barrier.wait();
        a.activate_selected(&a_token, change, objects)
    });
    barrier.wait();
    let results = [commit.join().unwrap(), activate.join().unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(selected(&store, &s).transaction.history_depth, 2);
}

fn retained_fixture(
    store: &Store,
    s: &SelectionScope,
) -> (
    SelectionPublication,
    SelectionPublication,
    Envelope,
    ObjectRef,
) {
    let base = initialize(store, s, &[record(store)]);
    let mut intent = record(store);
    let bytes = b"owned admitted evidence".to_vec();
    let hash = digest(&bytes);
    intent.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let original = store
        .commit_selected(
            &base.token,
            Uuid::new_v4(),
            vec![intent.clone()],
            BTreeMap::from([(hash, bytes)]),
        )
        .unwrap();
    let (mut change, mut objects) = change(&[record(store)]);
    let (retained, evidence) = closure(std::slice::from_ref(&intent), BTreeMap::new());
    let reference = retained.records[0].clone();
    change.retained.push(retained);
    objects.extend(evidence);
    let replacement = store
        .activate_selected(&original.token, change, objects)
        .unwrap();
    (original, replacement, intent, reference)
}
#[test]
fn retained_settlement_faults_preserve_winner_and_original_evidence_atomically() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::BeforeActivation,
        Fault::AfterCommit,
        Fault::AfterActivation,
        Fault::None,
    ] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let (original, replacement, intent, reference) = retained_fixture(&store, &s);
        let mut fact = record(&store);
        fact.domain_schema_version = 2;
        fact.payload = json!({"fixture":"unverified late outcome; no native authority"});
        let (facts, objects) = closure(std::slice::from_ref(&fact), BTreeMap::new());
        let mut retained = replacement.transaction.retained.clone();
        retained.push(facts);
        let update = RetainedCommit {
            operation: Uuid::new_v4(),
            original_operation: original.transaction.operation,
            intent: reference,
            retained,
        };
        store.set_fault(fault).unwrap();
        assert_eq!(
            store
                .commit_retained(&replacement.token, update.clone(), objects.clone())
                .is_ok(),
            fault == Fault::None
        );
        drop(store);
        let store = f.store();
        let snapshot = selected(&store, &s);
        assert_eq!(
            snapshot.transaction.selected.records,
            replacement.transaction.selected.records
        );
        assert_eq!(
            snapshot.transaction.selected.media,
            replacement.transaction.selected.media
        );
        assert!(!snapshot
            .selected_records
            .iter()
            .any(|r| r.record_id == fact.record_id));
        assert!(snapshot
            .retained_records
            .iter()
            .any(|r| r.revision_id == intent.revision_id));
        let accepted = matches!(
            fault,
            Fault::AfterCommit | Fault::AfterActivation | Fault::None
        );
        assert_eq!(
            snapshot
                .retained_records
                .iter()
                .any(|r| r.revision_id == fact.revision_id),
            accepted
        );
        let receipt = store
            .commit_retained(&replacement.token, update.clone(), objects.clone())
            .unwrap();
        assert_eq!(receipt.replayed, accepted);
        assert_eq!(receipt.transaction.mutation, SelectionMutation::Retained);
        let (mut next, mut next_objects) = change(&[record(&store)]);
        next.retained = receipt.transaction.retained.clone();
        // Existing retained objects are immutable and need not be uploaded again.
        next_objects.retain(|h, _| next.selected.records.iter().any(|r| &r.sha256 == h));
        store
            .activate_selected(&receipt.token, next, next_objects)
            .unwrap();
        let token = selected(&store, &s).token;
        assert!(
            store
                .commit_retained(&replacement.token, update, objects)
                .unwrap()
                .replayed
        );
        assert_eq!(selected(&store, &s).token, token);
    }
}
#[test]
fn foreign_active_or_dropped_retained_intent_cannot_settle() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let (original, replacement, _, reference) = retained_fixture(&store, &s);
    let (facts, objects) = closure(&[record(&store)], BTreeMap::new());
    let mut retained = replacement.transaction.retained.clone();
    retained.push(facts);
    let update = RetainedCommit {
        operation: Uuid::new_v4(),
        original_operation: original.transaction.operation,
        intent: reference,
        retained,
    };
    let mut foreign = update.clone();
    foreign.original_operation = Uuid::new_v4();
    assert!(store
        .commit_retained(&replacement.token, foreign, objects.clone())
        .is_err());
    let mut omitted = update.clone();
    omitted.retained.remove(0);
    assert!(store
        .commit_retained(&replacement.token, omitted, objects.clone())
        .is_err());
    let mut foreign = update.clone();
    foreign.intent = replacement.transaction.selected.records[0].clone();
    assert!(store
        .commit_retained(&replacement.token, foreign, objects.clone())
        .is_err());
    let mut lost_media = update.clone();
    lost_media.retained[0].media.clear();
    lost_media.retained[0].media_coverage = lost_media.retained[0]
        .media_coverage
        .iter()
        .map(|c| Coverage::Omitted {
            sha256: c.hash().into(),
            reason: Omission::PolicyDisabled,
        })
        .collect();
    assert!(store
        .commit_retained(&replacement.token, lost_media, objects.clone())
        .is_err());
    let active = selected(&store, &s).selected_records[0].clone();
    let (fork, fork_objects) = closure(&[edit(&active)], BTreeMap::new());
    let mut mutate_active = update.clone();
    mutate_active.retained.push(fork);
    let mut all_objects = objects.clone();
    all_objects.extend(fork_objects);
    assert!(store
        .commit_retained(&replacement.token, mutate_active, all_objects)
        .is_err());
    assert_eq!(selected(&store, &s).token, replacement.token);
    assert!(store
        .commit_retained(&replacement.token, update, objects)
        .is_ok());
}

#[test]
fn selected_generation_feature_rejects_legacy_unknown_or_foreign_headers() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let initial = initialize(&store, &s, &[record(&store)]);
    let root = store.generation(initial.token.store_generation());
    let feature = fs::read(root.join("selection-feature.json")).unwrap();
    assert_eq!(
        files::json::<u32>(&root.join("format.json"), 64).unwrap(),
        SELECTED_STORE_FORMAT
    );
    drop(store);
    for version in [FORMAT, 99] {
        files::atomic_json(&root.join("format.json"), &version).unwrap();
        assert!(Store::open(f.paths()).is_err());
    }
    files::atomic_json(&root.join("format.json"), &SELECTED_STORE_FORMAT).unwrap();
    fs::remove_file(root.join("selection-feature.json")).unwrap();
    assert!(Store::open(f.paths()).is_err());
    for value in [
        json!({"format":1,"actor":Uuid::new_v4(),"generation":initial.token.store_generation()}),
        json!({"format":99,"actor":files::json::<serde_json::Value>(&f.paths().data.join("identity.json"), 1024).unwrap()["actor"],"generation":initial.token.store_generation()}),
    ] {
        files::atomic_json(&root.join("selection-feature.json"), &value).unwrap();
        assert!(Store::open(f.paths()).is_err());
    }
    files::atomic(&root.join("selection-feature.json"), &feature).unwrap();
    assert_eq!(selected(&f.store(), &s).token, initial.token);
}

#[test]
fn selected_reader_qualification_fixture() {
    let Some(root) = std::env::var_os("BUDDY_SELECTED_READER_QUALIFICATION") else {
        return;
    };
    let f = Fixture(PathBuf::from(root));
    let store = f.store();
    let s = scope();
    initialize(&store, &s, &[record(&store)]);
    // The caller owns this disposable fixture and verifies an old reader against
    // it. Preserve only on this explicitly selected fixture path, never user data.
    std::mem::forget(f);
}

#[test]
fn interrupted_feature_preparation_preserves_ordinary_history_and_refuses_unknown_markers() {
    let f = Fixture::new();
    let store = f.store();
    let original = record(&store);
    store
        .commit(vec![original.clone()], BTreeMap::new())
        .unwrap();
    let generation = store.inner.lock().unwrap().generation;
    let actor = store.actor_id;
    let root = store.generation(generation);
    files::atomic_json(
        &root.join("selection-feature.json"),
        &SelectionFeature {
            format: FORMAT,
            actor,
            generation,
        },
    )
    .unwrap();
    drop(store);
    let store = f.store();
    assert!(store.heads(original.namespace, original.record_id).unwrap() == vec![original.clone()]);
    drop(store);
    files::atomic_json(
        &root.join("selection-feature.json"),
        &SelectionFeature {
            format: 99,
            actor,
            generation,
        },
    )
    .unwrap();
    assert!(Store::open(f.paths()).is_err());
    files::atomic_json(
        &root.join("selection-feature.json"),
        &SelectionFeature {
            format: FORMAT,
            actor,
            generation,
        },
    )
    .unwrap();
    files::atomic_json(&root.join("format.json"), &SELECTED_STORE_FORMAT).unwrap();
    let store = f.store();
    assert!(store.heads(original.namespace, original.record_id).unwrap() == vec![original]);
    let s = scope();
    assert!(store.selected_snapshot(&s, MAX_ITEMS).unwrap().is_none());
    initialize(&store, &s, &[record(&store)]);
    assert_eq!(selected(&store, &s).transaction.history_depth, 1);
}
