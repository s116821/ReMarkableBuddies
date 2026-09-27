use remarkable_reader_buddy::storage::*;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    process::Command,
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!("buddy-storage-{}", Uuid::new_v4())),
        }
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.root.join("data"),
            cache: self.root.join("cache"),
            credentials: self.root.join("secrets"),
        }
    }
    fn open(&self) -> Store {
        Store::open(self.paths()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn record(store: &Store, namespace: Namespace) -> Envelope {
    Envelope {
        envelope_version: 1,
        namespace,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"fixture":"opaque"}),
        media_descriptors: vec![],
    }
}
fn edit(source: &Envelope, store: &Store, tombstone: bool) -> Envelope {
    let mut e = source.clone();
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.actor_id = store.actor_id;
    e.parents = BTreeSet::from([source.revision_id]);
    if tombstone {
        e.kind = Kind::Tombstone;
        e.payload = serde_json::Value::Null;
    } else {
        e.payload = json!({"fixture":"edited"});
    }
    e
}
fn replicate(source: &Store, target: &Store) {
    let mut pending = source.manifests().unwrap();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|m| {
            let objects = m
                .records
                .iter()
                .chain(&m.media)
                .map(|r| (r.sha256.clone(), source.read_object(r).unwrap()))
                .collect();
            target.import(m.clone(), objects).is_err()
        });
        assert!(
            pending.len() < before,
            "fixture dependencies did not resolve"
        );
    }
}

#[test]
fn interruption_boundaries_publish_only_complete_commits_and_rebuild() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::AfterCommit,
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let first = record(&store, Namespace::SubjectMemory);
        let second = record(&store, Namespace::Handwriting);
        store.set_fault(fault).unwrap();
        assert!(store
            .commit(vec![first.clone(), second.clone()], BTreeMap::new())
            .is_err());
        drop(store);
        let reopened = fixture.open();
        for e in [&first, &second] {
            assert_eq!(
                reopened.heads(e.namespace, e.record_id).unwrap().len(),
                usize::from(fault == Fault::AfterCommit)
            );
        }
        assert_eq!(reopened.unavailable_commits().unwrap(), 0);
    }
}

