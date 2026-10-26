use super::*;
use crate::{
    config::Config,
    storage::{files, sync::mode::SyncModeChoice, Fault, Store, StorePaths},
};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};

struct Local {
    root: PathBuf,
    config: Config,
}
impl Local {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rmb-handwriting-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let config = Config {
            paths: StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            },
            ..Config::default()
        };
        Self { root, config }
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(Store::open(self.config.paths.clone()).unwrap())
    }
    fn execute(&self, store: &Arc<Store>, value: &Value) -> Report {
        LocalCorrections::new(store.clone(), &self.config)
            .unwrap()
            .execute(admitted(value).unwrap())
    }
}
impl Drop for Local {
    fn drop(&mut self) {
        let path = self.root.canonicalize().unwrap();
        assert_eq!(
            path.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rmb-handwriting-"));
        std::fs::remove_dir_all(path).unwrap();
    }
}
fn key(command: &str) -> Value {
    json!({"handwriting_version":1,"command":command,"profile":"11111111-1111-4111-8111-111111111111","correction":"22222222-2222-4222-8222-222222222222"})
}
fn status(operation: &Value) -> Value {
    let mut value = key("status");
    value["operation"] = operation.clone();
    value
}
fn query() -> Value {
    json!({"handwriting_version":1,"command":"query","profile":"11111111-1111-4111-8111-111111111111","query_text":null,"max_matches":16,"max_encoded_bytes":65536})
}

#[test]
fn store_lifecycle_replay_update_forget_and_retained_backup_survive_reopen() {
    let local = Local::new();
    let store = local.open();
    let original = request();
    let first = local.execute(&store, &original);
    assert_eq!(first.status, Status::Complete);
    assert!(!first.historical);
    let revision = first.facts[0].revision;
    let sample = first.facts[0].value.as_ref().unwrap().sample.clone();
    drop(first);
    let replay = local.execute(&store, &original);
    assert!(replay.historical);
    assert!(replay.transaction.is_none());
    drop(replay);
    let mut update = original.clone();
    update["command"] = json!("update");
    update["operation"] = json!(Uuid::new_v4());
    update["expected_heads"] = json!([revision]);
    update["corrected_text"] = json!("F = 10 N, confirmed twice.\n");
    update["confirmed_ms"] = json!(2);
    let updated = local.execute(&store, &update);
    assert_eq!(updated.status, Status::Complete);
    let current = updated.facts[0].revision;
    drop(updated);
    let read = local.execute(&store, &query());
    assert_eq!(read.facts.len(), 1);
    assert_eq!(read.facts[0].sample_availability, Availability::Available);
    drop(read);
    let mut forget = key("forget");
    forget["operation"] = json!(Uuid::new_v4());
    forget["expected_heads"] = json!([current]);
    let removed = local.execute(&store, &forget);
    assert_eq!(removed.status, Status::Complete);
    assert_eq!(removed.facts[0].state, FactState::Tombstone);
    drop(removed);
    let read = local.execute(&store, &query());
    assert_eq!(read.status, Status::CompleteEmpty);
    drop(read);
    assert_eq!(store.media_references(&sample).unwrap(), 2);
    let backup = local.root.join("backup");
    assert!(store.export(&backup).unwrap());
    drop(store);
    let reopened = local.open();
    let read = local.execute(&reopened, &status(&original["operation"]));
    assert!(read.historical);
    assert_eq!(read.facts[0].sample_availability, Availability::Available);
    drop(read);
    drop(reopened);
    let restored_local = Local::new();
    let restored = restored_local.open();
    restored.restore(&backup).unwrap();
    let read = restored_local.execute(&restored, &status(&original["operation"]));
    assert!(read.historical);
    assert!(read.transaction.is_none());
    drop(read);
    let replay = restored_local.execute(&restored, &original);
    assert!(replay.historical);
    assert!(replay.transaction.is_none());
    drop(replay);
}

#[test]
fn operation_lookup_distinguishes_accepted_ack_loss_from_unknown_absence_without_commit() {
    let local = Local::new();
    let store = local.open();
    let original = request();
    store.set_fault(Fault::AfterCommit).unwrap();
    let uncertain = local.execute(&store, &original);
    assert_eq!(uncertain.failure, Some(Failure::CommitUnknown));
    assert_eq!(
        uncertain.diagnostics.mutation_outcome,
        MutationOutcome::Unknown
    );
    drop(uncertain);
    drop(store);
    let store = local.open();
    let before = store.manifests().unwrap().len();
    store.set_fault(Fault::BeforeObjects).unwrap();
    let known = local.execute(&store, &status(&original["operation"]));
    assert_eq!(known.status, Status::Complete);
    assert!(known.historical);
    assert!(known.transaction.is_none());
    drop(known);
    let absent = local.execute(&store, &status(&json!(Uuid::new_v4())));
    assert_eq!(absent.status, Status::CompleteEmpty);
    assert_eq!(
        absent.diagnostics.mutation_outcome,
        MutationOutcome::Unknown
    );
    drop(absent);
    assert_eq!(store.manifests().unwrap().len(), before);
    let mut next = original;
    next["correction"] = json!(Uuid::new_v4());
    next["operation"] = json!(Uuid::new_v4());
    let attempted = local.execute(&store, &next);
    assert_eq!(attempted.failure, Some(Failure::CommitUnknown));
    drop(attempted);
    assert_eq!(store.manifests().unwrap().len(), before);
}

#[test]
fn changed_operation_and_immutable_original_or_stale_heads_refuse_without_new_records() {
    let local = Local::new();
    let store = local.open();
    let original = request();
    let first = local.execute(&store, &original);
    let revision = first.facts[0].revision;
    drop(first);
    let count = store.manifests().unwrap().len();
    let mut changed = original.clone();
    changed["corrected_text"] = json!("Other correction");
    let report = local.execute(&store, &changed);
    assert_eq!(report.failure, Some(Failure::OperationConflict));
    drop(report);
    changed["command"] = json!("update");
    changed["operation"] = json!(Uuid::new_v4());
    changed["expected_heads"] = json!([revision]);
    changed["original_text"] = json!("Substituted original");
    let report = local.execute(&store, &changed);
    assert_eq!(report.failure, Some(Failure::ImmutableChange));
    drop(report);
    let mut stale = original;
    stale["operation"] = json!(Uuid::new_v4());
    let report = local.execute(&store, &stale);
    assert_eq!(report.failure, Some(Failure::HeadsConflict));
    drop(report);
    assert_eq!(store.manifests().unwrap().len(), count);
}

#[test]
fn concurrent_identical_publication_has_one_fresh_and_one_historical_observation() {
    let local = Local::new();
    let store = local.open();
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let provider = LocalCorrections::new(store.clone(), &local.config).unwrap();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let input = admitted(&request()).unwrap();
            barrier.wait();
            provider.execute(input)
        }));
    }
    let reports = workers
        .into_iter()
        .map(|w| w.join().unwrap())
        .collect::<Vec<_>>();
    assert!(reports.iter().all(|r| r.status == Status::Complete));
    assert_eq!(reports.iter().filter(|r| !r.historical).count(), 1);
    assert_eq!(
        reports.iter().filter(|r| r.transaction.is_some()).count(),
        1
    );
    assert_eq!(store.manifests().unwrap().len(), 1);
    drop(reports);
}

