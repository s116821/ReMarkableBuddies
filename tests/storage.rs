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
