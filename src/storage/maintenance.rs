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

fn read_record<T: DeserializeOwned>(
    path: &Path,
    limit: u64,
    budget: Option<&mut super::selection::SelectedReadBudget>,
) -> Result<T, Refusal> {
    if let Some(budget) = budget {
        files::safe_path(path).map_err(|_| Refusal::Unsupported)?;
        let meta = fs::symlink_metadata(path).map_err(|_| Refusal::Unavailable)?;
        if !meta.is_file() {
            return Err(Refusal::Unsupported);
        }
        let bytes =
            files::read_with_budget(path, limit, Some(budget)).map_err(|_| Refusal::Unsupported)?;
        serde_json::from_slice(&bytes).map_err(|_| Refusal::Unsupported)
    } else {
        record(path, limit)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NotebookAdmission {
    Deny,
    Restore,
    Preferences,
    ReadyOrdinary,
    SelectedMode,
    Handwriting,
}

// A definite empty regular directory or definite absence; never follow links.
fn empty_if_present(path: &Path) -> Result<(), Refusal> {
    files::safe_path(path).map_err(|_| Refusal::Unsupported)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            if fs::read_dir(path)
                .map_err(|_| Refusal::Unsupported)?
                .next()
                .transpose()
                .map_err(|_| Refusal::Unsupported)?
                .is_none()
            {
                Ok(())
            } else {
                Err(Refusal::Unsupported)
            }
        }
        Ok(_) => Err(Refusal::Unsupported),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Refusal::Unsupported),
    }
}

pub(crate) struct ExistingCustody {
    pub(super) paths: StorePaths,
    pub(super) actor: Uuid,
    pub(super) generation: Uuid,
    pub(super) lease: Lease,
}