#[test]
fn owned_output_retains_modes_through_partial_io_failure_and_releases_on_drop() {
    struct Prefix(usize);
    impl std::io::Write for Prefix {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::other("test"));
            }
            let n = b.len().min(self.0);
            self.0 -= n;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let local = Local::new();
    let store = local.open();
    let report = local.execute(&store, &request());
    let token = store.sync_mode_snapshot().unwrap().token;
    assert!(report.write_to(&mut Prefix(7)).is_err());
    assert!(store
        .set_sync_mode(&token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
        .is_err());
    drop(report);
    store
        .set_sync_mode(&token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
        .unwrap();
    let token = store.sync_mode_snapshot().unwrap().token;
    store
        .set_sync_mode(
            &token,
            Uuid::new_v4(),
            SyncModeChoice::IsolatedBackup {
                binding_sha256: "a".repeat(64),
            },
        )
        .unwrap();
    assert!(matches!(
        LocalCorrections::new(store.clone(), &local.config),
        Err(Failure::UnsupportedMode)
    ));
}

#[test]
fn bounded_query_withholds_corrupt_samples_and_partial_is_not_empty_success() {
    let local = Local::new();
    let store = local.open();
    let first = local.execute(&store, &request());
    let hash = first.facts[0].value.as_ref().unwrap().sample.sha256.clone();
    drop(first);
    for n in 0..3 {
        let mut v = request();
        v["operation"] = json!(Uuid::new_v4());
        v["correction"] = json!(Uuid::new_v4());
        v["corrected_text"] = json!(format!("Additional confirmed text {n}"));
        drop(local.execute(&store, &v));
    }
    let mut q = query();
    q["max_matches"] = json!(1);
    let report = local.execute(&store, &q);
    assert_eq!(report.status, Status::Partial);
    assert_eq!(report.facts.len(), 1);
    assert_eq!(report.diagnostics.omitted, 3);
    drop(report);
    let generation: Uuid = files::json(&local.config.paths.data.join("CURRENT"), 128).unwrap();
    let object = local
        .config
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects")
        .join(hash);
    std::fs::write(object, b"owned synthetic corruption").unwrap();
    let report = local.execute(&store, &query());
    assert_eq!(report.status, Status::Partial);
    assert!(report.facts.is_empty());
    assert_eq!(report.diagnostics.missing_media, 4);
    drop(report);
    let report = local.execute(&store, &key("inspect"));
    assert_eq!(report.status, Status::Partial);
    assert_eq!(
        report.facts[0].sample_availability,
        Availability::Unavailable
    );
    drop(report);
}

#[test]
fn local_domain_and_legacy_publisher_exclude_each_other_until_report_drop() {
    let local = Local::new();
    let store = local.open();
    let publisher = store.legacy_publisher_lease().unwrap();
    assert!(matches!(
        LocalCorrections::new(store.clone(), &local.config),
        Err(Failure::UnsupportedMode)
    ));
    drop(publisher);
    let report = local.execute(&store, &request());
    assert!(store.legacy_publisher_lease().is_err());
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert!(store.legacy_publisher_lease().is_err());
    drop(report);
    drop(store.legacy_publisher_lease().unwrap());
}

#[test]
fn a_forged_empty_key_tombstone_never_looks_like_confirmed_history() {
    let local = Local::new();
    let store = local.open();
    let record = record_id(
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
    )
    .unwrap();
    let operation = Uuid::new_v4();
    let envelope = Envelope {
        envelope_version: 1,
        namespace: Namespace::Handwriting,
        domain_schema_version: 1,
        record_id: record,
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: operation,
        actor_id: store.actor_id,
        kind: Kind::Tombstone,
        payload: Value::Null,
        media_descriptors: vec![],
    };
    store
        .commit(vec![envelope], std::collections::BTreeMap::new())
        .unwrap();
    let report = local.execute(&store, &key("inspect"));
    assert_eq!(report.status, Status::Unsupported);
    assert_eq!(report.facts[0].state, FactState::Unsupported);
    drop(report);
    let report = local.execute(&store, &query());
    assert_eq!(report.status, Status::Partial);
    assert!(report.facts.is_empty());
    drop(report);
    let report = local.execute(&store, &status(&json!(operation)));
    assert_eq!(report.failure, Some(Failure::UnsupportedRecord));
    assert!(!report.historical);
    assert_eq!(
        report.diagnostics.mutation_outcome,
        MutationOutcome::Unknown
    );
    drop(report);
}
