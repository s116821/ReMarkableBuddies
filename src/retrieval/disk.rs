//! Disposable local normalized inputs, never durable evidence or source authority.
use super::{Index, IndexKey, NormalizedDocument};
use crate::storage::{files, Store, Uuid, FORMAT};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::Mutex,
};

const SLOTS: usize = 16;
const ENTRY_BYTES: u64 = 32 * 1024 * 1024;
const KIND: &str = "normalized-sources-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreOwner {
    format: u32,
    actor: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    format: u32,
    actor: Uuid,
    kind: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    format: u32,
    key: IndexKey,
    document: NormalizedDocument,
}

struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Holds both the namespace lock and a borrow of the already-open Store lease.
/// Opening may initialize owned files and clean abandoned owned staging files.
pub struct DiskCache<'a> {
    root: PathBuf,
    _lease: Lease,
    _store: &'a Store,
    operations: Mutex<()>,
}

fn document_name(name: &str) -> Option<Uuid> {
    let raw = name.strip_suffix(".json")?;
    let id = Uuid::parse_str(raw).ok()?;
    (!id.is_nil() && id.to_string() == raw).then_some(id)
}

impl<'a> DiskCache<'a> {
    pub fn open(store: &'a Store) -> Result<Self> {
        let parent: StoreOwner = files::json(&store.paths.cache.join("owner.json"), 1024)?;
        ensure!(
            parent.format == FORMAT && parent.actor == store.actor_id,
            "Cache root ownership mismatch"
        );
        let root = store.paths.cache.join(KIND);
        files::safe_path(&root)?;
        let marker = root.join("owner.json");
        if root.exists() && !marker.exists() {
            ensure!(
                fs::read_dir(&root)?.next().is_none(),
                "Unowned populated source cache"
            );
        }
        files::directory(&root)?;
        if marker.exists() {
            let owner: Owner = files::json(&marker, 1024)?;
            ensure!(
                owner.format == 1 && owner.actor == store.actor_id && owner.kind == KIND,
                "Source cache ownership mismatch"
            );
        } else {
            files::create_identity(
                &marker,
                &Owner {
                    format: 1,
                    actor: store.actor_id,
                    kind: KIND.to_owned(),
                },
            )?;
        }
        let lock = root.join("cache.lock");
        files::safe_path(&lock)?;
        if lock.exists() {
            ensure!(
                fs::symlink_metadata(&lock)?.is_file(),
                "Invalid source cache lock"
            );
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&lock)?;
        file.try_lock()
            .map_err(|_| anyhow::anyhow!("Source cache is busy"))?;
        let cache = Self {
            root,
            _lease: Lease(file),
            _store: store,
            operations: Mutex::new(()),
        };
        cache.slots()?;
        files::cleanup_staging(&cache.root)?;
        Ok(cache)
    }

    fn path(&self, document: Uuid) -> Result<PathBuf> {
        ensure!(!document.is_nil(), "Missing cached document identity");
        let path = self.root.join(format!("{document}.json"));
        files::safe_path(&path)?;
        Ok(path)
    }

    fn slots(&self) -> Result<BTreeSet<Uuid>> {
        files::safe_path(&self.root)?;
        let mut slots = BTreeSet::new();
        let mut stages = 0;
        for (count, entry) in fs::read_dir(&self.root)?.enumerate() {
            ensure!(
                count < SLOTS * 2 + 2,
                "Source cache directory exceeds bound"
            );
            let entry = entry?;
            files::safe_path(&entry.path())?;
            ensure!(
                entry.file_type()?.is_file(),
                "Unexpected source cache entry type"
            );
            let name = entry.file_name();
            let name = name.to_str().context("Invalid source cache entry name")?;
            if matches!(name, "owner.json" | "cache.lock") {
                continue;
            }
            if let Some(raw) = name.strip_prefix(".stage-") {
                let id = Uuid::parse_str(raw).ok();
                ensure!(
                    id.is_some_and(|id| id.to_string() == raw) && stages < SLOTS,
                    "Unknown source cache staging entry"
                );
                stages += 1;
                continue;
            }
            let id = document_name(name).context("Unknown source cache entry")?;
            slots.insert(id);
            ensure!(slots.len() <= SLOTS, "Source cache capacity exceeded");
        }
        Ok(slots)
    }

    /// Validate and encode completely before atomically replacing one derived slot.
    /// A directory-sync error after rename does not imply the old slot survived.
    pub fn install(&self, document: NormalizedDocument) -> Result<IndexKey> {
        let index = Index::build(document.clone())?;
        let key = index.key().clone();
        let bytes = serde_json::to_vec(&Entry {
            format: 1,
            key: key.clone(),
            document,
        })?;
        ensure!(
            bytes.len() as u64 <= ENTRY_BYTES,
            "Encoded source cache entry exceeds bound"
        );
        let _operation = self
            .operations
            .lock()
            .map_err(|_| anyhow::anyhow!("Source cache lock unavailable"))?;
        let slots = self.slots()?;
        ensure!(
            slots.contains(&key.document) || slots.len() < SLOTS,
            "Source cache capacity exceeded"
        );
        files::atomic(&self.path(key.document)?, &bytes)?;
        Ok(key)
    }

    /// Expected identity must come from the caller's selected source, not this cache.
    pub fn load(&self, expected: &IndexKey) -> Result<Option<Index>> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| anyhow::anyhow!("Source cache lock unavailable"))?;
        self.slots()?;
        let path = self.path(expected.document)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => anyhow::bail!("Cached source unavailable"),
            Ok(metadata) => ensure!(metadata.is_file(), "Invalid cached source entry"),
        }
        let entry: Entry = files::json(&path, ENTRY_BYTES)?;
        ensure!(
            entry.format == 1 && entry.key == *expected,
            "Stale or unsupported cached source"
        );
        let index = Index::build(entry.document)?;
        ensure!(index.key() == expected, "Cached source identity mismatch");
        Ok(Some(index))
    }

    /// Delete only one selected owned derived slot; never source or durable data.
    pub fn forget(&self, document: Uuid) -> Result<bool> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| anyhow::anyhow!("Source cache lock unavailable"))?;
        let path = self.path(document)?;
        if !self.slots()?.contains(&document) {
            return Ok(false);
        }
        fs::remove_file(path)?;
        files::sync_dir(&self.root)?;
        Ok(true)
    }
}

/// Cleanup admission validates material without asserting independently selected freshness.
#[cfg(target_os = "linux")]
pub(crate) fn validate_cleanup_entry(bytes: &[u8], document: Uuid) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= ENTRY_BYTES,
        "Cache entry exceeds bound"
    );
    let raw = crate::config::inspection::strict_json(bytes)?;
    ensure!(raw.is_object(), "Cache wrapper must be an object");
    ensure!(
        raw.get("key").is_some_and(|v| v.is_object())
            && raw.get("document").is_some_and(|v| v.is_object()),
        "Cache fields must be objects"
    );
    ensure!(
        raw.pointer("/document/units")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|units| units.iter().all(|u| u.is_object())),
        "Cache units must be objects"
    );
    let entry: Entry = serde_json::from_value(raw)?;
    ensure!(
        entry.format == 1 && entry.key.document == document,
        "Invalid cache wrapper"
    );
    let index = Index::build(entry.document)?;
    ensure!(index.key() == &entry.key, "Invalid derived cache material");
    Ok(())
}
