use super::*;
use serde_json::json;
use std::sync::{Arc, Barrier};

#[path = "outbox_tests.rs"]
mod outbox_tests;
#[path = "portable_tests.rs"]
mod portable_tests;

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
    let inherited = store._lease.0.try_clone().unwrap();
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
    if std::env::var("BUDDY_SELECTION_CRASH_MUTATION").as_deref() == Ok("retained") {
        let (operation, original_operation, intent, retained, objects): (
            Uuid,
            Uuid,
            ObjectRef,
            Vec<Manifest>,
            BTreeMap<String, Vec<u8>>,
        ) = files::json(&f.0.join("retained.json"), MAX_METADATA as u64).unwrap();
        store.set_fault(fault).unwrap();
        assert!(store
            .commit_retained(
                &snapshot.token,
                RetainedCommit {
                    operation,
                    original_operation,
                    intent,
                    retained,
                },
                objects,
            )
            .is_err());
        std::process::exit(73);
    }
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
            let supported = version == 1
                || (version == 2
                    && matches!(
                        namespace,
                        Namespace::Conversation | Namespace::Source | Namespace::ExportAssociation
                    ));
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
fn retained_settlement_process_death_preserves_receipt_media_and_unrelated_scope() {
    for point in ["before", "after"] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let (original, replacement, intent, reference) = retained_fixture(&store, &s);
        let unrelated_scope = scope();
        let unrelated = initialize(&store, &unrelated_scope, &[record(&store)]);
        let mut fact = record(&store);
        fact.domain_schema_version = 2;
        fact.payload = json!({"fixture":"opaque historical settlement"});
        let (facts, objects) = closure(std::slice::from_ref(&fact), BTreeMap::new());
        let mut retained = replacement.transaction.retained.clone();
        retained.push(facts);
        let update = RetainedCommit {
            operation: Uuid::new_v4(),
            original_operation: original.transaction.operation,
            intent: reference,
            retained,
        };
        files::atomic_json(&f.0.join("scope.json"), &s).unwrap();
        files::atomic_json(
            &f.0.join("retained.json"),
            &(
                update.operation,
                update.original_operation,
                &update.intent,
                &update.retained,
                &objects,
            ),
        )
        .unwrap();
        drop(store);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::selection::publication_tests::selection_crash_child",
            ])
            .env("BUDDY_SELECTION_CRASH_FIXTURE", &f.0)
            .env("BUDDY_SELECTION_CRASH_POINT", point)
            .env("BUDDY_SELECTION_CRASH_MUTATION", "retained")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("retained publication child exceeded deadline");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(73));
        let store = f.store();
        let mut snapshot = selected(&store, &s);
        // Each accepted publication has its own manifest transaction ID. Winner
        // membership/coverage must stay identical, not that publication identity.
        assert_eq!(
            snapshot.transaction.selected.records,
            replacement.transaction.selected.records
        );
        assert_eq!(
            snapshot.transaction.selected.record_namespaces,
            replacement.transaction.selected.record_namespaces
        );
        assert_eq!(
            snapshot.transaction.selected.media,
            replacement.transaction.selected.media
        );
        assert_eq!(
            snapshot.transaction.selected.media_coverage,
            replacement.transaction.selected.media_coverage
        );
        assert_eq!(
            snapshot.transaction.accepted_base_sha256,
            replacement.transaction.accepted_base_sha256
        );
        assert!(snapshot
            .retained_records
            .iter()
            .any(|r| r.revision_id == intent.revision_id));
        let media = &intent.media_descriptors[0];
        assert_eq!(
            snapshot
                .media
                .get_mut(&media.sha256)
                .unwrap()
                .chunk(0, media.bytes as usize)
                .unwrap(),
            b"owned admitted evidence"
        );
        let accepted = point == "after";
        assert_eq!(
            snapshot
                .retained_records
                .iter()
                .any(|r| r.revision_id == fact.revision_id),
            accepted
        );
        assert_eq!(
            store
                .selected_receipt(&s, update.operation)
                .unwrap()
                .is_some(),
            accepted
        );
        assert_eq!(selected(&store, &unrelated_scope).token, unrelated.token);
        let receipt = store
            .commit_retained(&replacement.token, update.clone(), objects.clone())
            .unwrap();
        assert_eq!(receipt.replayed, accepted);
        assert_eq!(receipt.transaction.mutation, SelectionMutation::Retained);
        let token = selected(&store, &s).token;
        assert!(
            store
                .commit_retained(&replacement.token, update, objects)
                .unwrap()
                .replayed
        );
        assert_eq!(selected(&store, &s).token, token);
        assert_eq!(selected(&store, &unrelated_scope).token, unrelated.token);
    }
}
#[test]
fn selected_snapshot_preserves_exact_noncanonical_refs_and_reference_order() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = record(&store);
    let head = edit(&original);
    // Deliberately reverse causal order and store valid noncanonical JSON.
    let records = [head, original];
    let (mut change, _) = change(&records);
    let mut objects = BTreeMap::new();
    let mut references = Vec::new();
    let mut namespaces = BTreeMap::new();
    for record in &records {
        let bytes = serde_json::to_vec_pretty(record).unwrap();
        let hash = digest(&bytes);
        assert_ne!(hash, digest(&serde_json::to_vec(record).unwrap()));
        references.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(hash.clone(), record.namespace);
        objects.insert(hash, bytes);
    }
    change.selected.records = references.clone();
    change.selected.record_namespaces = namespaces;
    let publication = store
        .initialize_selected(&s, change, objects.clone())
        .unwrap();
    drop(store);
    let store = f.store();
    let snapshot = selected(&store, &s);
    assert_eq!(snapshot.token, publication.token);
    assert!(snapshot.selected_records == records);
    assert_eq!(snapshot.transaction.selected.records, references);
    for (envelope, reference) in snapshot
        .selected_records
        .iter()
        .zip(&snapshot.transaction.selected.records)
    {
        let bytes = store.read_object(reference).unwrap();
        assert_eq!(&bytes, &objects[&reference.sha256]);
        assert!(serde_json::from_slice::<Envelope>(&bytes).unwrap() == *envelope);
        let canonical = serde_json::to_vec(envelope).unwrap();
        assert!(store
            .read_object(&ObjectRef {
                sha256: digest(&canonical),
                bytes: canonical.len() as u64
            })
            .is_err());
    }
    assert_eq!(
        store
            .selected_receipt(&s, publication.transaction.operation)
            .unwrap()
            .unwrap()
            .selected
            .records,
        references
    );
}

