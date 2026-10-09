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
    active_request: bool,
    cursor_expired: bool,
    quota_once: bool,
    change_calls: usize,
    fail_list_once: bool,
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
        let mut c = self.0.lock().unwrap();
        if page.is_some() && c.fail_list_once {
            c.fail_list_once = false;
            anyhow::bail!("injected listing interruption");
        }
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
        {
            let mut c = self.0.lock().unwrap();
            c.change_calls += 1;
            if c.quota_once {
                c.quota_once = false;
                return Err(TransportFailure {
                    status: 429,
                    retry_after_seconds: Some(2),
                    retryable: true,
                    authorization_required: false,
                }
                .into());
            }
            if c.cursor_expired {
                c.cursor_expired = false;
                return Err(TransportFailure {
                    status: 410,
                    retry_after_seconds: None,
                    retryable: false,
                    authorization_required: false,
                }
                .into());
            }
        }
        let slow = self.0.lock().unwrap().slow;
        if slow {
            self.0.lock().unwrap().active_request = true;
            std::thread::sleep(Duration::from_millis(600));
            self.0.lock().unwrap().active_request = false;
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
        source: &mut files::Source,
        _: &mut UploadSession,
    ) -> anyhow::Result<UploadProgress> {
        let bytes = source.chunk(0, source.len())?;
        let mut c = self.0.lock().unwrap();
        c.uploads += 1;
        if let Some((file, existing)) = c.files.get(id) {
            anyhow::ensure!(file.tag()? == *tag && existing == &bytes, "409 conflict");
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
    for _ in 0..500 {
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
    for deletion in [false, true] {
        for reverse in [false, true] {
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
                .commit(vec![revise(&original, &b.store, deletion)], BTreeMap::new())
                .unwrap();
            if reverse {
                pump(&mut ea);
            }
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
            let mut ec = c.engine(cloud.clone(), policy(collection, false));
            pump(&mut ec);
            assert_eq!(heads(&a.store), heads(&c.store));
            let mut resolution = revise(&original, &a.store, false);
            resolution.parents = heads(&a.store);
            a.store
                .commit(vec![resolution.clone()], BTreeMap::new())
                .unwrap();
            pump(&mut ea);
            pump(&mut eb);
            pump(&mut ec);
            assert_eq!(heads(&b.store), BTreeSet::from([resolution.revision_id]));
            assert_eq!(heads(&b.store), heads(&c.store));
            // Replay an older change cursor with durable observed/applied state intact.
            drop(eb);
            let path = b.store.paths.data.join("sync/state.json");
            let mut state: serde_json::Value = files::json(&path, MAX_METADATA as u64).unwrap();
            state["cursor"] = "0".into();
            files::atomic_json(&path, &state).unwrap();
            let before = b.store.manifests().unwrap().len();
            pump(&mut b.engine(cloud, policy(collection, false)));
            assert_eq!(b.store.manifests().unwrap().len(), before);
        }
    }
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
    eb.policy.media = true;
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
    let deadline = Instant::now() + Duration::from_secs(6);
    while !cloud.0.lock().unwrap().active_request && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        cloud.0.lock().unwrap().active_request,
        "fixture must actually be stalled in network code"
    );
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

#[test]
fn stale_or_corrupt_optional_sync_state_pauses_only_sync() {
    let a = Device::new();
    let cloud = Fake::default();
    let p = policy(Uuid::new_v4(), false);
    fs::write(a.store.paths.data.join("sync/state.json"), b"{broken").unwrap();
    assert!(Worker::start(a.store.clone(), p.clone(), cloud.clone())
        .unwrap()
        .is_none());
    let health: serde_json::Value =
        files::json(&a.store.paths.data.join("sync/health.json"), 1024).unwrap();
    assert_eq!(health["status"], "recovery-required");
    a.store
        .commit(
            vec![record(&a.store, Namespace::Conversation)],
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(
        fs::read(a.store.paths.data.join("sync/state.json")).unwrap(),
        b"{broken"
    );
}

#[test]
fn receiving_policy_defers_domains_and_media_without_fetching_then_replays_on_enable() {
    let cloud = Fake::default();
    let a = Device::new();
    let b = Device::new();
    let collection = Uuid::new_v4();
    let memory = record(&a.store, Namespace::SubjectMemory);
    let mut handwriting = record(&a.store, Namespace::Handwriting);
    let bytes = vec![71_u8; 1024];
    let hash = digest(&bytes);
    handwriting.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: 1024,
        media_type: "application/octet-stream".into(),
    });
    a.store
        .commit(
            vec![memory.clone(), handwriting.clone()],
            BTreeMap::from([(hash.clone(), bytes)]),
        )
        .unwrap();
    let mut send = policy(collection, true);
    send.media = true;
    let mut ea = a.engine(cloud.clone(), send);
    pump(&mut ea);
    let mut receive = policy(collection, false);
    receive.namespaces = BTreeSet::from([Namespace::SubjectMemory]);
    let mut eb = b.engine(cloud.clone(), receive);
    pump(&mut eb);
    assert_eq!(
        b.store
            .heads(memory.namespace, memory.record_id)
            .unwrap()
            .len(),
        1
    );
    assert!(b
        .store
        .heads(handwriting.namespace, handwriting.record_id)
        .unwrap()
        .is_empty());
    assert!(!b
        .store
        .paths
        .data
        .join("sync/incoming")
        .join(digest(&serde_json::to_vec(&handwriting).unwrap()))
        .exists());
    assert!(!b
        .store
        .paths
        .data
        .join("sync/incoming")
        .join(&hash)
        .exists());
    eb.policy.namespaces.insert(Namespace::Handwriting);
    pump(&mut eb);
    assert_eq!(
        b.store
            .heads(handwriting.namespace, handwriting.record_id)
            .unwrap()
            .len(),
        1
    );
    let reference = ObjectRef {
        sha256: hash,
        bytes: 1024,
    };
    assert!(b.store.open_object(&reference).is_err());
    eb.policy.media = true;
    pump(&mut eb);
    assert!(b.store.open_object(&reference).is_ok());
    eb.policy.media = false;
    eb.policy.namespaces.clear();
    pump(&mut eb);
    assert!(b.store.open_object(&reference).is_ok());
}

#[test]
fn more_deferred_children_than_batch_cannot_starve_their_present_parents() {
    let cloud = Fake::default();
    let a = Device::new();
    let b = Device::new();
    let collection = Uuid::new_v4();
    let mut e = record(&a.store, Namespace::SubjectMemory);
    a.store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    for _ in 0..8 {
        e = revise(&e, &a.store, false);
        a.store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    }
    let mut ea = a.engine(cloud.clone(), policy(collection, true));
    pump(&mut ea);
    // Put all descendant manifests before their parents in discovery key order.
    {
        let mut c = cloud.0.lock().unwrap();
        let old = std::mem::take(&mut c.files);
        c.changes.clear();
        for (old_id, (mut file, bytes)) in old {
            let id = if file.tag().unwrap().kind == "manifest" {
                let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let record_hash = manifest["manifest"]["records"][0]["sha256"]
                    .as_str()
                    .unwrap();
                let reference = ObjectRef {
                    sha256: record_hash.into(),
                    bytes: manifest["manifest"]["records"][0]["bytes"]
                        .as_u64()
                        .unwrap(),
                };
                let envelope: Envelope =
                    serde_json::from_slice(&a.store.read_object(&reference).unwrap()).unwrap();
                // Root is guaranteed last; >batch children precede it.
                format!(
                    "manifest-{}-{old_id}",
                    if envelope.parents.is_empty() {
                        "z"
                    } else {
                        "a"
                    }
                )
            } else {
                old_id
            };
            file.id = id.clone();
            c.files.insert(id, (file, bytes));
        }
    }
    let mut p = policy(collection, false);
    p.batch_items = 2;
    cloud.0.lock().unwrap().fail_list_once = true;
    let mut complete = false;
    let mut interrupted = false;
    for _ in 0..500 {
        // Reconstruct from durable state after every bounded batch/page.
        match b.engine(cloud.clone(), p.clone()).step() {
            Ok(Status::Current) => {
                complete = true;
                break;
            }
            Ok(_) => (),
            Err(error) => {
                assert!(error.to_string().contains("injected listing interruption"));
                interrupted = true;
            }
        }
    }
    assert!(complete && interrupted);
    assert_eq!(
        b.store.heads(e.namespace, e.record_id).unwrap()[0].revision_id,
        e.revision_id
    );
}

#[test]
fn rejected_cursor_and_disappeared_collection_do_not_recreate_cloud_from_local_defaults() {
    let cloud = Fake::default();
    let a = Device::new();
    let collection = Uuid::new_v4();
    let e = record(&a.store, Namespace::SubjectMemory);
    a.store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    let mut engine = a.engine(cloud.clone(), policy(collection, true));
    pump(&mut engine);
    pump(&mut engine);
    let uploads = cloud.0.lock().unwrap().uploads;
    {
        let mut c = cloud.0.lock().unwrap();
        c.files.clear();
        c.changes.clear();
        c.cursor_expired = true;
    }
    assert_eq!(engine.step().unwrap(), Status::Discovering);
    assert_eq!(engine.step().unwrap(), Status::RecoveryRequired);
    assert_eq!(cloud.0.lock().unwrap().uploads, uploads);
    assert!(a.store.value(e.namespace, e.record_id).unwrap().is_some());
}

#[test]
fn new_local_wakes_cannot_bypass_retry_after_but_shutdown_still_cancels_wait() {
    let cloud = Fake::default();
    cloud.0.lock().unwrap().quota_once = true;
    let a = Device::new();
    let p = policy(Uuid::new_v4(), true);
    let worker = Worker::start(a.store.clone(), p, cloud.clone())
        .unwrap()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let status =
            files::json::<serde_json::Value>(&a.store.paths.data.join("sync/health.json"), 1024)
                .ok();
        if status.is_some_and(|v| v["status"] == "retry-later") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not encounter injected quota failure"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let calls = cloud.0.lock().unwrap().change_calls;
    for _ in 0..3 {
        a.store
            .commit(
                vec![record(&a.store, Namespace::SubjectMemory)],
                BTreeMap::new(),
            )
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(cloud.0.lock().unwrap().change_calls, calls);
    let started = Instant::now();
    drop(worker);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn worker_polls_remote_only_arrivals_and_durable_work_survives_a_lost_wake() {
    let cloud = Fake::default();
    let a = Device::new();
    let b = Device::new();
    let collection = Uuid::new_v4();
    let first = record(&a.store, Namespace::Conversation);
    a.store
        .commit(vec![first.clone()], BTreeMap::new())
        .unwrap();
    let mut ea = a.engine(cloud.clone(), policy(collection, true));
    pump(&mut ea);
    let mut p = policy(collection, false);
    p.poll_seconds = 5;
    let worker = Worker::start(b.store.clone(), p.clone(), cloud.clone())
        .unwrap()
        .unwrap();
    let wait_for = |e: &Envelope| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while b.store.value(e.namespace, e.record_id).unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "remote polling did not import record"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait_for(&first);
    let second = record(&a.store, Namespace::Conversation);
    a.store
        .commit(vec![second.clone()], BTreeMap::new())
        .unwrap();
    pump(&mut ea);
    wait_for(&second); // No local commit or wake on b.
    b.store.set_wake(None); // Model a lost hint, keeping the durable commit.
    let local = record(&b.store, Namespace::SubjectMemory);
    b.store
        .commit(vec![local.clone()], BTreeMap::new())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        pump(&mut ea);
        if a.store
            .value(local.namespace, local.record_id)
            .unwrap()
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "lost wake stranded durable work");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(worker);
    let worker = Worker::start(b.store.clone(), p, cloud).unwrap().unwrap();
    assert_eq!(
        b.store
            .heads(local.namespace, local.record_id)
            .unwrap()
            .len(),
        1
    );
    drop(worker);
}

#[test]
fn namespace_labels_cannot_smuggle_a_disabled_domain_into_selected_records() {
    let cloud = Fake::default();
    let a = Device::new();
    let b = Device::new();
    let collection = Uuid::new_v4();
    let e = record(&a.store, Namespace::SubjectMemory);
    a.store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    pump(&mut a.engine(cloud.clone(), policy(collection, true)));
    {
        let mut c = cloud.0.lock().unwrap();
        for (file, bytes) in c.files.values_mut() {
            if file.tag().unwrap().kind == "manifest" {
                let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                for label in value["manifest"]["record_namespaces"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    *label = "conversation".into();
                }
                *bytes = serde_json::to_vec(&value).unwrap();
                file.app_properties.insert("sha256".into(), digest(bytes));
            }
        }
    }
    let mut p = policy(collection, false);
    p.namespaces = BTreeSet::from([Namespace::Conversation]);
    let mut engine = b.engine(cloud.clone(), p);
    let uploads = cloud.0.lock().unwrap().uploads;
    let mut refused = false;
    for _ in 0..20 {
        if let Err(error) = engine.step() {
            assert!(error.to_string().contains("namespace mismatch"));
            refused = true;
            break;
        }
    }
    assert!(refused);
    assert!(b.store.manifests().unwrap().is_empty());
    assert_eq!(cloud.0.lock().unwrap().uploads, uploads);
}

#[test]
fn legacy_worker_and_selected_namespace_cannot_overlap_or_publish_retained_history() {
    use remarkable_reader_buddy::storage::selection::{SelectionChange, SelectionScope};
    let device = Device::new();
    let cloud = Fake::default();
    let collection = Uuid::new_v4();
    let original = record(&device.store, Namespace::Conversation);
    device
        .store
        .commit(vec![original], BTreeMap::new())
        .unwrap();
    let mut manifest = device.store.manifests().unwrap()[0].clone();
    manifest.scope = Scope::SelectedRecords;
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"owned document"),
        binding_sha256: digest(b"owned binding"),
    };
    let change = SelectionChange {
        operation: Uuid::new_v4(),
        accepted_base_sha256: digest(b"base"),
        selected: manifest,
        retained: vec![],
    };
    let mut engine = device.engine(cloud.clone(), policy(collection, true));
    assert!(device
        .store
        .initialize_selected(&scope, change.clone(), BTreeMap::new())
        .is_err());
    assert!(device
        .store
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .is_none());
    engine.policy.enabled = false;
    assert_eq!(engine.step().unwrap(), Status::Disabled);
    device
        .store
        .initialize_selected(&scope, change, BTreeMap::new())
        .unwrap();
    engine.policy.enabled = true;
    assert!(engine.step().is_err());
    assert!(SyncEngine::new(
        device.store.clone(),
        policy(collection, true),
        cloud.clone(),
        Arc::new(AtomicBool::new(false))
    )
    .is_err());
    let mut disabled = policy(collection, true);
    disabled.enabled = false;
    assert_eq!(
        device.engine(cloud.clone(), disabled).step().unwrap(),
        Status::Disabled
    );
    let c = cloud.0.lock().unwrap();
    assert_eq!(c.allocated, 0);
    assert_eq!(c.uploads, 0);
    assert!(c.files.is_empty());
}
