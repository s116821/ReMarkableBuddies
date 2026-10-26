//! Original temporary Store/cache fixtures; no model, network or tablet.
use remarkable_reader_buddy::{
    retrieval::{disk::DiskCache, Index, NormalizedDocument, Provider, SourceFormat, SourceUnit},
    storage::{digest, Store, StorePaths, Uuid},
};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    store: Option<Store>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-retrieval-cache-{}", Uuid::new_v4()));
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let store = Store::open(paths).unwrap();
        Self {
            root,
            store: Some(store),
        }
    }
    fn store(&self) -> &Store {
        self.store.as_ref().unwrap()
    }
    fn cache_root(&self) -> PathBuf {
        self.root.join("cache/normalized-sources-v1")
    }
    fn slot(&self, id: Uuid) -> PathBuf {
        self.cache_root().join(format!("{id}.json"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.store.take());
        assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("buddy-retrieval-cache-"));
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn document(id: u128, text: &str) -> NormalizedDocument {
    NormalizedDocument {
        document: Uuid::from_u128(id),
        format: SourceFormat::Pdf,
        source_sha256: digest(text.as_bytes()),
        extractor_revision: "fixture-v1".into(),
        units: vec![SourceUnit {
            page: Some(1),
            section: None,
            text: text.into(),
        }],
    }
}

#[test]
fn provider_preserves_index_results_budgets_neighbors_and_slot_bytes() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    let material = document(
        1,
        &format!(
            "{} acceleration {}",
            "context ".repeat(300),
            "tail ".repeat(600)
        ),
    );
    let direct = Index::build(material.clone()).unwrap();
    let expected = direct.key();
    assert_eq!(&cache.install(material).unwrap(), expected);
    let before = fs::read(fixture.slot(expected.document)).unwrap();
    let provider: &dyn Provider = &cache;
    for question in ["acceleration", "missing", "  acceleration\n"] {
        for count in [1, 3] {
            for bytes in [512, 4096, 16384] {
                for neighbors in [false, true] {
                    let cached = provider.query(expected, question, count, bytes, neighbors);
                    let plain = direct.query(expected, question, count, bytes, neighbors);
                    match (cached, plain) {
                        (Ok(a), Ok(b)) => assert_eq!(
                            serde_json::to_vec(&a).unwrap(),
                            serde_json::to_vec(&b).unwrap()
                        ),
                        (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
                        _ => panic!("Provider changed Index admission"),
                    }
                }
            }
        }
    }
    for (question, count, bytes) in [
        ("", 1, 4096),
        ("acceleration", 0, 4096),
        ("acceleration", 1, 511),
    ] {
        assert_eq!(
            provider
                .query(expected, question, count, bytes, false)
                .err()
                .unwrap()
                .to_string(),
            direct
                .query(expected, question, count, bytes, false)
                .err()
                .unwrap()
                .to_string()
        );
    }
    assert_eq!(fs::read(fixture.slot(expected.document)).unwrap(), before);
}

#[test]
fn restart_rebuilds_real_index_and_requires_the_complete_expected_key() {
    let mut fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    let key = cache
        .install(document(1, "alpha acceleration context"))
        .unwrap();
    drop(cache);
    let paths = fixture.store().paths.clone();
    drop(fixture.store.take());
    fixture.store = Some(Store::open(paths).unwrap());
    let cache = DiskCache::open(fixture.store()).unwrap();
    let index = cache.load(&key).unwrap().unwrap();
    let found = index.query(&key, "acceleration", 1, 4096, false).unwrap();
    assert_eq!(found.passages[0].text, "alpha acceleration context");
    assert_eq!(found.passages[0].page, Some(1));
    for field in 0..4 {
        let mut stale = key.clone();
        match field {
            0 => stale.source_sha256 = digest(b"changed"),
            1 => stale.extractor_revision = "fixture-v2".into(),
            2 => stale.normalized_sha256 = digest(b"changed normalized"),
            _ => stale.format = SourceFormat::Notebook,
        }
        assert!(cache.load(&stale).is_err());
    }
    let missing = Index::build(document(2, "missing")).unwrap();
    assert!(cache.load(missing.key()).unwrap().is_none());
}

#[test]
fn rejected_replacement_preserves_good_bytes_and_valid_replacement_invalidates_old_key() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    let key = cache.install(document(1, "original")).unwrap();
    let original = fs::read(fixture.slot(key.document)).unwrap();
    let error = cache
        .install(document(1, "private-canary\0invalid"))
        .err()
        .unwrap();
    assert!(!error.to_string().contains("private-canary"));
    assert_eq!(fs::read(fixture.slot(key.document)).unwrap(), original);
    assert!(cache.load(&key).unwrap().is_some());
    let fresh = cache.install(document(1, "replacement")).unwrap();
    assert!(cache.load(&key).is_err());
    assert!(cache.load(&fresh).unwrap().is_some());
}