#[test]
fn selected_predecessor_is_exact_original_history_after_replacement_and_reopen() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let base = initialize(&store, &s, &[record(&store)]);
    let next = edit(&selected(&store, &s).selected_records[0]);
    let original = store
        .commit_selected(&base.token, Uuid::new_v4(), vec![next], BTreeMap::new())
        .unwrap();
    let parent = store
        .selected_predecessor(&s, original.transaction.operation)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&parent).unwrap(),
        serde_json::to_value(&base.transaction).unwrap()
    );
    let (replacement, objects) = change(&[record(&store)]);
    let replacement = store
        .activate_selected(&original.token, replacement, objects)
        .unwrap();
    let other_scope = scope();
    initialize(&store, &other_scope, &[record(&store)]);
    let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
    assert!(store
        .selected_predecessor(&s, Uuid::new_v4())
        .unwrap()
        .is_none());
    assert!(store
        .selected_predecessor(&other_scope, original.transaction.operation)
        .unwrap()
        .is_none());
    assert!(store
        .selected_predecessor(&scope(), original.transaction.operation)
        .unwrap()
        .is_none());
    assert!(store.selected_predecessor(&s, Uuid::nil()).is_err());
    assert!(store
        .selected_predecessor(&s, base.transaction.operation)
        .is_err());
    let parent = store
        .selected_predecessor(&s, original.transaction.operation)
        .unwrap()
        .unwrap();
    assert_eq!(parent.operation, base.transaction.operation);
    assert_ne!(
        parent.aggregate_generation,
        replacement.transaction.aggregate_generation
    );
    assert_eq!(selected(&store, &s).token, replacement.token);
    assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
    drop(store);
    let reopened = f.store();
    assert_eq!(
        reopened
            .selected_predecessor(&s, original.transaction.operation)
            .unwrap()
            .unwrap()
            .operation,
        base.transaction.operation
    );
    assert_eq!(selected(&reopened, &s).token, replacement.token);
}