#[test]
fn retry_identity_conflicts_tombstones_and_stale_resolution_survive_restore() {
    let a = Fixture::new();
    let b = Fixture::new();
    let sa = a.open();
    let sb = b.open();
    let original = record(&sa, Namespace::SubjectMemory);
    let tx = sa.commit(vec![original.clone()], BTreeMap::new()).unwrap();
    assert_eq!(
        sa.commit(vec![original.clone()], BTreeMap::new()).unwrap(),
        tx
    );
    let mut collision = original.clone();
    collision.payload = json!({"different":true});
    assert!(sa.commit(vec![collision], BTreeMap::new()).is_err());
    replicate(&sa, &sb);
    let left = edit(&original, &sa, false);
    let right = edit(&original, &sb, true);
    sa.commit(vec![left.clone()], BTreeMap::new()).unwrap();
    sb.commit(vec![right.clone()], BTreeMap::new()).unwrap();
    replicate(&sa, &sb);
    replicate(&sb, &sa);
    assert_eq!(
        sa.heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        2
    );
    let mut resolution = edit(&left, &sa, false);
    assert!(sa
        .commit(vec![resolution.clone()], BTreeMap::new())
        .is_err());
    let export = a.root.join("backup");
    sa.export(&export).unwrap();
    let c = Fixture::new();
    let sc = c.open();
    sc.restore(&export).unwrap();
    sc.restore(&export).unwrap();
    assert_ne!(sc.actor_id, sa.actor_id);
    assert_eq!(
        sc.heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        2
    );
    resolution.parents.insert(right.revision_id);
    sc.commit(vec![resolution], BTreeMap::new()).unwrap();
    assert_eq!(
        sc.heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn corrupt_required_media_is_unavailable_not_empty_or_deleted() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"synthetic source media".to_vec();
    let hash = digest(&bytes);
    let mut e = record(&store, Namespace::Source);
    e.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    store
        .commit(vec![e.clone()], BTreeMap::from([(hash.clone(), bytes)]))
        .unwrap();
    let generation: Uuid = files::json(&fixture.paths().data.join("CURRENT"), 128).unwrap();
    fs::write(
        fixture
            .paths()
            .data
            .join("generations")
            .join(generation.to_string())
            .join("objects")
            .join(hash),
        b"corrupt",
    )
    .unwrap();
    drop(store);
    let reopened = fixture.open();
    assert_eq!(reopened.unavailable_commits().unwrap(), 1);
    assert!(reopened.heads(e.namespace, e.record_id).unwrap().is_empty());
    assert!(reopened.export(&fixture.root.join("not-a-backup")).is_err());
}

#[test]
fn staged_restore_failure_preserves_current_and_success_retains_rollback() {
    let source = Fixture::new();
    let ss = source.open();
    let e = record(&ss, Namespace::Conversation);
    ss.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    let backup = source.root.join("backup");
    ss.export(&backup).unwrap();
    for fault in [Fault::BeforeActivation, Fault::AfterActivation] {
        let target = Fixture::new();
        let store = target.open();
        let before = fs::read(target.paths().data.join("CURRENT")).unwrap();
        store.set_fault(fault).unwrap();
        assert!(store.restore(&backup).is_err());
        drop(store);
        let reopened = target.open();
        assert_eq!(
            reopened.heads(e.namespace, e.record_id).unwrap().len(),
            usize::from(fault == Fault::AfterActivation)
        );
        assert!(target
            .paths()
            .data
            .join("generations")
            .join(serde_json::from_slice::<Uuid>(&before).unwrap().to_string())
            .exists());
    }
}

#[test]
fn refuse_unowned_overlapping_and_symlink_roots() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.paths().data).unwrap();
    fs::write(fixture.paths().data.join("unrelated"), b"preserve").unwrap();
    assert!(Store::open(fixture.paths()).is_err());
    let clean = Fixture::new();
    let mut paths = clean.paths();
    paths.cache = paths.data.join("nested");
    assert!(Store::open(paths).is_err());
    #[cfg(unix)]
    {
        let linked = Fixture::new();
        fs::create_dir_all(&linked.root).unwrap();
        std::os::unix::fs::symlink(&fixture.root, linked.paths().data).unwrap();
        assert!(Store::open(linked.paths()).is_err());
    }
}

#[test]
fn process_lease_child() {
    if let Some(root) = std::env::var_os("BUDDY_CRASH_FIXTURE") {
        let fixture = Fixture {
            root: PathBuf::from(root),
        };
        let _store = fixture.open();
        fs::write(fixture.root.join("ready"), b"locked").unwrap();
        loop {
            std::thread::park();
        }
    }
    if let Some(root) = std::env::var_os("BUDDY_LOCK_FIXTURE") {
        let root = PathBuf::from(root);
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("secrets"),
        };
        assert!(Store::open(paths).is_err());
    }
}

#[test]
fn killed_owner_releases_os_lease_and_stale_status_does_not_authorize_a_writer() {
    use std::time::{Duration, Instant};
    let fixture = Fixture::new();
    drop(fixture.open());
    fs::write(
        fixture.paths().data.join("status.json"),
        b"{\"pid\":0,\"running\":false}",
    )
    .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_lease_child"])
        .env("BUDDY_CRASH_FIXTURE", &fixture.root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.root.join("ready").exists() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not acquire lease");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let blocked = Store::open(fixture.paths()).is_err();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(blocked);
    let _reopened = fixture.open();
}
#[test]
fn exclusive_lease_blocks_a_real_second_process_and_releases_on_drop() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_lease_child"])
        .env("BUDDY_LOCK_FIXTURE", &fixture.root)
        .status()
        .unwrap();
    assert!(status.success());
    drop(store);
    let _reopened = fixture.open();
}

#[test]
fn missing_current_cannot_silently_replace_committed_store_with_empty_generation() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store
        .commit(
            vec![record(&store, Namespace::Conversation)],
            BTreeMap::new(),
        )
        .unwrap();
    let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    drop(store);
    fs::remove_file(fixture.paths().data.join("CURRENT")).unwrap();
    assert!(Store::open(fixture.paths()).is_err());
    assert!(!fixture.paths().data.join("CURRENT").exists());
    fs::write(fixture.paths().data.join("CURRENT"), current).unwrap();
    assert_eq!(fixture.open().manifests().unwrap().len(), 1);
}