#[test]
fn tampered_material_unknown_schema_and_oversize_never_return_an_index() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    let key = cache.install(document(1, "original")).unwrap();
    let path = fixture.slot(key.document);
    let original = fs::read(&path).unwrap();
    for field in 0..3 {
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        match field {
            0 => value["document"]["units"][0]["text"] = "altered secret".into(),
            1 => value["format"] = 2.into(),
            _ => value["private-canary"] = true.into(),
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let error = cache.load(&key).err().unwrap().to_string();
        assert!(!error.contains("private-canary") && !error.contains("altered secret"));
    }
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(32 * 1024 * 1024 + 1).unwrap();
    drop(file);
    assert!(cache.load(&key).is_err());
    assert!(cache.forget(key.document).unwrap());
    assert!(cache.load(&key).unwrap().is_none());
}

#[test]
fn capacity_and_targeted_forget_preserve_other_sources_and_durable_store() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    let identity = fs::read(fixture.root.join("data/identity.json")).unwrap();
    fs::write(fixture.root.join("original.pdf"), b"source must survive").unwrap();
    let mut keys = Vec::new();
    for id in 1..=16 {
        keys.push(cache.install(document(id, "context")).unwrap());
    }
    assert!(cache.install(document(17, "too many")).is_err());
    assert!(cache.forget(keys[0].document).unwrap());
    assert!(!cache.forget(keys[0].document).unwrap());
    for key in &keys[1..] {
        assert!(cache.load(key).unwrap().is_some());
    }
    cache.install(document(17, "now fits")).unwrap();
    assert_eq!(
        fs::read(fixture.root.join("original.pdf")).unwrap(),
        b"source must survive"
    );
    assert_eq!(
        fs::read(fixture.root.join("data/identity.json")).unwrap(),
        identity
    );
    assert!(fixture.store().manifests().unwrap().is_empty());
}

#[test]
fn namespace_lock_is_exclusive_and_owned_staging_recovers_without_unknown_cleanup() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    assert!(DiskCache::open(fixture.store()).is_err());
    let key = cache
        .install(document(1, "survives staging cleanup"))
        .unwrap();
    drop(cache);
    let stage = fixture
        .cache_root()
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&stage, b"incomplete").unwrap();
    let cache = DiskCache::open(fixture.store()).unwrap();
    assert!(!stage.exists());
    assert!(cache.load(&key).unwrap().is_some());
    drop(cache);
    let foreign = fixture.cache_root().join("foreign.txt");
    fs::write(&foreign, b"do not delete").unwrap();
    assert!(DiskCache::open(fixture.store()).is_err());
    assert_eq!(fs::read(foreign).unwrap(), b"do not delete");
}

#[test]
fn unmarked_populated_namespace_is_not_adopted() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.cache_root()).unwrap();
    let foreign = fixture.cache_root().join("foreign.txt");
    fs::write(&foreign, b"unowned").unwrap();
    assert!(DiskCache::open(fixture.store()).is_err());
    assert!(!fixture.cache_root().join("owner.json").exists());
    assert_eq!(fs::read(foreign).unwrap(), b"unowned");
}

#[test]
fn shared_handle_serializes_capacity_admission() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    for id in 1..=15 {
        cache.install(document(id, "existing")).unwrap();
    }
    std::thread::scope(|scope| {
        let a = scope.spawn(|| cache.install(document(16, "a")));
        let b = scope.spawn(|| cache.install(document(17, "b")));
        assert_ne!(a.join().unwrap().is_ok(), b.join().unwrap().is_ok());
    });
}

#[cfg(unix)]
#[test]
fn symlink_slots_cannot_read_or_remove_external_files() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    let key = Index::build(document(1, "external")).unwrap().key().clone();
    let outside = fixture.root.join("external.json");
    fs::write(&outside, b"must survive").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.slot(key.document)).unwrap();
    assert!(cache.load(&key).is_err());
    assert!(cache.forget(key.document).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"must survive");
}

#[test]
fn mismatched_domain_owner_refuses_before_staging_cleanup() {
    let fixture = Fixture::new();
    let cache = DiskCache::open(fixture.store()).unwrap();
    drop(cache);
    let marker = fixture.cache_root().join("owner.json");
    let mut owner: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    owner["actor"] = Uuid::new_v4().to_string().into();
    fs::write(&marker, serde_json::to_vec(&owner).unwrap()).unwrap();
    let stage = fixture
        .cache_root()
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&stage, b"unqualified owner must not clean").unwrap();
    assert!(DiskCache::open(fixture.store()).is_err());
    assert!(stage.exists());
}