#[test]
fn selected_predecessor_validates_retained_media_and_preserves_original_operation_link() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let (original, replacement, intent, reference) = retained_fixture(&store, &s);
    let (manifest, objects) = closure(&[intent.clone(), edit(&intent)], BTreeMap::new());
    let settled = store
        .commit_retained(
            &replacement.token,
            RetainedCommit {
                operation: Uuid::new_v4(),
                original_operation: original.transaction.operation,
                intent: reference,
                retained: vec![manifest],
            },
            objects,
        )
        .unwrap();
    let before = selected(&store, &s).token;
    let parent = store
        .selected_predecessor(&s, settled.transaction.operation)
        .unwrap()
        .unwrap();
    assert_eq!(parent.operation, replacement.transaction.operation);
    assert_eq!(
        serde_json::to_value(&parent).unwrap(),
        serde_json::to_value(&replacement.transaction).unwrap()
    );
    let media = &intent.media_descriptors[0];
    assert!(parent
        .retained
        .iter()
        .any(|m| m.media.iter().any(|r| r.sha256 == media.sha256)));
    let root = store.generation(before.store_generation());
    fs::remove_file(root.join("objects").join(&media.sha256)).unwrap();
    assert!(store
        .selected_predecessor(&s, settled.transaction.operation)
        .is_err());
    assert_eq!(
        files::json::<SelectionTransaction>(
            &root.join("selections").join(s.filename().unwrap()),
            MAX_METADATA as u64
        )
        .unwrap()
        .operation,
        settled.transaction.operation
    );
}

