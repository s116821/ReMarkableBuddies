//! Durable selected-record projections and a bounded background worker.
use super::{drive::*, *};
use crate::config::SyncPolicy;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteCommit {
    protocol: u32,
    collection: Uuid,
    manifest: Manifest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    id: String,
    tag: Tag,
    complete: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u32,
    collection: Option<Uuid>,
    initialized: bool,
    listing_page: Option<String>,
    #[serde(default)]
    listing_seen: BTreeSet<String>,
    start_token: Option<String>,
    cursor: Option<String>,
    files: BTreeMap<String, RemoteFile>,
    applied: BTreeSet<String>,
    uploads: BTreeMap<String, Upload>,
    recovery_required: bool,
    #[serde(default)]
    last_attempt: Option<String>,
    #[serde(default)]
    policy_fingerprint: String,
    #[serde(default)]
    deferred: BTreeSet<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Disabled,
    Discovering,
    Pending,
    Current,
    RecoveryRequired,
    AuthorizationRequired,
    RetryLater,
    UnsupportedOrCorrupt,
}

pub struct SyncEngine<T: DriveTransport> {
    store: Arc<Store>,
    pub policy: SyncPolicy,
    transport: T,
    state: State,
    cancel: Arc<AtomicBool>,
    legacy_lease: Option<LegacySyncLease>,
}
impl<T: DriveTransport> SyncEngine<T> {
    pub fn new(
        store: Arc<Store>,
        policy: SyncPolicy,
        transport: T,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        policy.validate()?;
        let path = store.paths.data.join("sync/state.json");
        let state = if path.exists() {
            files::json(&path, MAX_METADATA as u64)?
        } else {
            State {
                format: FORMAT,
                collection: policy.collection,
                ..Default::default()
            }
        };
        ensure!(
            state.format == FORMAT && state.collection == policy.collection,
            "sync state binding mismatch; explicit recovery required"
        );
        let legacy_lease = if policy.enabled {
            Some(store.legacy_sync_lease()?)
        } else {
            None
        };
        Ok(Self {
            store,
            policy,
            transport,
            state,
            cancel,
            legacy_lease,
        })
    }
    fn save(&self) -> Result<()> {
        files::atomic_json(&self.store.paths.data.join("sync/state.json"), &self.state)
    }
    fn alive(&self) -> Result<()> {
        ensure!(!self.cancel.load(Ordering::Relaxed), "sync cancelled");
        Ok(())
    }
    fn observe(&mut self, file: RemoteFile) -> Result<()> {
        if !file.app_properties.contains_key("buddy_protocol") {
            return Ok(());
        }
        let _ = file.tag()?;
        ensure!(
            self.state.files.len() < MAX_ITEMS || self.state.files.contains_key(&file.id),
            "remote discovery exceeds supported bound"
        );
        if file.trashed {
            self.state.recovery_required = true;
        }
        self.state.files.insert(file.id.clone(), file);
        Ok(())
    }
    pub fn step(&mut self) -> Result<Status> {
        if !self.policy.enabled {
            self.legacy_lease = None;
            return Ok(Status::Disabled);
        }
        self.alive()?;
        if self.legacy_lease.is_none() {
            self.legacy_lease = Some(self.store.legacy_sync_lease()?);
        }
        let policy_fingerprint = digest(&serde_json::to_vec(&self.policy)?);
        if self.state.policy_fingerprint != policy_fingerprint {
            self.state.applied.clear();
            self.state.deferred.clear();
            self.state.policy_fingerprint = policy_fingerprint;
            self.save()?;
        }
        let collection = self
            .policy
            .collection
            .ok_or_else(|| anyhow::anyhow!("collection selection required"))?;
        self.transport.authorize(collection)?;
        if self.state.recovery_required {
            return Ok(Status::RecoveryRequired);
        }
        if !self.state.initialized {
            if self.state.start_token.is_none() {
                self.state.start_token = Some(self.transport.start_token()?);
                self.save()?;
            }
            let page = self.transport.list(self.state.listing_page.as_deref())?;
            for file in page.items {
                self.state.listing_seen.insert(file.id.clone());
                self.observe(file)?;
            }
            self.state.listing_page = page.next;
            if self.state.listing_page.is_none() {
                let collections = self
                    .state
                    .files
                    .values()
                    .filter(|f| self.state.listing_seen.contains(&f.id))
                    .filter_map(|f| f.tag().ok().map(|t| t.collection))
                    .collect::<BTreeSet<_>>();
                if !collections.contains(&collection)
                    && !(collections.is_empty() && self.policy.create_new)
                {
                    self.state.recovery_required = true;
                    self.save()?;
                    return Ok(Status::RecoveryRequired);
                }
                if self.state.files.values().any(|f| {
                    f.tag().is_ok_and(|t| t.collection == collection)
                        && !self.state.listing_seen.contains(&f.id)
                }) {
                    self.state.recovery_required = true;
                    self.save()?;
                    return Ok(Status::RecoveryRequired);
                }
                self.state.initialized = true;
                self.state.cursor = self.state.start_token.clone();
            }
            self.save()?;
            return Ok(Status::Discovering);
        }
        // First journal every observation. Cursor advancement never discards pending
        // manifests: the file inventory/applied set is the durable work queue.
        let cursor = self
            .state
            .cursor
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing durable cursor"))?;
        let page = match self.transport.changes(&cursor) {
            Ok(page) => page,
            Err(error) => {
                if error
                    .downcast_ref::<TransportFailure>()
                    .is_some_and(|e| e.status == 410)
                {
                    self.state.initialized = false;
                    self.state.listing_page = None;
                    self.state.listing_seen.clear();
                    self.state.start_token = None;
                    // Retain old observations for recovery; never infer deletions.
                    self.save()?;
                    return Ok(Status::Discovering);
                }
                return Err(error);
            }
        };
        let more_changes = page.next.is_some();
        ensure!(
            page.next.is_some() || page.checkpoint.is_some(),
            "change page missing checkpoint"
        );
        for change in page.items {
            if change.removed {
                if self.state.files.contains_key(&change.file_id) {
                    self.state.recovery_required = true;
                }
            } else if let Some(file) = change.file {
                self.observe(file)?;
            }
        }
        self.state.cursor = page.next.or(page.checkpoint).or(Some(cursor));
        self.save()?;
        if self.state.recovery_required {
            return Ok(Status::RecoveryRequired);
        }
        if more_changes {
            return Ok(Status::Discovering);
        }
        let mut budget = self.policy.batch_items;
        let inbound = self.import_pending(collection, &mut budget)?;
        if inbound || budget == 0 {
            return Ok(Status::Pending);
        }
        self.publish_pending(collection, &mut budget)
    }
    fn import_pending(&mut self, collection: Uuid, budget: &mut usize) -> Result<bool> {
        let mut candidates: Vec<_> = self
            .state
            .files
            .values()
            .filter(|f| {
                f.tag()
                    .is_ok_and(|t| t.collection == collection && t.kind == "manifest")
                    && !self.state.applied.contains(&f.id)
            })
            .cloned()
            .collect();
        candidates.sort_by_key(|f| {
            (
                self.state
                    .last_attempt
                    .as_ref()
                    .is_some_and(|last| f.id <= *last),
                f.id.clone(),
            )
        });
        let mut pending = false;
        for file in candidates.into_iter().take(self.policy.batch_items) {
            self.alive()?;
            if *budget == 0 {
                pending = true;
                break;
            }
            self.state.last_attempt = Some(file.id.clone());
            self.save()?;
            let tag = file.tag()?;
            let incoming = self.store.paths.data.join("sync/incoming");
            files::directory(&incoming)?;
            let manifest_path = incoming.join(&tag.sha256);
            let bytes = if manifest_path.exists() {
                files::read(&manifest_path, MAX_METADATA as u64)?
            } else {
                *budget -= 1;
                self.transport.download(&file.id, MAX_METADATA as u64)?
            };
            ensure!(
                digest(&bytes) == tag.sha256,
                "remote manifest hash mismatch"
            );
            files::object(&incoming.join(&tag.sha256), &tag.sha256, &bytes)?;
            let mut remote: RemoteCommit = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("unsupported remote manifest"))?;
            ensure!(
                remote.protocol == FORMAT
                    && remote.collection == collection
                    && remote.manifest.scope != Scope::LocalTransaction,
                "remote manifest binding mismatch"
            );
            remote.manifest.validate()?;
            let original_record_count = remote.manifest.records.len();
            remote.manifest.records.retain(|r| {
                remote
                    .manifest
                    .record_namespaces
                    .get(&r.sha256)
                    .is_some_and(|n| self.policy.namespaces.contains(n))
            });
            remote
                .manifest
                .record_namespaces
                .retain(|_, n| self.policy.namespaces.contains(n));
            let mut policy_deferred = original_record_count != remote.manifest.records.len();
            if remote.manifest.records.is_empty() {
                self.state.applied.insert(file.id.clone());
                self.state.deferred.insert(file.id);
                self.save()?;
                continue;
            }
            // Selected record descriptors determine the required media; disabled
            // namespace records are never fetched merely to classify their payload.
            let mut descriptors = BTreeMap::new();
            let mut missing_record = false;
            for reference in &remote.manifest.records {
                if !self.fetch_object(collection, reference, &incoming, budget)? {
                    missing_record = true;
                    break;
                }
                let record_bytes = self.store.read_object(reference).or_else(|_| {
                    files::read(&incoming.join(&reference.sha256), MAX_RECORD as u64)
                })?;
                let envelope: Envelope = serde_json::from_slice(&record_bytes)
                    .map_err(|_| anyhow::anyhow!("unsupported inbound envelope"))?;
                envelope.validate()?;
                ensure!(
                    remote.manifest.record_namespaces.get(&reference.sha256)
                        == Some(&envelope.namespace),
                    "inbound namespace mismatch"
                );
                for descriptor in envelope.media_descriptors {
                    descriptors.insert(descriptor.sha256.clone(), descriptor);
                }
            }
            if missing_record {
                pending = true;
                continue;
            }
            remote
                .manifest
                .media_coverage
                .retain(|c| descriptors.contains_key(c.hash()));
            for coverage in &mut remote.manifest.media_coverage {
                if matches!(coverage, Coverage::Included { .. }) {
                    let reason = if !self.policy.media {
                        Some(Omission::PolicyDisabled)
                    } else if descriptors
                        .get(coverage.hash())
                        .is_some_and(|d| d.bytes > self.policy.max_media_bytes)
                    {
                        Some(Omission::SizeDeferred)
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        policy_deferred = true;
                        *coverage = Coverage::Omitted {
                            sha256: coverage.hash().to_owned(),
                            reason,
                        };
                    }
                }
            }
            remote.manifest.media.retain(|r| {
                remote
                    .manifest
                    .media_coverage
                    .iter()
                    .any(|c| matches!(c,Coverage::Included{sha256} if sha256==&r.sha256))
            });
            remote.manifest.scope = Scope::SelectedRecords;
            remote.manifest.transaction_id = Uuid::nil();
            remote.manifest.transaction_id = Uuid::from_u128(u128::from_str_radix(
                &digest(&serde_json::to_vec(&remote.manifest)?)[..32],
                16,
            )?);
            let mut missing = false;
            for reference in remote.manifest.records.iter().chain(&remote.manifest.media) {
                self.alive()?;
                if !self.fetch_object(collection, reference, &incoming, budget)? {
                    missing = true;
                    break;
                }
            }
            if missing {
                pending = true;
                continue;
            }
            if self
                .store
                .import_staged(remote.manifest, &incoming)
                .is_err()
            {
                // Bytes and remote observation remain durable/recoverable; no false
                // completion or outbound publication while lineage/schema is pending.
                pending = true;
                continue;
            }
            if policy_deferred {
                self.state.deferred.insert(file.id.clone());
            }
            self.state.applied.insert(file.id);
            self.save()?;
        }
        Ok(pending
            || self.state.files.values().any(|f| {
                f.tag()
                    .is_ok_and(|t| t.collection == collection && t.kind == "manifest")
                    && !self.state.applied.contains(&f.id)
            }))
    }
    fn fetch_object(
        &mut self,
        collection: Uuid,
        reference: &ObjectRef,
        incoming: &Path,
        budget: &mut usize,
    ) -> Result<bool> {
        self.alive()?;
        if self.store.open_object(reference).is_ok()
            || files::Source::open(&incoming.join(&reference.sha256), reference).is_ok()
        {
            return Ok(true);
        }
        if *budget == 0 {
            return Ok(false);
        }
        let source = self.state.files.values().find(|f| {
            f.tag().is_ok_and(|t| {
                t.collection == collection && t.kind == "object" && t.sha256 == reference.sha256
            })
        });
        let Some(source) = source else {
            return Ok(false);
        };
        *budget -= 1;
        self.transport
            .download_to(&source.id, reference, &incoming.join(&reference.sha256))?;
        Ok(true)
    }
    fn projection(&self, source: &Manifest) -> Result<(Manifest, BTreeMap<String, files::Source>)> {
        let mut objects = BTreeMap::new();
        let mut records = Vec::new();
        let mut record_namespaces = BTreeMap::new();
        let mut descriptors = BTreeMap::new();
        for reference in &source.records {
            let bytes = self.store.read_object(reference)?;
            let envelope: Envelope = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid local envelope"))?;
            if !self.policy.namespaces.contains(&envelope.namespace) {
                continue;
            }
            for media in envelope.media_descriptors {
                descriptors.insert(media.sha256.clone(), media);
            }
            objects.insert(reference.sha256.clone(), files::Source::Memory(bytes));
            record_namespaces.insert(reference.sha256.clone(), envelope.namespace);
            records.push(reference.clone());
        }
        records.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        let mut media = Vec::new();
        let mut coverage = Vec::new();
        for descriptor in descriptors.values() {
            let reference = ObjectRef {
                sha256: descriptor.sha256.clone(),
                bytes: descriptor.bytes,
            };
            let reason = if !self.policy.media {
                Some(Omission::PolicyDisabled)
            } else if descriptor.bytes > self.policy.max_media_bytes {
                Some(Omission::SizeDeferred)
            } else {
                match self.store.open_object(&reference) {
                    Ok(bytes) => {
                        objects.insert(reference.sha256.clone(), bytes);
                        media.push(reference);
                        None
                    }
                    Err(_) => Some(Omission::UnavailableAtSource),
                }
            };
            coverage.push(match reason {
                Some(reason) => Coverage::Omitted {
                    sha256: descriptor.sha256.clone(),
                    reason,
                },
                None => Coverage::Included {
                    sha256: descriptor.sha256.clone(),
                },
            });
        }
        let mut manifest = Manifest {
            format: FORMAT,
            transaction_id: Uuid::nil(),
            scope: Scope::SelectedRecords,
            records,
            record_namespaces,
            media,
            media_coverage: coverage,
        };
        // Canonical projection identity survives lost local journals and repeat restore.
        // No source transaction ID or excluded-domain metadata crosses the wire.
        let hash = digest(&serde_json::to_vec(&manifest)?);
        manifest.transaction_id = Uuid::from_u128(u128::from_str_radix(&hash[..32], 16)?);
        Ok((manifest, objects))
    }
    fn publish_pending(&mut self, collection: Uuid, budget: &mut usize) -> Result<Status> {
        for source in self.store.manifests()? {
            self.alive()?;
            let (manifest, objects) = self.projection(&source)?;
            if manifest.records.is_empty() {
                continue;
            }
            for (hash, mut bytes) in objects {
                let tag = Tag {
                    collection,
                    kind: "object".into(),
                    sha256: hash,
                };
                if !self.ensure_upload(tag, &mut bytes, budget)? {
                    return Ok(Status::Pending);
                }
            }
            let bytes = serde_json::to_vec(&RemoteCommit {
                protocol: FORMAT,
                collection,
                manifest,
            })?;
            let tag = Tag {
                collection,
                kind: "manifest".into(),
                sha256: digest(&bytes),
            };
            if !self.ensure_upload(tag, &mut files::Source::Memory(bytes), budget)? {
                return Ok(Status::Pending);
            }
        }
        Ok(Status::Current)
    }
    fn ensure_upload(
        &mut self,
        tag: Tag,
        bytes: &mut files::Source,
        budget: &mut usize,
    ) -> Result<bool> {
        self.alive()?;
        if self
            .state
            .uploads
            .get(&tag.sha256)
            .is_some_and(|u| u.complete)
        {
            return Ok(true);
        }
        // Discover logical content before allocating after restore; verify media,
        // never trust filename/properties alone for a successful retry.
        if let Some(file) = self
            .state
            .files
            .values()
            .find(|f| f.tag().is_ok_and(|t| t == tag))
            .cloned()
        {
            if *budget == 0 {
                return Ok(false);
            }
            *budget -= 1;
            self.transport.verify(&file.id, &bytes.reference())?;
            self.state.uploads.insert(
                tag.sha256.clone(),
                Upload {
                    id: file.id,
                    tag,
                    complete: true,
                },
            );
            self.save()?;
            return Ok(true);
        }
        if *budget == 0 {
            return Ok(false);
        }
        *budget -= 1;
        if !self.state.uploads.contains_key(&tag.sha256) {
            let id = self.transport.allocate()?;
            self.state.uploads.insert(
                tag.sha256.clone(),
                Upload {
                    id,
                    tag: tag.clone(),
                    complete: false,
                },
            );
            self.save()?;
        }
        let upload = self.state.uploads.get(&tag.sha256).cloned().unwrap();
        ensure!(upload.tag == tag, "outbox identity collision");
        let session_path = self
            .store
            .paths
            .credentials
            .join(format!("{}.session.json", tag.sha256));
        let mut session = if session_path.exists() {
            files::json(&session_path, MAX_RECORD as u64)?
        } else {
            UploadSession::default()
        };
        let progress = self
            .transport
            .upload(&upload.id, &tag, bytes, &mut session)?;
        files::atomic_json(&session_path, &session)?;
        if matches!(progress, UploadProgress::Complete) {
            self.state.uploads.get_mut(&tag.sha256).unwrap().complete = true;
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

pub struct Worker {
    cancel: Arc<AtomicBool>,
    wake: mpsc::SyncSender<()>,
    done: mpsc::Receiver<()>,
    handle: Option<thread::JoinHandle<()>>,
    store: Arc<Store>,
}
impl Worker {
    pub fn start<T: DriveTransport + 'static>(
        store: Arc<Store>,
        policy: SyncPolicy,
        transport: T,
    ) -> Result<Option<Self>> {
        if !policy.enabled {
            return Ok(None);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let mut engine =
            match SyncEngine::new(store.clone(), policy.clone(), transport, cancel.clone()) {
                Ok(engine) => engine,
                Err(_) => {
                    let _ = files::atomic_json(
                        &store.paths.data.join("sync/health.json"),
                        &json!({"status":Status::RecoveryRequired}),
                    );
                    return Ok(None);
                }
            };
        let (wake, receiver) = mpsc::sync_channel(1);
        let (done_sender, done) = mpsc::channel();
        store.set_wake(Some(wake.clone()));
        let handle=thread::Builder::new().name("buddy-drive-sync".into()).spawn(move||{
            let mut failures=0_u32;
            while !engine.cancel.load(Ordering::Relaxed) {
                let (status,retry)=match engine.step() {
                    Ok(status)=>{failures=0;(status,None)},
                    Err(error)=>{
                        failures=failures.saturating_add(1);
                        match error.downcast_ref::<TransportFailure>() {
                            Some(e) if e.authorization_required=>(Status::AuthorizationRequired,None),
                            Some(e) if e.retryable=>(Status::RetryLater,e.retry_after_seconds),
                            Some(_)=>(Status::RecoveryRequired,None),
                            None=>(Status::RetryLater,None),
                        }
                    }
                };
                let _=files::atomic_json(&engine.store.paths.data.join("sync/health.json"),&json!({"status":status,"recovery_scope":"selected-records","policy_deferred_manifests":engine.state.deferred.len(),"full_media_recovery":false}));
                if matches!(status,Status::AuthorizationRequired|Status::RecoveryRequired) {break;}
                let jitter=u64::from(Uuid::new_v4().as_bytes()[0])%3;
                let delay=retry.unwrap_or_else(|| if failures>0 {2_u64.saturating_pow(failures.min(10)).min(3600)} else if matches!(status,Status::Pending|Status::Discovering){1}else{policy.poll_seconds})+jitter;
                let deadline=std::time::Instant::now()+Duration::from_secs(delay);
                loop {
                    if engine.cancel.load(Ordering::Relaxed){break;}
                    let remaining=deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() || receiver.recv_timeout(remaining).is_err(){break;}
                    // New commits remain durable, but cannot bypass Retry-After or
                    // backoff. Cancellation still wakes this wait immediately.
                    if failures==0 {break;}
                }
            }
            let _=done_sender.send(());
        })?;
        Ok(Some(Self {
            cancel,
            wake,
            done,
            handle: Some(handle),
            store,
        }))
    }
    pub fn google(store: Arc<Store>, policy: SyncPolicy) -> Result<Option<Self>> {
        if !policy.enabled {
            return Ok(None);
        }
        // Loading a local credential file does not perform network I/O.
        match GoogleDrive::open(store.paths.credentials.join("drive.json")) {
            Ok(transport) => match Self::start(store.clone(), policy, transport) {
                Ok(worker) => Ok(worker),
                Err(_) => {
                    let _ = files::atomic_json(
                        &store.paths.data.join("sync/health.json"),
                        &json!({"status":Status::RecoveryRequired}),
                    );
                    Ok(None)
                }
            },
            Err(_) => {
                let _ = files::atomic_json(
                    &store.paths.data.join("sync/health.json"),
                    &json!({"status":Status::AuthorizationRequired}),
                );
                Ok(None)
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.store.set_wake(None);
        let _ = self.wake.try_send(());
        if self.done.recv_timeout(Duration::from_millis(250)).is_ok() {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
        // A bounded HTTP request may finish after this grace. Process termination
        // releases its lease; persisted uploads resume after a service restart.
    }
}
