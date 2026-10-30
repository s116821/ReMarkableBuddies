use super::buddy::storage::{
    self,
    selection::{SelectionChange, SelectionScope},
    *,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Fixture {
    pub root: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let value = Self {
            root: std::env::temp_dir().join(format!("host-archive-{}", Uuid::new_v4())),
        };
        fs::create_dir(&value.root).unwrap();
        value.save_config(false);
        value
    }
    pub fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.root.join("data"),
            cache: self.root.join("cache"),
            credentials: self.root.join("credentials"),
        }
    }
    pub fn save_config(&self, legacy: bool) {
        fs::write(
            self.root.join("config.json"),
            serde_json::to_vec(&json!({"paths":self.paths(),"sync":{"enabled":legacy}})).unwrap(),
        )
        .unwrap();
    }
    pub fn selected(&self, partial: bool) -> (Arc<Store>, SelectionScope) {
        let store = Arc::new(Store::open(self.paths()).unwrap());
        let mut unrelated = record(&store);
        unrelated.namespace = Namespace::Source;
        store.commit(vec![unrelated], BTreeMap::new()).unwrap();
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: storage::digest(b"fixture-key"),
            binding_sha256: storage::digest(b"fixture-binding"),
        };
        let (first, objects) = change(&[record(&store)], BTreeMap::new(), false);
        let first = store.initialize_selected(&scope, first, objects).unwrap();
        let bytes = b"retained owned evidence".to_vec();
        let hash = storage::digest(&bytes);
        let mut retained = record(&store);
        retained.media_descriptors.push(Media {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
            media_type: "application/octet-stream".into(),
        });
        let (retained, objects) = closure(
            &[retained],
            if partial {
                BTreeMap::new()
            } else {
                BTreeMap::from([(hash, bytes)])
            },
            partial,
        );
        let (mut next, mut next_objects) = change(&[record(&store)], BTreeMap::new(), false);
        next.retained.push(retained);
        next_objects.extend(objects);
        store
            .activate_selected(&first.token, next, next_objects)
            .unwrap();
        fs::write(
            self.paths().data.join("config.snapshot.json"),
            b"PRIVATE_RUNTIME_SENTINEL",
        )
        .unwrap();
        fs::write(
            self.paths().cache.join("private-cache"),
            b"PRIVATE_CACHE_SENTINEL",
        )
        .unwrap();
        fs::write(
            self.paths().credentials.join("private-credential"),
            b"PRIVATE_CREDENTIAL_SENTINEL",
        )
        .unwrap();
        (store, scope)
    }
    pub fn inventory(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            if !path.exists() {
                return;
            }
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, out)
                } else {
                    let bytes = if path.file_name().is_some_and(|n| n == "store.lock") {
                        fs::metadata(&path).unwrap().len().to_le_bytes().to_vec()
                    } else {
                        fs::read(&path).unwrap()
                    };
                    out.insert(path, bytes);
                }
            }
        }
        let mut out = BTreeMap::new();
        for root in [
            self.paths().data,
            self.paths().cache,
            self.paths().credentials,
        ] {
            visit(&root, &mut out)
        }
        out
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn record(store: &Store) -> Envelope {
    Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"fixture":"opaque archive custody"}),
        media_descriptors: vec![],
    }
}
fn closure(
    records: &[Envelope],
    mut objects: BTreeMap<String, Vec<u8>>,
    partial: bool,
) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut refs = vec![];
    let mut namespaces = BTreeMap::new();
    let mut media = vec![];
    let mut coverage = vec![];
    for record in records {
        let bytes = serde_json::to_vec(record).unwrap();
        let hash = storage::digest(&bytes);
        refs.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(hash.clone(), record.namespace);
        objects.insert(hash, bytes);
        for item in &record.media_descriptors {
            if partial {
                coverage.push(Coverage::Omitted {
                    sha256: item.sha256.clone(),
                    reason: Omission::SizeDeferred,
                })
            } else {
                media.push(ObjectRef {
                    sha256: item.sha256.clone(),
                    bytes: item.bytes,
                });
                coverage.push(Coverage::Included {
                    sha256: item.sha256.clone(),
                })
            }
        }
    }
    (
        Manifest {
            format: 1,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: refs,
            record_namespaces: namespaces,
            media,
            media_coverage: coverage,
        },
        objects,
    )
}
fn change(
    records: &[Envelope],
    objects: BTreeMap<String, Vec<u8>>,
    partial: bool,
) -> (SelectionChange, BTreeMap<String, Vec<u8>>) {
    let (selected, objects) = closure(records, objects, partial);
    (
        SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"fixture-base"),
            selected,
            retained: vec![],
        },
        objects,
    )
}