#[test]
fn selected_predecessor_refuses_corrupt_missing_foreign_and_unavailable_parent_closure() {
    for case in 0..7 {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let base = initialize(&store, &s, &[record(&store)]);
        let original = if case == 5 {
            // The original's winner deliberately excludes its parent's object,
            // proving that the lookup validates the parent closure separately.
            let (replacement, objects) = change(&[record(&store)]);
            store
                .activate_selected(&base.token, replacement, objects)
                .unwrap()
        } else {
            store
                .commit_selected(
                    &base.token,
                    Uuid::new_v4(),
                    vec![record(&store)],
                    BTreeMap::new(),
                )
                .unwrap()
        };
        let root = store.generation(original.token.store_generation());
        let head_path = root.join("selections").join(s.filename().unwrap());
        let previous = original.transaction.previous_sha256.as_ref().unwrap();
        let path = root.join("selection-history").join(previous);
        match case {
            0 => fs::remove_file(&path).unwrap(),
            1 => fs::write(&path, b"corrupt").unwrap(),
            2..=4 => {
                let mut parent = base.transaction.clone();
                match case {
                    2 => parent.scope = scope(),
                    3 => parent.store_generation = Uuid::new_v4(),
                    4 => parent.operation = original.transaction.operation,
                    _ => unreachable!(),
                }
                let bytes = serde_json::to_vec(&parent).unwrap();
                let hash = digest(&bytes);
                fs::write(root.join("selection-history").join(&hash), bytes).unwrap();
                let mut head = original.transaction.clone();
                head.previous_sha256 = Some(hash);
                files::atomic_json(&head_path, &head).unwrap();
            }
            5 => fs::remove_file(
                root.join("objects")
                    .join(&base.transaction.selected.records[0].sha256),
            )
            .unwrap(),
            6 => fs::remove_file(
                root.join("objects")
                    .join(&original.transaction.selected.records.last().unwrap().sha256),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
        let head = fs::read(&head_path).unwrap();
        if case == 5 {
            assert!(store
                .selected_receipt(&s, original.transaction.operation)
                .unwrap()
                .is_some());
        }
        assert!(
            store
                .selected_predecessor(&s, original.transaction.operation)
                .is_err(),
            "case{case}"
        );
        assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
        assert_eq!(fs::read(&head_path).unwrap(), head);
    }
}

#[test]
fn retained_settlement_racing_activation_cannot_use_one_token_twice() {
    let f = Fixture::new();
    let store = Arc::new(f.store());
    let s = scope();
    let (original, replacement, intent, reference) = retained_fixture(&store, &s);
    let fact = record(&store);
    let (facts, objects) = closure(std::slice::from_ref(&fact), BTreeMap::new());
    let mut retained = replacement.transaction.retained.clone();
    retained.push(facts);
    let update = RetainedCommit {
        operation: Uuid::new_v4(),
        original_operation: original.transaction.operation,
        intent: reference,
        retained,
    };
    let (mut next, next_objects) = change(&[record(&store)]);
    next.retained = replacement.transaction.retained.clone();
    let barrier = Arc::new(Barrier::new(3));
    let settled_store = store.clone();
    let settled_token = replacement.token.clone();
    let settled_barrier = barrier.clone();
    let settlement = std::thread::spawn(move || {
        settled_barrier.wait();
        settled_store.commit_retained(&settled_token, update, objects)
    });
    let replaced_store = store.clone();
    let replaced_barrier = barrier.clone();
    let activation = std::thread::spawn(move || {
        replaced_barrier.wait();
        replaced_store.activate_selected(&replacement.token, next, next_objects)
    });
    barrier.wait();
    let results = [settlement.join().unwrap(), activation.join().unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results
        .iter()
        .find_map(|r| r.as_ref().err())
        .unwrap()
        .to_string()
        .contains("stale selection token"));
    let snapshot = selected(&store, &s);
    assert_eq!(snapshot.transaction.history_depth, 4);
    assert!(snapshot
        .retained_records
        .iter()
        .any(|r| r.revision_id == intent.revision_id));
    let settled = snapshot.transaction.mutation == SelectionMutation::Retained;
    assert_eq!(
        snapshot
            .retained_records
            .iter()
            .any(|r| r.revision_id == fact.revision_id),
        settled
    );
    drop(store);
    let reopened = f.store();
    assert_eq!(selected(&reopened, &s).token, snapshot.token);
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

#[test]
fn selected_snapshot_refuses_post_open_required_history_corruption_and_recovers_exact_bytes() {
    for case in 0..4 {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let mut historical = record(&store);
        let media = b"required historical media".to_vec();
        let media_hash = digest(&media);
        historical.media_descriptors.push(Media {
            sha256: media_hash.clone(),
            bytes: media.len() as u64,
            media_type: "application/octet-stream".into(),
        });
        let (initial_change, mut objects) = change(&[historical]);
        objects.insert(media_hash.clone(), media);
        let initial = store
            .initialize_selected(&s, initial_change, objects)
            .unwrap();
        // The replacement intentionally excludes its parent's records/media.
        // Validating only the current winner cannot establish history completeness.
        let (replacement, objects) = change(&[record(&store)]);
        let active = store
            .activate_selected(&initial.token, replacement, objects)
            .unwrap();
        assert_eq!(selected(&store, &s).token, active.token);
        let root = store.generation(active.token.store_generation());
        let head_path = root.join("selections").join(s.filename().unwrap());
        let history_path = root
            .join("selection-history")
            .join(active.transaction.previous_sha256.as_ref().unwrap());
        let damaged = match case {
            0 | 1 => history_path,
            2 => root
                .join("objects")
                .join(&initial.transaction.selected.records[0].sha256),
            3 => root.join("objects").join(&media_hash),
            _ => unreachable!(),
        };
        let bytes = fs::read(&damaged).unwrap();
        if case == 1 {
            fs::write(&damaged, b"corrupt parent metadata").unwrap();
        } else {
            fs::remove_file(&damaged).unwrap();
        }
        let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
        let head = fs::read(&head_path).unwrap();
        assert!(
            store.selected_snapshot(&s, MAX_ITEMS).is_err(),
            "case {case}"
        );
        assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
        assert_eq!(fs::read(&head_path).unwrap(), head);
        // Repair the exact fixture bytes: a valid selection is not globally disabled.
        fs::write(&damaged, &bytes).unwrap();
        assert_eq!(selected(&store, &s).token, active.token);
        drop(store);
        if case == 1 {
            fs::write(&damaged, b"corrupt parent metadata").unwrap();
        } else {
            fs::remove_file(&damaged).unwrap();
        }
        assert!(Store::open(f.paths()).is_err(), "reopen case {case}");
        assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
        assert_eq!(fs::read(&head_path).unwrap(), head);
        fs::write(&damaged, bytes).unwrap();
        assert_eq!(selected(&f.store(), &s).token, active.token);
    }
}

#[test]
fn budgeted_selected_reads_share_cumulative_limits_and_preserve_tokens() {
    let f = Fixture::new();
    let store = f.store();
    let a = scope();
    let mut b = a.clone();
    b.key_sha256 = digest(b"another aggregate");
    let first = initialize(&store, &a, &[record(&store)]);
    let second = initialize(&store, &b, &[record(&store)]);
    let root = store.generation(first.token.store_generation());
    let head_bytes = [&a, &b]
        .into_iter()
        .map(|s| {
            fs::metadata(root.join("selections").join(s.filename().unwrap()))
                .unwrap()
                .len()
        })
        .sum::<u64>();
    let record_bytes =
        first.transaction.selected.records[0].bytes + second.transaction.selected.records[0].bytes;
    // Each current closure is verified and then decoded both during full history
    // validation and during snapshot construction. All actual reads count.
    let metadata_bytes = head_bytes + 4 * record_bytes;
    let mut exact = SelectedReadBudget::new(2, 6, metadata_bytes, 0);
    assert_eq!(
        store
            .selected_snapshot_with_budget(&a, MAX_ITEMS, &mut exact)
            .unwrap()
            .unwrap()
            .snapshot
            .token,
        first.token
    );
    assert_eq!(
        store
            .selected_snapshot_with_budget(&b, MAX_ITEMS, &mut exact)
            .unwrap()
            .unwrap()
            .snapshot
            .token,
        second.token
    );
    assert_eq!(
        exact.usage(),
        SelectedReadUsage {
            aggregates: 2,
            work_items: 6,
            metadata_bytes,
            media_bytes: 0
        }
    );
    for limits in [
        (1, 6, metadata_bytes),
        (2, 5, metadata_bytes),
        (2, 6, metadata_bytes - 1),
    ] {
        let mut budget = SelectedReadBudget::new(limits.0, limits.1, limits.2, 0);
        assert!(store
            .selected_snapshot_with_budget(&a, MAX_ITEMS, &mut budget)
            .unwrap()
            .is_some());
        assert!(store
            .selected_snapshot_with_budget(&b, MAX_ITEMS, &mut budget)
            .is_err());
        assert!(budget.exhausted());
        let used = budget.usage();
        assert!(store
            .selected_snapshot_with_budget(&a, MAX_ITEMS, &mut budget)
            .is_err());
        assert_eq!(budget.usage(), used);
        assert_eq!(selected(&store, &a).token, first.token);
        assert_eq!(selected(&store, &b).token, second.token);
    }
    let mut absent = SelectedReadBudget::new(1, 0, 0, 0);
    assert!(store
        .selected_snapshot_with_budget(&scope(), MAX_ITEMS, &mut absent)
        .unwrap()
        .is_none());
    assert_eq!(absent.usage().aggregates, 1);
    assert_eq!(absent.usage().work_items, 0);
}

#[test]
fn budgeted_selected_reads_charge_history_and_metadata_before_content_io() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = initialize(&store, &s, &[record(&store)]);
    let (change, objects) = change(&[record(&store)]);
    let winner = store
        .activate_selected(&original.token, change, objects)
        .unwrap();
    let root = store.generation(winner.token.store_generation());
    let parent_path = root
        .join("selection-history")
        .join(winner.transaction.previous_sha256.as_ref().unwrap());
    let parent_bytes = fs::read(&parent_path).unwrap();
    fs::write(&parent_path, b"invalid historical JSON").unwrap();
    let head_length = fs::metadata(root.join("selections").join(s.filename().unwrap()))
        .unwrap()
        .len();
    for limits in [(1, u64::MAX), (u64::MAX, head_length)] {
        let mut budget = SelectedReadBudget::new(1, limits.0, limits.1, u64::MAX);
        let error = match store.selected_snapshot_with_budget(&s, MAX_ITEMS, &mut budget) {
            Err(error) => error,
            Ok(_) => panic!("budget should refuse before corrupt parent content"),
        };
        assert!(error.to_string().contains("budget exhausted"));
        assert_eq!(budget.usage().metadata_bytes, head_length);
        assert!(budget.exhausted());
    }
    let mut enough = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, MAX_MEDIA);
    assert!(store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut enough)
        .is_err());
    assert!(!enough.exhausted()); // Actual corruption was still validated/refused.
    fs::write(&parent_path, parent_bytes).unwrap();
    let mut repaired = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, MAX_MEDIA);
    assert_eq!(
        store
            .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut repaired)
            .unwrap()
            .unwrap()
            .snapshot
            .token,
        winner.token
    );
    assert_eq!(repaired.usage().work_items, 5); // Two transactions + three record verifications.
    let head_path = root.join("selections").join(s.filename().unwrap());
    let head_bytes = fs::read(&head_path).unwrap();
    fs::write(&head_path, b"invalid current JSON").unwrap();
    let mut zero_metadata = SelectedReadBudget::new(1, 1, 0, 0);
    let error = store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut zero_metadata)
        .err()
        .unwrap();
    assert!(error.to_string().contains("budget exhausted"));
    assert_eq!(zero_metadata.usage().metadata_bytes, 0);
    fs::write(head_path, head_bytes).unwrap();
    assert_eq!(selected(&store, &s).token, winner.token);
}

