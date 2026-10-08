//! Versioned local persistence, independent of Reader/Writer domain semantics.
pub mod drive;
pub mod files;
pub mod migration;
mod recovery;
#[cfg(test)]
mod selected_projection_tests;
pub mod selection;
pub mod sync;
pub mod types;
use anyhow::{ensure, Context, Result};
pub use recovery::BackupInspection;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{mpsc::SyncSender, Arc, Mutex},
};
pub use types::*;
pub const CONTRACT_V1: &str = include_str!("contract-v1.json");

#[derive(Debug)]
pub struct Conflict {
    pub heads: BTreeSet<Uuid>,
}
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("conflict: expected parents changed")
    }
}
impl std::error::Error for Conflict {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorePaths {
    pub data: PathBuf,
    pub cache: PathBuf,
    pub credentials: PathBuf,
}
impl Default for StorePaths {
    fn default() -> Self {
        Self {
            data: "/home/root/.local/share/remarkable-buddies".into(),
            cache: "/home/root/.cache/remarkable-buddies".into(),
            credentials: "/home/root/.config/remarkable-buddies/credentials".into(),
        }
    }
}
impl StorePaths {
    pub fn validate(&self) -> Result<()> {
        let paths = [&self.data, &self.cache, &self.credentials];
        for (i, path) in paths.iter().enumerate() {
            files::safe_path(path)?;
            for other in &paths[i + 1..] {
                ensure!(
                    !path.starts_with(other) && !other.starts_with(path),
                    "overlapping owned roots"
                );
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    format: u32,
    actor: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    BeforeObjects,
    AfterObjects,
    BeforeCommit,
    AfterCommit,
    BeforeActivation,
    AfterActivation,
}

#[derive(Default)]
struct Index {
    records: BTreeMap<Uuid, (Envelope, String)>,
    operations: BTreeMap<Uuid, String>,
    heads: BTreeMap<(Namespace, Uuid), BTreeSet<Uuid>>,
    manifests: BTreeMap<Uuid, Manifest>,
    unavailable: usize,
}
struct Inner {
    generation: Uuid,
    index: Index,
    fault: Fault,
    legacy_sync_handles: usize,
}
pub struct Store {
    pub paths: StorePaths,
    pub actor_id: Uuid,
    _lease: File,
    inner: Mutex<Inner>,
    wake: Mutex<Option<SyncSender<()>>>,
}

pub(crate) struct LegacySyncLease(Arc<Store>);
impl Drop for LegacySyncLease {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.0.inner.lock() {
            inner.legacy_sync_handles -= 1;
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // A concurrent subprocess fork can temporarily inherit this descriptor.
        // Closing our copy alone would retain flock until that child execs.
        let _ = self._lease.unlock();
    }
}

impl Store {
    pub(crate) fn legacy_sync_lease(self: &Arc<Self>) -> Result<LegacySyncLease> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            !selection::has_selected(&self.generation(inner.generation))?,
            "selected storage requires coordinated sync, not the legacy all-history worker"
        );
        inner.legacy_sync_handles = inner
            .legacy_sync_handles
            .checked_add(1)
            .context("sync lease overflow")?;
        Ok(LegacySyncLease(self.clone()))
    }
    pub fn open(paths: StorePaths) -> Result<Self> {
        paths.validate()?;
        let identity_path = paths.data.join("identity.json");
        if !identity_path.exists() {
            if paths.data.exists() {
                ensure!(
                    fs::read_dir(&paths.data)?.next().is_none(),
                    "non-owned data root refused"
                );
            }
            files::directory(&paths.data)?;
            files::create_identity(
                &identity_path,
                &Identity {
                    format: FORMAT,
                    actor: Uuid::new_v4(),
                },
            )?;
        }
        let identity: Identity = files::json(&identity_path, MAX_RECORD as u64)?;
        ensure!(
            identity.format == FORMAT && !identity.actor.is_nil(),
            "unsupported store identity"
        );
        let lock_path = paths.data.join("store.lock");
        files::safe_path(&lock_path)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lease
            .try_lock()
            .map_err(|_| anyhow::anyhow!("store is owned by another process"))?;
        for root in [&paths.cache, &paths.credentials] {
            if root.exists() && !root.join("owner.json").exists() {
                ensure!(
                    fs::read_dir(root)?.next().is_none(),
                    "non-owned auxiliary root refused"
                );
            }
            files::directory(root)?;
            let marker = root.join("owner.json");
            if marker.exists() {
                let owner: Identity = files::json(&marker, MAX_RECORD as u64)?;
                ensure!(
                    owner.actor == identity.actor && owner.format == FORMAT,
                    "auxiliary ownership mismatch"
                );
            } else {
                files::atomic_json(&marker, &identity)?;
            }
        }
        for child in ["generations", "quarantine", "sync"] {
            files::directory(&paths.data.join(child))?;
        }
        let current = paths.data.join("CURRENT");
        let generation: Uuid = if current.exists() {
            files::json(&current, 128)?
        } else {
            ensure!(
                fs::read_dir(paths.data.join("generations"))?
                    .next()
                    .is_none(),
                "CURRENT is missing from an existing store; explicit recovery required"
            );
            let id = Uuid::new_v4();
            Self::create_generation(&paths.data, id)?;
            files::atomic_json(&current, &id)?;
            id
        };
        let store = Self {
            paths,
            actor_id: identity.actor,
            _lease: lease,
            inner: Mutex::new(Inner {
                generation,
                index: Index::default(),
                fault: Fault::None,
                legacy_sync_handles: 0,
            }),
            wake: Mutex::new(None),
        };
        selection::validate_generation(&store.generation(generation), store.actor_id, generation)?;
        for directory in [
            store.paths.data.clone(),
            store.paths.credentials.clone(),
            store.paths.data.join("sync"),
            store.paths.data.join("sync/incoming"),
            store.generation(generation),
            store.generation(generation).join("objects"),
            store.generation(generation).join("commits"),
            store.generation(generation).join("selections"),
            store.generation(generation).join("selection-history"),
        ] {
            files::cleanup_staging(&directory)?;
        }
        {
            let mut inner = store
                .inner
                .lock()
                .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
            inner.index = store.recover(generation)?;
        }
        files::atomic(
            &store.paths.data.join("contract-v1.json"),
            CONTRACT_V1.as_bytes(),
        )?;
        files::atomic_json(
            &store.paths.data.join("capabilities.json"),
            &serde_json::json!({
            "contract_version": 1, "schema_file":"contract-v1.json", "store_versions": [1, 2], "selected_store_feature": "actor-generation-bound-v1", "envelope_versions": [1], "config_versions": [1],
                "paths": store.paths, "lock": "stable-inode-exclusive-os-lease", "linux_lock": "flock",
                "maintenance": "stop-confirm-lock-stage-commit-release-restart", "namespaces": ["conversation","source","export-association","subject-memory","handwriting"], "sync": "optional-immutable-drive-v1"
            }),
        )?;
        Ok(store)
    }
    fn create_generation(root: &Path, id: Uuid) -> Result<()> {
        for child in ["objects", "commits"] {
            files::directory(&root.join("generations").join(id.to_string()).join(child))?;
        }
        files::atomic_json(
            &root
                .join("generations")
                .join(id.to_string())
                .join("format.json"),
            &FORMAT,
        )
    }
    fn generation(&self, id: Uuid) -> PathBuf {
        self.paths.data.join("generations").join(id.to_string())
    }
    fn recover(&self, generation: Uuid) -> Result<Index> {
        let root = self.generation(generation);
        let mut index = Index::default();
        let mut pending = Vec::new();
        for entry in fs::read_dir(root.join("commits"))? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|s| s.to_string_lossy().starts_with(".stage-"))
            {
                continue;
            }
            let result = (|| {
                let manifest: Manifest = files::json(&path, MAX_METADATA as u64)?;
                let records = Self::validate_objects(&root, &manifest)?;
                Ok::<_, anyhow::Error>((manifest, records))
            })();
            match result {
                Ok(item) => pending.push(item),
                Err(_) => index.unavailable += 1,
            }
        }
        pending.extend(selection::recover_selected(&root, generation)?);
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|(manifest, records)| {
                if Self::can_apply(&index, records).is_ok() {
                    Self::apply_index(&mut index, manifest.clone(), records.clone());
                    false
                } else {
                    true
                }
            });
            if pending.len() == before {
                index.unavailable += pending.len();
                break;
            }
        }
        Ok(index)
    }
    fn validate_objects(root: &Path, manifest: &Manifest) -> Result<Vec<(Envelope, String)>> {
        manifest.validate()?;
        for item in manifest.records.iter().chain(&manifest.media) {
            let _ = files::Source::open(&root.join("objects").join(&item.sha256), item)?;
        }
        let mut records = Vec::new();
        let mut descriptors = BTreeMap::new();
        for item in &manifest.records {
            ensure!(item.bytes <= MAX_RECORD as u64, "record exceeds bound");
            let envelope: Envelope =
                files::json(&root.join("objects").join(&item.sha256), MAX_RECORD as u64)?;
            envelope.validate()?;
            ensure!(
                manifest.record_namespaces.get(&item.sha256) == Some(&envelope.namespace),
                "manifest namespace mismatch"
            );
            for media in &envelope.media_descriptors {
                if let Some(previous) = descriptors.insert(media.sha256.clone(), media.clone()) {
                    ensure!(previous == *media, "inconsistent media descriptors");
                }
            }
            records.push((envelope, item.sha256.clone()));
        }
        ensure!(
            descriptors
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == manifest.media_coverage.iter().map(Coverage::hash).collect(),
            "coverage must describe every media descriptor"
        );
        for item in &manifest.media {
            ensure!(
                descriptors
                    .get(&item.sha256)
                    .is_some_and(|d| d.bytes == item.bytes),
                "media size mismatch"
            );
        }
        Ok(records)
    }
    fn can_apply(index: &Index, records: &[(Envelope, String)]) -> Result<()> {
        let batch: BTreeMap<_, _> = records
            .iter()
            .map(|(e, h)| (e.revision_id, (e, h)))
            .collect();
        ensure!(
            batch.len() == records.len(),
            "duplicate revision in transaction"
        );
        let mut operations = BTreeMap::new();
        for (e, hash) in records {
            if let Some(previous) = index.records.get(&e.revision_id) {
                ensure!(&previous.1 == hash, "revision identity collision");
            }
            if let Some(previous) = index
                .operations
                .get(&e.operation_id)
                .or_else(|| operations.get(&e.operation_id))
            {
                ensure!(previous == hash, "operation identity collision");
            }
            operations.insert(e.operation_id, hash.clone());
            let mut stack: Vec<_> = e.parents.iter().copied().collect();
            let mut visited = BTreeSet::new();
            while let Some(id) = stack.pop() {
                ensure!(id != e.revision_id, "cyclic lineage");
                if !visited.insert(id) {
                    continue;
                }
                let parent = index
                    .records
                    .get(&id)
                    .map(|p| &p.0)
                    .or_else(|| batch.get(&id).map(|p| p.0))
                    .context("pending parent dependency")?;
                ensure!(
                    parent.namespace == e.namespace && parent.record_id == e.record_id,
                    "parent belongs to different record"
                );
                stack.extend(parent.parents.iter().copied());
            }
        }
        Ok(())
    }
    fn apply_index(index: &mut Index, manifest: Manifest, records: Vec<(Envelope, String)>) {
        for (e, hash) in records {
            index.operations.insert(e.operation_id, hash.clone());
            index.records.insert(e.revision_id, (e, hash));
        }
        index.heads.clear();
        for (e, _) in index.records.values() {
            index
                .heads
                .entry((e.namespace, e.record_id))
                .or_default()
                .insert(e.revision_id);
        }
        for (e, _) in index.records.values() {
            if let Some(heads) = index.heads.get_mut(&(e.namespace, e.record_id)) {
                for parent in &e.parents {
                    heads.remove(parent);
                }
            }
        }
        index.manifests.insert(manifest.transaction_id, manifest);
    }
    pub fn heads(&self, namespace: Namespace, record: Uuid) -> Result<Vec<Envelope>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        Ok(inner
            .index
            .heads
            .get(&(namespace, record))
            .into_iter()
            .flatten()
            .filter_map(|id| inner.index.records.get(id).map(|p| p.0.clone()))
            .collect())
    }
    pub fn value(&self, namespace: Namespace, record: Uuid) -> Result<Option<Envelope>> {
        let mut heads = self.heads(namespace, record)?;
        if heads.len() > 1 {
            return Err(Conflict {
                heads: heads.iter().map(|e| e.revision_id).collect(),
            }
            .into());
        }
        Ok(heads.pop().filter(|e| e.kind == Kind::Value))
    }
    pub fn unavailable_commits(&self) -> Result<usize> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?
            .index
            .unavailable)
    }
    pub fn set_fault(&self, fault: Fault) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?
            .fault = fault;
        Ok(())
    }
    fn trip(inner: &mut Inner, fault: Fault) -> Result<()> {
        if inner.fault == fault {
            inner.fault = Fault::None;
            anyhow::bail!("injected storage interruption");
        }
        Ok(())
    }
    pub fn commit(&self, records: Vec<Envelope>, media: BTreeMap<String, Vec<u8>>) -> Result<Uuid> {
        let mut objects = media;
        let mut refs = Vec::new();
        let mut record_namespaces = BTreeMap::new();
        let mut descriptors = BTreeMap::new();
        for envelope in &records {
            envelope.validate()?;
            let bytes = serde_json::to_vec(envelope)?;
            let sha256 = digest(&bytes);
            record_namespaces.insert(sha256.clone(), envelope.namespace);
            refs.push(ObjectRef {
                sha256: sha256.clone(),
                bytes: bytes.len() as u64,
            });
            objects.insert(sha256, bytes);
            for item in &envelope.media_descriptors {
                descriptors.insert(item.sha256.clone(), item.clone());
            }
        }
        let manifest = Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::LocalTransaction,
            records: refs,
            record_namespaces,
            media: descriptors
                .values()
                .map(|m| ObjectRef {
                    sha256: m.sha256.clone(),
                    bytes: m.bytes,
                })
                .collect(),
            media_coverage: descriptors
                .keys()
                .map(|h| Coverage::Included { sha256: h.clone() })
                .collect(),
        };
        self.publish(manifest, objects, true)
    }
    pub fn import(&self, manifest: Manifest, objects: BTreeMap<String, Vec<u8>>) -> Result<Uuid> {
        self.publish(manifest, objects, false)
    }
    fn publish(
        &self,
        manifest: Manifest,
        objects: BTreeMap<String, Vec<u8>>,
        local: bool,
    ) -> Result<Uuid> {
        manifest.validate()?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let root = self.generation(inner.generation);
        if let Some(previous) = inner.index.manifests.get(&manifest.transaction_id) {
            ensure!(previous == &manifest, "transaction identity collision");
            return Ok(manifest.transaction_id);
        }
        Self::trip(&mut inner, Fault::BeforeObjects)?;
        for item in manifest.records.iter().chain(&manifest.media) {
            if let Some(bytes) = objects.get(&item.sha256) {
                ensure!(bytes.len() as u64 == item.bytes, "object size mismatch");
                files::object(
                    &root.join("objects").join(&item.sha256),
                    &item.sha256,
                    bytes,
                )?;
            }
        }
        Self::trip(&mut inner, Fault::AfterObjects)?;
        let records = Self::validate_objects(&root, &manifest)?;
        Self::can_apply(&inner.index, &records)?;
        if local {
            for (e, hash) in &records {
                if inner.index.operations.get(&e.operation_id) == Some(hash) {
                    continue;
                }
                let heads = inner
                    .index
                    .heads
                    .get(&(e.namespace, e.record_id))
                    .cloned()
                    .unwrap_or_default();
                if heads != e.parents {
                    return Err(Conflict { heads }.into());
                }
            }
            if records
                .iter()
                .all(|(e, h)| inner.index.operations.get(&e.operation_id) == Some(h))
            {
                if let Some(prior) = inner
                    .index
                    .manifests
                    .values()
                    .find(|m| {
                        m.records.iter().map(|r| &r.sha256).collect::<BTreeSet<_>>()
                            == manifest
                                .records
                                .iter()
                                .map(|r| &r.sha256)
                                .collect::<BTreeSet<_>>()
                            && m.media == manifest.media
                    })
                    .map(|m| m.transaction_id)
                {
                    return Ok(prior);
                }
            }
        }
        Self::trip(&mut inner, Fault::BeforeCommit)?;
        files::atomic_json(
            &root
                .join("commits")
                .join(format!("{}.json", manifest.transaction_id)),
            &manifest,
        )?;
        let after_commit = inner.fault == Fault::AfterCommit;
        Self::apply_index(&mut inner.index, manifest.clone(), records);
        if after_commit {
            Self::trip(&mut inner, Fault::AfterCommit)?;
        }
        drop(inner);
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                let _ = wake.try_send(());
            }
        }
        Ok(manifest.transaction_id)
    }
    pub fn set_wake(&self, wake: Option<SyncSender<()>>) {
        if let Ok(mut target) = self.wake.lock() {
            *target = wake;
        }
    }
    pub fn manifests(&self) -> Result<Vec<Manifest>> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?
            .index
            .manifests
            .values()
            .cloned()
            .collect())
    }
    pub fn read_object(&self, reference: &ObjectRef) -> Result<Vec<u8>> {
        let mut source = self.open_object(reference)?;
        source.chunk(0, source.len())
    }
    pub fn open_object(&self, reference: &ObjectRef) -> Result<files::Source> {
        ensure!(
            valid_digest(&reference.sha256) && reference.bytes <= MAX_MEDIA,
            "invalid object reference"
        );
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            inner.index.manifests.values().any(|m| m
                .records
                .iter()
                .chain(&m.media)
                .any(|r| r == reference)),
            "object is not reachable from a committed manifest"
        );
        let path = self
            .generation(inner.generation)
            .join("objects")
            .join(&reference.sha256);
        drop(inner);
        files::Source::open(&path, reference)
    }
    /// Stage streaming media; it is invisible until a subsequent commit references it.
    pub fn stage_blob(&self, reference: &ObjectRef, reader: impl std::io::Read) -> Result<()> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        files::copy_verified(
            &self
                .generation(inner.generation)
                .join("objects")
                .join(&reference.sha256),
            reader,
            reference,
        )
    }
    pub fn import_staged(&self, manifest: Manifest, incoming: &Path) -> Result<Uuid> {
        manifest.validate()?;
        for reference in manifest.records.iter().chain(&manifest.media) {
            if self.open_object(reference).is_ok() {
                continue;
            }
            let path = incoming.join(&reference.sha256);
            files::safe_path(&path)?;
            self.stage_blob(reference, File::open(path)?)?;
        }
        self.import(manifest, BTreeMap::new())
    }
}