#[test]
fn export_above_global_restore_bound_never_writes_a_complete_backup_marker() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let records = (0..MAX_ITEMS)
        .map(|_| record(&store, Namespace::Conversation))
        .collect();
    store.commit(records, BTreeMap::new()).unwrap();
    store
        .commit(
            vec![record(&store, Namespace::Handwriting)],
            BTreeMap::new(),
        )
        .unwrap();
    let destination = fixture.root.join("oversized-backup");
    assert!(store.export(&destination).is_err());
    assert!(!destination.join("backup.json").exists());
    assert_eq!(store.manifests().unwrap().len(), 2);
}

#[test]
fn synthetic_registered_migration_has_atomic_activation_and_retained_rollback() {
    use remarkable_reader_buddy::storage::migration::*;
    struct Synthetic;
    impl GenerationMigration for Synthetic {
        fn source_version(&self) -> u32 {
            1
        }
        fn target_version(&self) -> u32 {
            2
        }
        fn transform(&self, path: &std::path::Path) -> anyhow::Result<()> {
            files::atomic_json(&path.join("synthetic-v2.json"), &json!({"fixture":true}))
        }
        fn validate(&self, path: &std::path::Path) -> anyhow::Result<()> {
            anyhow::ensure!(
                files::json::<serde_json::Value>(&path.join("synthetic-v2.json"), 1024)?["fixture"]
                    == true,
                "invalid fixture"
            );
            Ok(())
        }
    }
    for fault in [Fault::BeforeActivation, Fault::AfterActivation, Fault::None] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let e = record(&store, Namespace::Conversation);
        store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
        let old = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
        assert!(store.migrate(&MigrationRegistry::default(), 2).is_err());
        let mut registry = MigrationRegistry::default();
        registry.register(Synthetic).unwrap();
        store.set_fault(fault).unwrap();
        let result = store.migrate(&registry, 2);
        assert_eq!(result.is_ok(), fault == Fault::None);
        let new = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
        assert_eq!(new == old, fault == Fault::BeforeActivation);
        drop(store);
        if fault == Fault::BeforeActivation {
            assert_eq!(
                fixture
                    .open()
                    .heads(e.namespace, e.record_id)
                    .unwrap()
                    .len(),
                1
            );
        } else {
            assert!(
                Store::open(fixture.paths()).is_err(),
                "v1 runtime must refuse an unsupported actual v2 format"
            );
        }
        fs::write(fixture.paths().data.join("CURRENT"), old).unwrap();
        assert_eq!(
            fixture
                .open()
                .heads(e.namespace, e.record_id)
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn streamed_media_is_invisible_until_commit_and_short_input_fails_without_publication() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = vec![37_u8; 2 * 1024 * 1024];
    let hash = digest(&bytes);
    let reference = ObjectRef {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
    };
    assert!(store.stage_blob(&reference, &bytes[..100]).is_err());
    assert!(store.read_object(&reference).is_err());
    store.stage_blob(&reference, bytes.as_slice()).unwrap();
    assert!(store.read_object(&reference).is_err());
    let mut e = record(&store, Namespace::Source);
    e.media_descriptors.push(Media {
        sha256: hash,
        bytes: reference.bytes,
        media_type: "application/octet-stream".into(),
    });
    store.commit(vec![e], BTreeMap::new()).unwrap();
    let mut source = store.open_object(&reference).unwrap();
    assert_eq!(source.chunk(1024, 4096).unwrap(), vec![37_u8; 4096]);
}

#[test]
fn configuration_cas_export_excludes_secrets_and_restore_keeps_settings_inactive() {
    use remarkable_reader_buddy::config::Config;
    let fixture = Fixture::new();
    let store = fixture.open();
    let config = Config {
        paths: fixture.paths(),
        model: Some("fixture-model".into()),
        ..Default::default()
    };
    let path = fixture.root.join("config.json");
    let first = store.replace_config(&path, None, &config).unwrap();
    assert!(store.replace_config(&path, None, &config).is_err());
    let mut invalid = config.clone();
    invalid.config_schema_version = 2;
    assert!(store.replace_config(&path, Some(&first), &invalid).is_err());
    assert_eq!(digest(&fs::read(&path).unwrap()), first);
    store.snapshot_config(&config).unwrap();
    files::atomic(
        &store.paths.credentials.join("fake-token"),
        b"fixture-secret-never-export",
    )
    .unwrap();
    store
        .commit(
            vec![record(&store, Namespace::Conversation)],
            BTreeMap::new(),
        )
        .unwrap();
    let backup = fixture.root.join("backup");
    store.export(&backup).unwrap();
    let text = String::from_utf8(fs::read(backup.join("backup.json")).unwrap()).unwrap();
    assert!(text.contains("fixture-model"));
    assert!(!text.contains("fixture-secret-never-export"));
    assert!(!backup.join("credentials").exists());
    let target = Fixture::new();
    let restored = target.open();
    restored.restore(&backup).unwrap();
    let current: Uuid = files::json(&target.paths().data.join("CURRENT"), 128).unwrap();
    assert!(target
        .paths()
        .data
        .join("generations")
        .join(current.to_string())
        .join("restored-config.json")
        .exists());
    assert!(!target.root.join("config.json").exists());
}

#[test]
fn unsupported_incoming_versions_and_malformed_backup_do_not_change_live_heads() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let e = record(&store, Namespace::Handwriting);
    store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    let original = store.manifests().unwrap()[0].clone();
    for version in [2, 99] {
        let mut future = e.clone();
        future.envelope_version = version;
        future.revision_id = Uuid::new_v4();
        future.operation_id = Uuid::new_v4();
        future.parents.insert(e.revision_id);
        let bytes = serde_json::to_vec(&future).unwrap();
        let hash = digest(&bytes);
        let mut manifest = original.clone();
        manifest.transaction_id = Uuid::new_v4();
        manifest.records = vec![ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        }];
        manifest.record_namespaces = BTreeMap::from([(hash.clone(), future.namespace)]);
        assert!(store
            .import(manifest, BTreeMap::from([(hash, bytes)]))
            .is_err());
        assert_eq!(
            store
                .value(e.namespace, e.record_id)
                .unwrap()
                .unwrap()
                .revision_id,
            e.revision_id
        );
    }
    let mut left = edit(&e, &store, false);
    let mut right = edit(&e, &store, false);
    left.parents = BTreeSet::from([right.revision_id]);
    right.parents = BTreeSet::from([left.revision_id]);
    let mut cyclic = original.clone();
    cyclic.transaction_id = Uuid::new_v4();
    cyclic.records.clear();
    cyclic.record_namespaces.clear();
    let mut objects = BTreeMap::new();
    for envelope in [left, right] {
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let hash = digest(&bytes);
        cyclic.records.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        cyclic
            .record_namespaces
            .insert(hash.clone(), envelope.namespace);
        objects.insert(hash, bytes);
    }
    assert!(store
        .import(cyclic, objects)
        .unwrap_err()
        .to_string()
        .contains("cyclic lineage"));
    assert_eq!(
        store.heads(e.namespace, e.record_id).unwrap()[0].revision_id,
        e.revision_id
    );
    let backup = fixture.root.join("backup");
    store.export(&backup).unwrap();
    let mut value: serde_json::Value =
        files::json(&backup.join("backup.json"), MAX_METADATA as u64).unwrap();
    value["objects"][0]["sha256"] = "../escape".into();
    fs::write(
        backup.join("backup.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    assert!(store.restore(&backup).is_err());
    assert_eq!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        current
    );
}

#[test]
fn binary_replacement_fixture_preserves_store_config_and_conflict_contract() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let original = record(&store, Namespace::Conversation);
    store
        .commit(vec![original.clone()], BTreeMap::new())
        .unwrap();
    let installed = fixture.root.join("installed-reader-buddy");
    fs::write(&installed, b"old-fixture-binary").unwrap();
    fs::remove_file(&installed).unwrap();
    fs::write(&installed, b"new-fixture-binary").unwrap();
    drop(store);
    let reopened = fixture.open();
    assert_eq!(
        reopened
            .value(original.namespace, original.record_id)
            .unwrap()
            .unwrap()
            .revision_id,
        original.revision_id
    );
    let deletion = edit(&original, &reopened, true);
    reopened.commit(vec![deletion], BTreeMap::new()).unwrap();
    assert!(reopened
        .value(original.namespace, original.record_id)
        .unwrap()
        .is_none());
    assert_eq!(reopened.manifests().unwrap().len(), 2);
}