#[test]
fn budgeted_selected_reads_charge_every_media_verification_before_hashing() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let bytes = b"bounded included media".to_vec();
    let hash = digest(&bytes);
    let mut envelope = record(&store);
    envelope.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let (selected, objects) = closure(&[envelope], BTreeMap::from([(hash.clone(), bytes.clone())]));
    let publication = store
        .initialize_selected(
            &s,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    let root = store.generation(publication.token.store_generation());
    let media_path = root.join("objects").join(&hash);
    let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
    fs::write(&media_path, vec![b'X'; bytes.len()]).unwrap();
    let mut zero = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, 0);
    let error = store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut zero)
        .err()
        .unwrap();
    assert!(error.to_string().contains("budget exhausted"));
    assert_eq!(zero.usage().media_bytes, 0);
    let mut generous = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, MAX_MEDIA);
    assert!(store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut generous)
        .is_err());
    assert!(!generous.exhausted());
    fs::write(&media_path, &bytes).unwrap();
    let total = 3 * bytes.len() as u64; // History, current record validation, returned handle.
    let mut short = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, total - 1);
    assert!(store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut short)
        .is_err());
    assert_eq!(short.usage().media_bytes, 2 * bytes.len() as u64);
    let mut exact = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, total);
    let mut snapshot = store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut exact)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.snapshot.token, publication.token);
    assert_eq!(exact.usage().media_bytes, total);
    assert_eq!(exact.usage().work_items, 6);
    assert_eq!(
        snapshot
            .snapshot
            .media
            .get_mut(&hash)
            .unwrap()
            .chunk(0, bytes.len())
            .unwrap(),
        bytes
    );
    assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
}

