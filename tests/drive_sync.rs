use remarkable_reader_buddy::{
    config::SyncPolicy,
    storage::{
        drive::*,
        sync::{Status, SyncEngine, Worker},
        *,
    },
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Cloud {
    files: BTreeMap<String, (RemoteFile, Vec<u8>)>,
    changes: Vec<Change>,
    allocated: usize,
    uploads: usize,
    fail_after_create: bool,
    slow: bool,
}
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Cloud>>);
impl Fake {
    fn remove(&self, id: &str) {
        let mut c = self.0.lock().unwrap();
        c.files.remove(id);
        c.changes.push(Change {
            file_id: id.into(),
            removed: true,
            file: None,
        });
    }
}
impl DriveTransport for Fake {
    fn authorize(&mut self, _: Uuid) -> anyhow::Result<()> {
        Ok(())
    }
    fn start_token(&mut self) -> anyhow::Result<String> {
        Ok(self.0.lock().unwrap().changes.len().to_string())
    }
    fn list(&mut self, page: Option<&str>) -> anyhow::Result<Page<RemoteFile>> {
        let c = self.0.lock().unwrap();
        let offset: usize = page.unwrap_or("0").parse()?;
        let items = c
            .files
            .values()
            .skip(offset)
            .take(2)
            .map(|(f, _)| f.clone())
            .collect();
        Ok(Page {
            items,
            next: (offset + 2 < c.files.len()).then(|| (offset + 2).to_string()),
            checkpoint: None,
        })
    }
    fn changes(&mut self, page: &str) -> anyhow::Result<Page<Change>> {
        let slow = self.0.lock().unwrap().slow;
        if slow {
            std::thread::sleep(Duration::from_millis(600));
        }
        let c = self.0.lock().unwrap();
        let offset: usize = page.parse()?;
        Ok(Page {
            items: c.changes.iter().skip(offset).take(2).cloned().collect(),
            next: (offset + 2 < c.changes.len()).then(|| (offset + 2).to_string()),
            checkpoint: Some(c.changes.len().to_string()),
        })
    }
    fn download(&mut self, id: &str, limit: u64) -> anyhow::Result<Vec<u8>> {
        let c = self.0.lock().unwrap();
        let bytes = &c
            .files
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("fixture unavailable"))?
            .1;
        anyhow::ensure!(bytes.len() as u64 <= limit, "fixture exceeds limit");
        Ok(bytes.clone())
    }
    fn allocate(&mut self) -> anyhow::Result<String> {
        let mut c = self.0.lock().unwrap();
        c.allocated += 1;
        Ok(format!("file{}", c.allocated))
    }
    fn upload(
        &mut self,
        id: &str,
        tag: &Tag,
        bytes: &[u8],
        _: &mut UploadSession,
    ) -> anyhow::Result<UploadProgress> {
        let mut c = self.0.lock().unwrap();
        c.uploads += 1;
        if let Some((file, existing)) = c.files.get(id) {
            anyhow::ensure!(file.tag()? == *tag && existing == bytes, "409 conflict");
            return Ok(UploadProgress::Complete);
        }
        let file = RemoteFile {
            id: id.into(),
            trashed: false,
            app_properties: BTreeMap::from([
                ("buddy_protocol".into(), "1".into()),
                ("collection".into(), tag.collection.to_string()),
                ("kind".into(), tag.kind.clone()),
                ("sha256".into(), tag.sha256.clone()),
            ]),
        };
        c.files.insert(id.into(), (file.clone(), bytes.to_vec()));
        c.changes.push(Change {
            file_id: id.into(),
            removed: false,
            file: Some(file),
        });
        if c.fail_after_create {
            c.fail_after_create = false;
            anyhow::bail!("ambiguous fixture timeout");
        }
        Ok(UploadProgress::Complete)
    }
}
struct Device {
    root: PathBuf,
    store: Arc<Store>,
}
impl Device {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-sync-{}", Uuid::new_v4()));
        let store = Arc::new(
            Store::open(StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            })
            .unwrap(),
        );
        Self { root, store }
    }
    fn engine(&self, cloud: Fake, policy: SyncPolicy) -> SyncEngine<Fake> {
        SyncEngine::new(
            self.store.clone(),
            policy,
            cloud,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn policy(collection: Uuid, create: bool) -> SyncPolicy {
    SyncPolicy {
        enabled: true,
        namespaces: BTreeSet::from([
            Namespace::SubjectMemory,
            Namespace::Handwriting,
            Namespace::Conversation,
            Namespace::Source,
        ]),
        collection: Some(collection),
        create_new: create,
        ..Default::default()
    }
}
fn record(store: &Store, namespace: Namespace) -> Envelope {
    Envelope {
        envelope_version: 1,
        namespace,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        parents: BTreeSet::new(),
        kind: Kind::Value,
        payload: json!({"synthetic":true}),
        media_descriptors: vec![],
    }
}
fn pump(engine: &mut SyncEngine<Fake>) {
    for _ in 0..100 {
        if engine.step().unwrap() == Status::Current {
            return;
        }
    }
    panic!("sync did not converge");
}
fn revise(original: &Envelope, store: &Store, delete: bool) -> Envelope {
    let mut e = original.clone();
    e.parents = BTreeSet::from([original.revision_id]);
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.actor_id = store.actor_id;
    if delete {
        e.kind = Kind::Tombstone;
        e.payload = serde_json::Value::Null;
        e.media_descriptors.clear();
    } else {
        e.payload = json!({"edited":true});
    }
    e
}

#[test]
fn offline_two_device_conflicts_delete_and_empty_recovery_converge_without_clocks() {
    let cloud = Fake::default();
    let a = Device::new();
    let b = Device::new();
    let collection = Uuid::new_v4();
    let original = record(&a.store, Namespace::SubjectMemory);
    a.store
        .commit(vec![original.clone()], BTreeMap::new())
        .unwrap();
    let mut ea = a.engine(cloud.clone(), policy(collection, true));
    pump(&mut ea);
    let mut eb = b.engine(cloud.clone(), policy(collection, false));
    pump(&mut eb);
    assert_eq!(
        b.store
            .heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        1
    );
    a.store
        .commit(vec![revise(&original, &a.store, false)], BTreeMap::new())
        .unwrap();
    b.store
        .commit(vec![revise(&original, &b.store, true)], BTreeMap::new())
        .unwrap();
    pump(&mut eb);
    pump(&mut ea);
    pump(&mut eb);
    let heads = |s: &Store| {
        s.heads(original.namespace, original.record_id)
            .unwrap()
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(heads(&a.store), heads(&b.store));
    assert_eq!(heads(&a.store).len(), 2);
    let c = Device::new();
    let mut ec = c.engine(cloud, policy(collection, false));
    pump(&mut ec);
    assert_eq!(heads(&a.store), heads(&c.store));
}

#[test]
fn mixed_policy_history_only_later_media_and_namespace_enablement_preserve_revision_identity() {
    let cloud = Fake::default();
    let a = Device::new();
    let b = Device::new();
    let collection = Uuid::new_v4();
    let bytes = b"synthetic media fixture".to_vec();
    let hash = digest(&bytes);
    let mut memory = record(&a.store, Namespace::SubjectMemory);
    let handwriting = record(&a.store, Namespace::Handwriting);
    memory.payload = json!({"intentional_association":handwriting.record_id});
    memory.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    a.store
        .commit(
            vec![memory.clone(), handwriting.clone()],
            BTreeMap::from([(hash.clone(), bytes.clone())]),
        )
        .unwrap();
    let mut selected = policy(collection, true);
    selected.namespaces = BTreeSet::from([Namespace::SubjectMemory]);
    let mut ea = a.engine(cloud.clone(), selected.clone());
    pump(&mut ea);
    for (file, body) in cloud.0.lock().unwrap().files.values() {
        if file.tag().unwrap().kind == "manifest" {
            assert!(!String::from_utf8_lossy(body).contains(&handwriting.record_id.to_string()));
        }
        assert_ne!(
            digest(body),
            digest(&serde_json::to_vec(&handwriting).unwrap())
        );
    }
    let mut eb = b.engine(cloud.clone(), policy(collection, false));
    pump(&mut eb);
    let got = b.store.heads(memory.namespace, memory.record_id).unwrap();
    assert_eq!(got[0].revision_id, memory.revision_id);
    assert_eq!(got[0].payload, memory.payload);
    assert!(b
        .store
        .heads(handwriting.namespace, handwriting.record_id)
        .unwrap()
        .is_empty());
    let media_ref = ObjectRef {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
    };
    assert!(b.store.read_object(&media_ref).is_err());
    ea.policy.media = true;
    ea.policy.max_media_bytes = 1;
    pump(&mut ea);
    pump(&mut eb);
    assert!(b.store.read_object(&media_ref).is_err());
    ea.policy.max_media_bytes = 1024;
    pump(&mut ea);
    pump(&mut eb);
    assert_eq!(b.store.read_object(&media_ref).unwrap(), bytes);
    ea.policy.namespaces.insert(Namespace::Handwriting);
    pump(&mut ea);
    pump(&mut eb);
    assert_eq!(
        b.store
            .heads(handwriting.namespace, handwriting.record_id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        b.store.heads(memory.namespace, memory.record_id).unwrap()[0].revision_id,
        memory.revision_id
    );
    ea.policy.media = false;
    pump(&mut ea);
    assert!(b.store.read_object(&media_ref).is_ok());
}

#[test]
fn ambiguous_create_reuses_durable_id_and_removed_transport_never_deletes_local_record() {
    let cloud = Fake::default();
    let a = Device::new();
    let collection = Uuid::new_v4();
    let e = record(&a.store, Namespace::SubjectMemory);
    a.store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    let mut engine = a.engine(cloud.clone(), policy(collection, true));
    assert_eq!(engine.step().unwrap(), Status::Discovering);
    cloud.0.lock().unwrap().fail_after_create = true;
    assert!(engine.step().is_err());
    drop(engine);
    let allocated = cloud.0.lock().unwrap().allocated;
    let mut engine = a.engine(cloud.clone(), policy(collection, true));
    pump(&mut engine);
    assert_eq!(cloud.0.lock().unwrap().allocated, allocated + 1); // The second ID is the final manifest, not a repeated object.
    pump(&mut engine);
    let id = cloud.0.lock().unwrap().files.keys().next().unwrap().clone();
    cloud.remove(&id);
    assert_eq!(engine.step().unwrap(), Status::RecoveryRequired);
    assert_eq!(a.store.heads(e.namespace, e.record_id).unwrap().len(), 1);
}

#[test]
fn disabled_sync_has_no_calls_and_slow_worker_never_holds_store_mutex() {
    let cloud = Fake::default();
    let a = Device::new();
    let disabled = SyncPolicy::default();
    assert!(Worker::start(a.store.clone(), disabled, cloud.clone())
        .unwrap()
        .is_none());
    assert_eq!(cloud.0.lock().unwrap().allocated, 0);
    let mut p = policy(Uuid::new_v4(), true);
    p.poll_seconds = 5;
    cloud.0.lock().unwrap().slow = true;
    let worker = Worker::start(a.store.clone(), p, cloud.clone())
        .unwrap()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    a.store
        .commit(
            vec![record(&a.store, Namespace::SubjectMemory)],
            BTreeMap::new(),
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    let started = Instant::now();
    drop(worker);
    assert!(started.elapsed() < Duration::from_millis(500));
}
