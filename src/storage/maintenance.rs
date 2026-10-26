//! Open an already owned store without initializing or repairing its structure.
use super::{files, Fault, Identity, Inner, Lease, Store, StorePaths, Uuid, FORMAT, MAX_RECORD};
use serde::de::DeserializeOwned;
use std::{fs, fs::OpenOptions, path::Path, sync::Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Unavailable,
    Unsupported,
    Busy,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "existing store unavailable",
            Self::Unsupported => "existing store unsupported",
            Self::Busy => "store is owned by another process",
        })
    }
}
impl std::error::Error for Refusal {}

fn directory(path: &Path) -> Result<(), Refusal> {
    files::safe_path(path).map_err(|_| Refusal::Unsupported)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| Refusal::Unavailable)?;
    if !metadata.is_dir() {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}

fn record<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<T, Refusal> {
    files::safe_path(path).map_err(|_| Refusal::Unsupported)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| Refusal::Unavailable)?;
    if !metadata.is_file() {
        return Err(Refusal::Unsupported);
    }
    files::json(path, limit).map_err(|_| Refusal::Unsupported)
}

impl Store {
    /// Retains one lease through caller maintenance. Does not create directories,
    /// repair markers, clean staging, publish capabilities or initialize a store.
    pub fn open_existing(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, false)
    }
    /// Read-only handwriting context custody. Validate existing formats so the
    /// ordinary/local-domain leases can refuse selected state, never initialize,
    /// repair, clean staging, publish capabilities or acquire mutation authority.
    pub(crate) fn open_existing_for_handwriting(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, true)
    }
    /// Opt-in mode maintenance only. Preserve legacy callers' format refusal.
    pub(crate) fn open_existing_for_mode(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, true)
    }
    /// Selected archive custody only; no initialization or restore admission.
    pub(crate) fn open_existing_for_archive(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, true)
    }
    /// Explicit configuration publication only; never initialize or activate a mode.
    pub(crate) fn open_existing_for_config(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, true)
    }
    pub(super) fn open_existing_for_conversation(
        paths: StorePaths,
        admit: impl FnOnce(&StorePaths, &std::fs::File) -> anyhow::Result<()>,
        recover: impl FnOnce(&Store, Uuid) -> anyhow::Result<super::Index>,
    ) -> Result<Self, Refusal> {
        Self::open_existing_using(paths, false, admit, recover)
    }
    fn open_existing_inner(paths: StorePaths, selected_aware: bool) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            selected_aware,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    fn open_existing_using(
        paths: StorePaths,
        selected_aware: bool,
        admit: impl FnOnce(&StorePaths, &std::fs::File) -> anyhow::Result<()>,
        recover: impl FnOnce(&Store, Uuid) -> anyhow::Result<super::Index>,
    ) -> Result<Self, Refusal> {
        paths.validate().map_err(|_| Refusal::Unsupported)?;
        directory(&paths.data)?;
        let lock_path = paths.data.join("store.lock");
        files::safe_path(&lock_path).map_err(|_| Refusal::Unsupported)?;
        let metadata = fs::symlink_metadata(&lock_path).map_err(|_| Refusal::Unavailable)?;
        if !metadata.is_file() {
            return Err(Refusal::Unsupported);
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|_| Refusal::Unavailable)?;
        lease.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => Refusal::Busy,
            fs::TryLockError::Error(_) => Refusal::Unsupported,
        })?;
        // Unlock even if validation fails before a Store can be constructed.
        let lease = Lease(lease);
        admit(&paths, &lease.0).map_err(|_| Refusal::Unsupported)?;
        let identity: Identity = record(&paths.data.join("identity.json"), MAX_RECORD as u64)?;
        if identity.format != FORMAT || identity.actor.is_nil() {
            return Err(Refusal::Unsupported);
        }
        for root in [&paths.cache, &paths.credentials] {
            directory(root)?;
            let owner: Identity = record(&root.join("owner.json"), MAX_RECORD as u64)?;
            if owner.actor != identity.actor || owner.format != FORMAT {
                return Err(Refusal::Unsupported);
            }
        }
        for child in ["generations", "quarantine", "sync"] {
            directory(&paths.data.join(child))?;
        }
        let generation: Uuid = record(&paths.data.join("CURRENT"), 128)?;
        if generation.is_nil() {
            return Err(Refusal::Unsupported);
        }
        let generation_root = paths.data.join("generations").join(generation.to_string());
        for path in [
            &generation_root,
            &generation_root.join("objects"),
            &generation_root.join("commits"),
        ] {
            directory(path)?;
        }
        if selected_aware {
            super::selection::validate_generation(&generation_root, identity.actor, generation)
                .map_err(|_| Refusal::Unsupported)?;
        } else if record::<u32>(&generation_root.join("format.json"), 64)? != FORMAT {
            return Err(Refusal::Unsupported);
        }
        let store = Self {
            paths,
            actor_id: identity.actor,
            _lease: lease,
            inner: Mutex::new(Inner {
                generation,
                index: Default::default(),
                fault: Fault::None,
                legacy_sync_handles: 0,
                mode_handles: 0,
                legacy_publisher_handles: 0,
                local_domain_handles: 0,
            }),
            wake: Mutex::new(None),
            export_active: std::sync::atomic::AtomicBool::new(false),
            writer_active: std::sync::atomic::AtomicBool::new(false),
        };
        let index = recover(&store, generation).map_err(|_| Refusal::Unsupported)?;
        if index.unavailable != 0 {
            return Err(Refusal::Unsupported);
        }
        store.inner.lock().map_err(|_| Refusal::Unavailable)?.index = index;
        Ok(store)
    }
}