#[test]
fn budgeted_selected_reads_return_already_verified_raw_lineage_without_extra_io() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let first = initialize(&store, &s, &[record(&store)]);
    let (change, objects) = change(&[record(&store)]);
    let second = store
        .activate_selected(&first.token, change, objects)
        .unwrap();
    let root = store.generation(second.token.store_generation());
    let raw_parent = serde_json::to_vec_pretty(&first.transaction).unwrap();
    let parent_hash = digest(&raw_parent);
    files::object(
        &root.join("selection-history").join(&parent_hash),
        &parent_hash,
        &raw_parent,
    )
    .unwrap();
    let mut current = second.transaction;
    current.previous_sha256 = Some(parent_hash.clone());
    let raw_head = serde_json::to_vec_pretty(&current).unwrap();
    let head_path = root.join("selections").join(s.filename().unwrap());
    files::atomic(&head_path, &raw_head).unwrap();
    let mut budget = SelectedReadBudget::new(1, 4096, MAX_METADATA as u64, MAX_MEDIA);
    let read = store
        .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut budget)
        .unwrap()
        .unwrap();
    let usage = budget.usage();
    assert_eq!(read.snapshot.token, selected(&store, &s).token);
    assert_eq!(read.accepted_history.len(), 2);
    assert_eq!(read.accepted_history[0].sha256, digest(&raw_head));
    assert_eq!(read.accepted_history[1].sha256, parent_hash);
    assert_eq!(
        read.accepted_history[1].transaction.operation,
        first.transaction.operation
    );
    assert_eq!(
        read.accepted_history[1].transaction.mutation,
        SelectionMutation::Initialize
    );
    assert_ne!(
        read.accepted_history[1].sha256,
        digest(&serde_json::to_vec(&read.accepted_history[1].transaction).unwrap())
    );
    // Inspecting the already validated metadata does not read/hide more work.
    assert_eq!(budget.usage(), usage);
    assert_eq!(fs::read(head_path).unwrap(), raw_head);
}