impl ExistingCustody {
    pub(super) fn open(
        paths: StorePaths,
        admit: impl FnOnce(&StorePaths, &std::fs::File) -> anyhow::Result<()>,
        mut budget: Option<&mut super::selection::SelectedReadBudget>,
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
        let identity: Identity = read_record(
            &paths.data.join("identity.json"),
            MAX_RECORD as u64,
            budget.as_deref_mut(),
        )?;
        if identity.format != FORMAT || identity.actor.is_nil() {
            return Err(Refusal::Unsupported);
        }
        for root in [&paths.cache, &paths.credentials] {
            directory(root)?;
            let owner: Identity = read_record(
                &root.join("owner.json"),
                MAX_RECORD as u64,
                budget.as_deref_mut(),
            )?;
            if owner.actor != identity.actor || owner.format != FORMAT {
                return Err(Refusal::Unsupported);
            }
        }
        for child in ["generations", "quarantine", "sync"] {
            directory(&paths.data.join(child))?;
        }
        let generation: Uuid = read_record(&paths.data.join("CURRENT"), 128, budget)?;
        if generation.is_nil() {
            return Err(Refusal::Unsupported);
        }
        Ok(Self {
            paths,
            actor: identity.actor,
            generation,
            lease,
        })
    }
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
        Self::open_existing_using(
            paths,
            true,
            NotebookAdmission::Handwriting,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    /// Opt-in mode maintenance only. Preserve legacy callers' format refusal.
    pub(crate) fn open_existing_for_mode(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            true,
            NotebookAdmission::SelectedMode,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    /// Selected archive custody only; no initialization or restore admission.
    pub(crate) fn open_existing_for_archive(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, true)
    }
    /// Historical selected-outbox custody only; no initialization or repair.
    pub(crate) fn open_existing_for_outbox(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_inner(paths, true)
    }
    /// Protected Linux credential-file publication only; never initialize or activate a mode.
    #[cfg(target_os = "linux")]
    pub(crate) fn open_existing_for_credential_file(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            true,
            NotebookAdmission::Preferences,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    pub(super) fn open_existing_for_conversation(
        paths: StorePaths,
        admit: impl FnOnce(&StorePaths, &std::fs::File) -> anyhow::Result<()>,
        recover: impl FnOnce(&Store, Uuid) -> anyhow::Result<super::Index>,
    ) -> Result<Self, Refusal> {
        Self::open_existing_using(paths, false, NotebookAdmission::Deny, admit, recover)
    }
    fn open_existing_inner(paths: StorePaths, selected_aware: bool) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            selected_aware,
            NotebookAdmission::Deny,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    pub(crate) fn open_existing_for_notebook_restore(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            false,
            NotebookAdmission::Restore,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    /// Nonsecret basic/model/memory/product/research declaration writers may admit
    /// a ready ordinary notebook. Other writers use their own scoped entries.
    pub(crate) fn open_existing_for_preferences(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            true,
            NotebookAdmission::Preferences,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    /// Saved reference declarations only; protected file operations use a separate entry.
    pub(crate) fn open_existing_for_credential_reference(
        paths: StorePaths,
    ) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            true,
            NotebookAdmission::Preferences,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    /// Personalization retains legacy format1-only admission. The ordinary
    /// notebook gate is shared, without admitting legacy selected/feedback stores.
    pub(crate) fn open_existing_for_personalization(paths: StorePaths) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            false,
            NotebookAdmission::Preferences,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    /// Portable preferences use the common legacy1/ordinary notebook family only.
    pub(crate) fn open_existing_for_portable_preferences(
        paths: StorePaths,
    ) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            false,
            NotebookAdmission::Preferences,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }
    pub(crate) fn open_existing_for_ready_notebook_receiving(
        paths: StorePaths,
    ) -> Result<Self, Refusal> {
        Self::open_existing_using(
            paths,
            false,
            NotebookAdmission::ReadyOrdinary,
            |_, _| Ok(()),
            |store, generation| store.recover(generation),
        )
    }

    fn open_existing_using(
        paths: StorePaths,
        selected_aware: bool,
        admission: NotebookAdmission,
        admit: impl FnOnce(&StorePaths, &std::fs::File) -> anyhow::Result<()>,
        recover: impl FnOnce(&Store, Uuid) -> anyhow::Result<super::Index>,
    ) -> Result<Self, Refusal> {
        let ExistingCustody {
            paths,
            actor,
            generation,
            lease,
        } = ExistingCustody::open(paths, admit, None)?;
        let generation_root = paths.data.join("generations").join(generation.to_string());
        files::safe_path(&generation_root.join(super::notebook_generation::MARKER))
            .map_err(|_| Refusal::Unsupported)?;
        let notebook = admission == NotebookAdmission::Restore
            || record::<u32>(&generation_root.join("format.json"), 64)?
                == super::notebook_generation::VERSION
            || generation_root
                .join(super::notebook_generation::MARKER)
                .exists();
        if admission == NotebookAdmission::ReadyOrdinary && !notebook {
            return Err(Refusal::Unsupported);
        }
        if admission == NotebookAdmission::Deny && notebook {
            // These are legacy maintenance admissions. Explicit notebook
            // preparation/history/backup use their separately validated APIs.
            return Err(Refusal::Unsupported);
        }
        if admission == NotebookAdmission::SelectedMode
            && notebook
            && super::target_generation::base_version(&generation_root)
                .map_err(|_| Refusal::Unsupported)?
                != super::compound_generation::VERSION
        {
            return Err(Refusal::Unsupported);
        }
        for path in [
            &generation_root,
            &generation_root.join("objects"),
            &generation_root.join("commits"),
        ] {
            directory(path)?;
        }
        if super::target_generation::base_version(&generation_root)
            .map_err(|_| Refusal::Unsupported)?
            == super::compound_generation::VERSION
        {
            if admission != NotebookAdmission::SelectedMode {
                return Err(Refusal::Unsupported);
            }
            super::compound_generation::validate_ready(&paths, actor, generation)
                .map_err(|_| Refusal::Unsupported)?;
        } else if notebook {
            super::notebook_generation::validate_ready(&paths, actor, generation)
                .map_err(|_| Refusal::Unsupported)?;
        } else if selected_aware {
            super::selection::validate_generation(&generation_root, actor, generation)
                .map_err(|_| Refusal::Unsupported)?;
        } else if record::<u32>(&generation_root.join("format.json"), 64)? != FORMAT {
            return Err(Refusal::Unsupported);
        }
        let notebook_preferences = notebook
            && matches!(
                admission,
                NotebookAdmission::Preferences
                    | NotebookAdmission::ReadyOrdinary
                    | NotebookAdmission::Handwriting
            );
        if notebook_preferences {
            empty_if_present(&generation_root.join("selections"))?;
            empty_if_present(&generation_root.join("selection-history"))?;
        }
        let store = Self {
            paths,
            actor_id: actor,
            runtime_policy: None,
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
            selected_checkpoint: std::sync::RwLock::new(()),
            export_active: std::sync::atomic::AtomicBool::new(false),
            writer_active: std::sync::atomic::AtomicBool::new(false),
        };
        let index = recover(&store, generation).map_err(|_| Refusal::Unsupported)?;
        if index.unavailable != 0
            || (notebook_preferences
                && index
                    .manifests
                    .values()
                    .any(|manifest| manifest.scope != super::Scope::LocalTransaction))
        {
            return Err(Refusal::Unsupported);
        }
        store.inner.lock().map_err(|_| Refusal::Unavailable)?.index = index;
        if notebook_preferences
            && !super::sync::mode::ordinary_local(&store).map_err(|_| Refusal::Unsupported)?
        {
            return Err(Refusal::Unsupported);
        }
        Ok(store)
    }
}
