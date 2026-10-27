use remarkable_reader_buddy::{
    conversation::AttemptReason,
    storage::*,
    unassociated_feedback::{Feedback, Observation},
};
use std::{collections::BTreeMap, fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("feedback-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn root(&self) -> PathBuf {
        let id: Uuid =
            serde_json::from_slice(&fs::read(self.paths().data.join("CURRENT")).unwrap()).unwrap();
        self.paths().data.join("generations").join(id.to_string())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn active_owner_and_unprepared_generic_write_refuse() {
    let f = Fixture::new();
    let store = Store::open(f.paths()).unwrap();
    assert!(Feedback::prepare(f.paths()).is_err());
    let e = Observation {
        schema: 1,
        actor: store.actor_id,
        operation: Uuid::new_v4(),
        observed_at: 1,
        reason: AttemptReason::NoSuccessor,
        reconciliation: false,
    }
    .envelope()
    .unwrap();
    assert!(store.commit(vec![e], BTreeMap::new()).is_err());
    assert!(store.manifests().unwrap().is_empty());
}
#[test]
fn additive_restore_retains_history_and_never_downgrades() {
    let f = Fixture::new();
    let legacy = Store::open(f.paths()).unwrap();
    let archive1 = f.0.join("archive1");
    legacy.export(&archive1).unwrap();
    drop(legacy);
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let first = feedback
        .retain(Uuid::new_v4(), 1, AttemptReason::NoSuccessor, false)
        .unwrap();
    let archive3 = f.0.join("archive3");
    feedback.export(&archive3).unwrap();
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(archive3.join("backup.json")).unwrap()).unwrap();
    assert_eq!(metadata["format"], 3);
    assert_eq!(
        metadata["feedback_history"]["origin_actor"],
        feedback.actor().to_string()
    );
    let second = feedback
        .retain(Uuid::new_v4(), 2, AttemptReason::ProviderFailure, false)
        .unwrap();
    assert!(feedback.store().restore(&archive1).is_err());
    assert!(feedback
        .store()
        .export(&f.0.join("unsupported-export"))
        .is_err());
    feedback.restore_additive(&archive3).unwrap();
    feedback.restore_additive(&archive1).unwrap();
    let results = feedback.snapshot().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.contains(&first) && results.contains(&second));
    assert_eq!(
        serde_json::from_slice::<u32>(&fs::read(f.root().join("format.json")).unwrap()).unwrap(),
        3
    );
    let other = Fixture::new();
    let foreign = Feedback::prepare(other.paths()).unwrap();
    let before = fs::read(other.paths().data.join("CURRENT")).unwrap();
    assert!(foreign.restore_additive(&archive3).is_err());
    assert_eq!(
        fs::read(other.paths().data.join("CURRENT")).unwrap(),
        before
    );
}
#[test]
fn interrupted_restore_keeps_current_and_selected_restore_is_not_granted() {
    let f = Fixture::new();
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let backup = f.0.join("backup");
    feedback.export(&backup).unwrap();
    let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
    feedback.store().set_fault(Fault::BeforeActivation).unwrap();
    assert!(feedback.restore_additive(&backup).is_err());
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
    let marker = f.root().join("selection-feature.json");
    let gen: Uuid = serde_json::from_slice(&before).unwrap();
    fs::write(
        &marker,
        serde_json::to_vec(
            &serde_json::json!({"format":1,"actor":feedback.actor(),"generation":gen}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(feedback.restore_additive(&backup).is_err());
    assert!(feedback.export(&f.0.join("selected-export")).is_err());
    assert_eq!(
        fs::read(marker).unwrap(),
        serde_json::to_vec(
            &serde_json::json!({"format":1,"actor":feedback.actor(),"generation":gen})
        )
        .unwrap()
    );
}
#[test]
fn inspection_routes_versions_without_mutating_store() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let legacy = serde_json::to_value(inspection::inspect_owned_data(&f.paths())).unwrap();
    assert_eq!(legacy["report_version"], 1);
    assert!(legacy.get("current_generation_format").is_none());
    let feedback = Feedback::prepare(f.paths()).unwrap();
    feedback
        .retain(Uuid::new_v4(), 42, AttemptReason::ProviderFailure, false)
        .unwrap();
    drop(feedback);
    fn files(root: &std::path::Path) -> BTreeMap<PathBuf, String> {
        let mut result = BTreeMap::new();
        if root.is_dir() {
            for e in fs::read_dir(root).unwrap() {
                result.extend(files(&e.unwrap().path()));
            }
        } else {
            result.insert(root.to_owned(), digest(&fs::read(root).unwrap()));
        }
        result
    }
    let before = files(&f.0);
    let report = serde_json::to_value(inspection::inspect_owned_data(&f.paths())).unwrap();
    assert_eq!(files(&f.0), before);
    assert_eq!(report["report_version"], 2);
    assert_eq!(report["current_generation_format"], 3);
    assert_eq!(
        report["observed_domains"]["unassociated-feedback"]["observed_record_files"],
        1
    );
    assert_eq!(report["credentials_inspected"], false);
    assert!(!report.to_string().contains("RMB-RESPONSE"));
}
#[test]
fn direct_sync_policy_and_unqualified_migration_refuse() {
    let mut policy = remarkable_reader_buddy::config::SyncPolicy::default();
    policy.namespaces.insert(Namespace::UnassociatedFeedback);
    assert!(policy.validate().is_err());
    struct Migration;
    impl migration::GenerationMigration for Migration {
        fn source_version(&self) -> u32 {
            3
        }
        fn target_version(&self) -> u32 {
            4
        }
        fn transform(&self, _: &std::path::Path) -> anyhow::Result<()> {
            panic!("must refuse before transform")
        }
        fn validate(&self, _: &std::path::Path) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let f = Fixture::new();
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let mut registry = migration::MigrationRegistry::default();
    registry.register(Migration).unwrap();
    let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
    assert!(feedback.store().migrate(&registry, 4).is_err());
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
}

#[test]
fn sync_constructor_and_mutated_policy_refuse_before_transport() {
    use remarkable_reader_buddy::storage::drive::*;
    use std::sync::{atomic::AtomicBool, Arc};
    struct Never;
    impl DriveTransport for Never {
        fn authorize(&mut self, _: Uuid) -> anyhow::Result<()> {
            panic!("no transport")
        }
        fn start_token(&mut self) -> anyhow::Result<String> {
            panic!("no transport")
        }
        fn list(&mut self, _: Option<&str>) -> anyhow::Result<Page<RemoteFile>> {
            panic!("no transport")
        }
        fn changes(&mut self, _: &str) -> anyhow::Result<Page<Change>> {
            panic!("no transport")
        }
        fn download(&mut self, _: &str, _: u64) -> anyhow::Result<Vec<u8>> {
            panic!("no transport")
        }
        fn allocate(&mut self) -> anyhow::Result<String> {
            panic!("no transport")
        }
        fn upload(
            &mut self,
            _: &str,
            _: &Tag,
            _: &mut files::Source,
            _: &mut UploadSession,
        ) -> anyhow::Result<UploadProgress> {
            panic!("no transport")
        }
    }
    let f = Fixture::new();
    let store = Arc::new(Store::open(f.paths()).unwrap());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut policy = remarkable_reader_buddy::config::SyncPolicy::default();
    policy.namespaces.insert(Namespace::UnassociatedFeedback);
    assert!(sync::SyncEngine::new(store.clone(), policy, Never, cancel.clone()).is_err());
    let mut engine = sync::SyncEngine::new(store, Default::default(), Never, cancel).unwrap();
    engine
        .policy
        .namespaces
        .insert(Namespace::UnassociatedFeedback);
    assert!(engine.step().is_err());
}

#[test]
fn generic_foreign_import_and_corrupt_committed_feedback_refuse() {
    let f = Fixture::new();
    let feedback = Feedback::prepare(f.paths()).unwrap();
    feedback
        .retain(Uuid::new_v4(), 1, AttemptReason::RequestFailed, false)
        .unwrap();
    let manifest = feedback.store().manifests().unwrap().remove(0);
    let mut objects = BTreeMap::new();
    for reference in &manifest.records {
        objects.insert(
            reference.sha256.clone(),
            feedback.store().read_object(reference).unwrap(),
        );
    }
    let other = Fixture::new();
    let foreign = Feedback::prepare(other.paths()).unwrap();
    assert!(foreign.store().import(manifest.clone(), objects).is_err());
    assert!(foreign.snapshot().unwrap().is_empty());
    drop(foreign);
    let root = f.root();
    drop(feedback);
    let old = &manifest.records[0];
    let mut payload: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("objects").join(&old.sha256)).unwrap()).unwrap();
    payload["payload"]["private_raw"] = serde_json::json!("SECRET-CANARY");
    let bytes = serde_json::to_vec(&payload).unwrap();
    let hash = digest(&bytes);
    fs::write(root.join("objects").join(&hash), &bytes).unwrap();
    let mut changed = manifest;
    changed.record_namespaces.remove(&changed.records[0].sha256);
    changed.records[0] = ObjectRef {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
    };
    changed
        .record_namespaces
        .insert(hash, Namespace::UnassociatedFeedback);
    fs::write(
        root.join("commits")
            .join(format!("{}.json", changed.transaction_id)),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(Feedback::prepare(f.paths()).is_err());
    let report = serde_json::to_value(inspection::inspect_owned_data(&f.paths())).unwrap();
    assert_eq!(report["domain_accounting_complete"], false);
    assert!(report["complete_apparent_bytes"].is_null());
    assert!(!report.to_string().contains("SECRET-CANARY"));
}
#[test]
fn foreign_feature_cannot_be_rebound_by_preparation() {
    let f = Fixture::new();
    drop(Feedback::prepare(f.paths()).unwrap());
    let marker = f.root().join("feedback-feature.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    value["actor"] = serde_json::json!(Uuid::new_v4());
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(&marker, &bytes).unwrap();
    assert!(Store::open(f.paths()).is_err());
    assert!(Feedback::prepare(f.paths()).is_err());
    assert_eq!(fs::read(marker).unwrap(), bytes);
}