#[test]
fn budgeted_selected_history_preserves_creation_claim_custody_without_new_io() {
    for retain_old_chain in [false, true] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let document = record(&store);
        let raw_document = serde_json::to_vec_pretty(&document).unwrap();
        let document_ref = ObjectRef {
            sha256: digest(&raw_document),
            bytes: raw_document.len() as u64,
        };
        let (mut initial, _) = change(std::slice::from_ref(&document));
        initial.selected.records = vec![document_ref.clone()];
        initial.selected.record_namespaces =
            BTreeMap::from([(document_ref.sha256.clone(), document.namespace)]);
        let genesis = store
            .initialize_selected(
                &s,
                initial,
                BTreeMap::from([(document_ref.sha256.clone(), raw_document)]),
            )
            .unwrap();
        // The fact is introduced AFTER aggregate Initialize, not a genesis anchor.
        let mut fact = record(&store);
        fact.namespace = Namespace::SubjectMemory;
        fact.payload = json!({"claim":"original immutable claim", "value":"first"});
        let created = store
            .commit_selected(
                &genesis.token,
                Uuid::new_v4(),
                vec![fact.clone()],
                BTreeMap::new(),
            )
            .unwrap();
        let mut revised = edit(&fact);
        revised.payload = json!({"claim":"original immutable claim", "value":"revised"});
        let edited = store
            .commit_selected(
                &created.token,
                Uuid::new_v4(),
                vec![revised.clone()],
                BTreeMap::new(),
            )
            .unwrap();
        let mut rerooted = record(&store);
        rerooted.namespace = Namespace::SubjectMemory;
        rerooted.record_id = fact.record_id;
        rerooted.payload = json!({"claim":"different claim", "value":"replacement"});
        let (mut replacement, objects) = change(std::slice::from_ref(&rerooted));
        // Preserve the opaque noncanonical document reference, never reserialize it.
        replacement.selected.records.push(document_ref.clone());
        replacement
            .selected
            .record_namespaces
            .insert(document_ref.sha256.clone(), document.namespace);
        if retain_old_chain {
            replacement.retained = vec![edited.transaction.selected.clone()];
        }
        let winner = store
            .activate_selected(&edited.token, replacement, objects)
            .unwrap();
        let publications = [&winner, &edited, &created, &genesis];
        let references = |transaction: &SelectionTransaction| {
            std::iter::once(&transaction.selected)
                .chain(&transaction.retained)
                .flat_map(|m| &m.records)
                .map(|r| r.bytes)
                .sum::<u64>()
        };
        let reference_bytes = publications
            .iter()
            .map(|p| references(&p.transaction))
            .sum::<u64>()
            + references(&winner.transaction);
        let metadata_bytes = publications
            .iter()
            .map(|p| serde_json::to_vec(&p.transaction).unwrap().len() as u64)
            .sum::<u64>()
            + 2 * reference_bytes;
        let work_items = if retain_old_chain { 20 } else { 14 };
        let mut budget = SelectedReadBudget::new(1, work_items, metadata_bytes, 0);
        let read = store
            .selected_snapshot_with_budget(&s, MAX_ITEMS, &mut budget)
            .unwrap()
            .unwrap();
        assert_eq!(
            budget.usage(),
            SelectedReadUsage {
                aggregates: 1,
                work_items,
                metadata_bytes,
                media_bytes: 0
            }
        );
        assert_eq!(read.snapshot.token, winner.token);
        assert!(read.snapshot.selected_records.contains(&rerooted));
        assert!(!read.snapshot.selected_records.contains(&fact));
        assert!(!read.snapshot.selected_records.contains(&revised));
        assert_eq!(read.accepted_history.len(), 4);
        assert!(read.accepted_history[2].selected_records.contains(&fact));
        assert!(!read.accepted_history[3].selected_records.contains(&fact));
        let root = store.generation(winner.token.store_generation());
        for accepted in &read.accepted_history {
            assert_eq!(
                accepted.selected_records.len(),
                accepted.transaction.selected.records.len()
            );
            for (record, reference) in accepted
                .selected_records
                .iter()
                .zip(&accepted.transaction.selected.records)
            {
                let raw = fs::read(root.join("objects").join(&reference.sha256)).unwrap();
                let decoded: Envelope = serde_json::from_slice(&raw).unwrap();
                assert!(*record == decoded);
                assert_eq!(digest(&raw), reference.sha256);
                if record.record_id == document.record_id {
                    assert_eq!(*reference, document_ref);
                    assert_ne!(
                        digest(&serde_json::to_vec(record).unwrap()),
                        reference.sha256
                    );
                }
            }
        }
        assert_eq!(selected(&store, &s).token, winner.token);
    }
}
