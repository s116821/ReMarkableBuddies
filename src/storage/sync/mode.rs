//! Private mode choice/lifetime control. No provider/bootstrap/domain authority.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SyncModeChoice {
    LocalOnly,
    IsolatedBackup {
        binding_sha256: String,
    },
    SharedGroup {
        group: Uuid,
        binding_sha256: String,
        root_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SyncMode {
    LocalOnly,
    IsolatedBackup {
        backup: Uuid,
        binding_sha256: String,
    },
    SharedGroup {
        group: Uuid,
        binding_sha256: String,
        root_id: String,
    },
}
impl SyncMode {
    fn choice(&self) -> SyncModeChoice {
        match self {
            Self::LocalOnly => SyncModeChoice::LocalOnly,
            Self::IsolatedBackup { binding_sha256, .. } => SyncModeChoice::IsolatedBackup {
                binding_sha256: binding_sha256.clone(),
            },
            Self::SharedGroup {
                group,
                binding_sha256,
                root_id,
            } => SyncModeChoice::SharedGroup {
                group: *group,
                binding_sha256: binding_sha256.clone(),
                root_id: root_id.clone(),
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SyncModeToken {
    actor: Uuid,
    sha256: String,
}
impl SyncModeToken {
    /// Decode an operator's original conditional precondition, never a live lease.
    pub(crate) fn from_maintenance_request(actor: Uuid, sha256: String) -> Result<Self> {
        ensure!(
            !actor.is_nil() && valid_digest(&sha256),
            "invalid mode precondition"
        );
        Ok(Self { actor, sha256 })
    }
}
#[derive(Clone)]
pub struct SyncModeSnapshot {
    pub device: Option<Uuid>,
    pub epoch: Option<Uuid>,
    /// None means existing unconfigured legacy compatibility, never shared mode.
    pub mode: Option<SyncMode>,
    pub token: SyncModeToken,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncModeReceipt {
    pub operation: Uuid,
    pub epoch: Uuid,
    pub mode: SyncMode,
    expected: String,
    request_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModeState {
    format: u32,
    actor: Uuid,
    device: Uuid,
    transitions: Vec<SyncModeReceipt>,
}
fn fresh_identity(state: &ModeState) -> Uuid {
    loop {
        let id = Uuid::new_v4();
        if id != state.actor
            && id != state.device
            && state.transitions.iter().all(|r| {
                id != r.epoch
                    && match r.mode {
                        SyncMode::IsolatedBackup { backup, .. } => id != backup,
                        SyncMode::SharedGroup { group, .. } => id != group,
                        SyncMode::LocalOnly => true,
                    }
            })
        {
            return id;
        }
    }
}
fn path(store: &Store) -> PathBuf {
    store.paths.data.join("sync/mode.json")
}
pub(crate) fn validate_choice(choice: &SyncModeChoice) -> Result<()> {
    match choice {
        SyncModeChoice::LocalOnly => (),
        SyncModeChoice::IsolatedBackup { binding_sha256 } => {
            ensure!(valid_digest(binding_sha256), "invalid isolated binding")
        }
        SyncModeChoice::SharedGroup {
            group,
            binding_sha256,
            root_id,
        } => {
            ensure!(
                !group.is_nil() && valid_digest(binding_sha256),
                "invalid shared mode binding"
            );
            id(root_id)?;
        }
    }
    Ok(())
}
pub(crate) fn load(store: &Store) -> Result<Option<ModeState>> {
    let path = path(store);
    files::safe_path(&path)?;
    if !path.exists() {
        return Ok(None);
    }
    let state: ModeState = files::json(&path, MAX_METADATA as u64)?;
    ensure!(
        state.format == FORMAT
            && state.actor == store.actor_id
            && !state.device.is_nil()
            && state.device != state.actor
            && !state.transitions.is_empty()
            && state.transitions.len() <= MAX_ITEMS,
        "foreign or unsupported sync mode"
    );
    let mut operations = BTreeSet::new();
    let mut epochs = BTreeSet::new();
    let backups: BTreeSet<_> = state
        .transitions
        .iter()
        .filter_map(|r| match r.mode {
            SyncMode::IsolatedBackup { backup, .. } => Some(backup),
            _ => None,
        })
        .collect();
    let mut seen_backups = BTreeSet::new();
    let mut previous: Option<&SyncMode> = None;
    for receipt in &state.transitions {
        ensure!(
            !receipt.operation.is_nil()
                && !receipt.epoch.is_nil()
                && operations.insert(receipt.operation)
                && epochs.insert(receipt.epoch)
                && valid_digest(&receipt.expected),
            "invalid mode history identity"
        );
        let choice = receipt.mode.choice();
        validate_choice(&choice)?;
        ensure!(
            receipt.request_sha256
                == digest(&serde_json::to_vec(&(
                    state.actor,
                    &receipt.expected,
                    &choice
                ))?),
            "mode request identity mismatch"
        );
        match &receipt.mode {
            SyncMode::IsolatedBackup {
                backup,
                binding_sha256,
            } => {
                ensure!(
                    !backup.is_nil() && *backup != state.actor && *backup != state.device,
                    "invalid backup identity"
                );
                match previous {
                    Some(SyncMode::IsolatedBackup {
                        backup: old,
                        binding_sha256: binding,
                    }) if binding == binding_sha256 => {
                        ensure!(old == backup, "same-binding backup identity discontinuity")
                    }
                    _ => ensure!(
                        seen_backups.insert(*backup),
                        "reused historical backup identity"
                    ),
                }
            }
            SyncMode::SharedGroup { group, .. } => ensure!(
                *group != state.actor && *group != state.device && !backups.contains(group),
                "mode namespace identity collision"
            ),
            SyncMode::LocalOnly => (),
        }
        previous = Some(&receipt.mode);
    }
    Ok(Some(state))
}
fn snapshot(store: &Store, state: &Option<ModeState>) -> Result<SyncModeSnapshot> {
    let current = state.as_ref().and_then(|s| s.transitions.last());
    Ok(SyncModeSnapshot {
        device: state.as_ref().map(|s| s.device),
        epoch: current.map(|r| r.epoch),
        mode: current.map(|r| r.mode.clone()),
        token: SyncModeToken {
            actor: store.actor_id,
            sha256: digest(&serde_json::to_vec(&(store.actor_id, state))?),
        },
    })
}
pub(crate) fn shared_epoch(
    store: &Store,
    scope: &selection::SelectionScope,
) -> Result<Option<Uuid>> {
    let state = load(store)?;
    let Some(current) = state.as_ref().and_then(|s| s.transitions.last()) else {
        return Ok(None);
    };
    match &current.mode {
        SyncMode::SharedGroup {
            group,
            binding_sha256,
            ..
        } => {
            ensure!(
                *group == scope.group && binding_sha256 == &scope.binding_sha256,
                "outbox current mode binding mismatch"
            );
            Ok(Some(current.epoch))
        }
        _ => anyhow::bail!("selected shared intent requires current shared mode"),
    }
}
/// Pins absent or LocalOnly mode for an ordinary application's lifetime.
/// This supplies no data, publisher, selected-domain, provider or native authority.
#[must_use = "retain the ordinary mode guard through all admitted consumers"]
pub struct OrdinaryModeLease {
    store: Arc<Store>,
}
impl Drop for OrdinaryModeLease {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.store.inner.lock() {
            inner.mode_handles -= 1;
        }
    }
}

/// Holds only a mode lifetime. Provider/root qualification and actual domain
/// admission remain required. Dropping/stopping publisher releases this lease.
pub struct SyncModeLease {
    store: Arc<Store>,
    current: SyncModeSnapshot,
}
impl SyncModeLease {
    pub fn snapshot(&self) -> &SyncModeSnapshot {
        &self.current
    }
    pub fn require_shared_scope(&self, scope: &selection::SelectionScope) -> Result<()> {
        scope.validate()?;
        let _inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            shared_epoch(&self.store, scope)? == self.current.epoch,
            "mode lease is not current shared binding"
        );
        Ok(())
    }
}
impl Drop for SyncModeLease {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.store.inner.lock() {
            inner.mode_handles -= 1;
        }
    }
}
impl Store {
    pub fn sync_mode_snapshot(&self) -> Result<SyncModeSnapshot> {
        let _inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        snapshot(self, &load(self)?)
    }
    /// Explicit choice only; original replay returns history, not a current token.
    pub fn set_sync_mode(
        &self,
        expected: &SyncModeToken,
        operation: Uuid,
        choice: SyncModeChoice,
    ) -> Result<SyncModeReceipt> {
        ensure!(
            !operation.is_nil() && expected.actor == self.actor_id,
            "invalid mode request actor/operation"
        );
        validate_choice(&choice)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let original = load(self)?;
        let fingerprint = digest(&serde_json::to_vec(&(
            self.actor_id,
            &expected.sha256,
            &choice,
        ))?);
        if let Some(receipt) = original
            .as_ref()
            .and_then(|s| s.transitions.iter().find(|r| r.operation == operation))
        {
            ensure!(
                receipt.request_sha256 == fingerprint,
                "mode operation collision"
            );
            return Ok(receipt.clone());
        }
        ensure!(
            snapshot(self, &original)?.token == *expected,
            "stale mode token"
        );
        ensure!(
            inner.legacy_publisher_handles == 0 && inner.mode_handles == 0,
            "stop existing publisher leases before mode change"
        );
        let mut state = original.unwrap_or_else(|| ModeState {
            format: FORMAT,
            actor: self.actor_id,
            device: loop {
                let id = Uuid::new_v4();
                if id != self.actor_id {
                    break id;
                }
            },
            transitions: vec![],
        });
        let mode = match choice {
            SyncModeChoice::LocalOnly => SyncMode::LocalOnly,
            SyncModeChoice::IsolatedBackup { binding_sha256 } => {
                let backup = match state.transitions.last().map(|r| &r.mode) {
                    Some(SyncMode::IsolatedBackup {
                        backup,
                        binding_sha256: old,
                    }) if *old == binding_sha256 => *backup,
                    _ => fresh_identity(&state),
                };
                SyncMode::IsolatedBackup {
                    backup,
                    binding_sha256,
                }
            }
            SyncModeChoice::SharedGroup {
                group,
                binding_sha256,
                root_id,
            } => SyncMode::SharedGroup {
                group,
                binding_sha256,
                root_id,
            },
        };
        let receipt = SyncModeReceipt {
            operation,
            epoch: fresh_identity(&state),
            mode,
            expected: expected.sha256.clone(),
            request_sha256: fingerprint,
        };
        state.transitions.push(receipt.clone());
        let bytes = serde_json::to_vec(&state)?;
        ensure!(
            state.transitions.len() <= MAX_ITEMS && bytes.len() <= MAX_METADATA,
            "mode history exceeds bound"
        );
        // Reuse all structural/namespace validation before publishing.
        let backups: BTreeSet<_> = state
            .transitions
            .iter()
            .filter_map(|r| match r.mode {
                SyncMode::IsolatedBackup { backup, .. } => Some(backup),
                _ => None,
            })
            .collect();
        ensure!(state.device != state.actor, "device/actor collision");
        if let SyncMode::SharedGroup { group, .. } = &receipt.mode {
            ensure!(
                *group != state.actor && *group != state.device && !backups.contains(group),
                "mode namespace identity collision"
            );
        }
        files::directory(&self.paths.data.join("sync"))?;
        Self::trip(&mut inner, Fault::BeforeCommit)?;
        files::atomic(&path(self), &bytes)?;
        Self::trip(&mut inner, Fault::AfterCommit)?;
        Ok(receipt)
    }
    /// Atomically admit an ordinary lifetime without broadening publisher leases.
    pub fn ordinary_mode_lease(
        self: &Arc<Self>,
        expected: &SyncModeToken,
    ) -> Result<OrdinaryModeLease> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let current = snapshot(self, &load(self)?)?;
        ensure!(
            current.token == *expected,
            "stale ordinary mode lease token"
        );
        ensure!(
            matches!(current.mode, None | Some(SyncMode::LocalOnly)),
            "ordinary mode requires absent or LocalOnly choice"
        );
        inner.mode_handles = inner
            .mode_handles
            .checked_add(1)
            .context("mode lease overflow")?;
        Ok(OrdinaryModeLease {
            store: self.clone(),
        })
    }
    pub fn sync_mode_lease(self: &Arc<Self>, expected: &SyncModeToken) -> Result<SyncModeLease> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let current = snapshot(self, &load(self)?)?;
        ensure!(current.token == *expected, "stale mode lease token");
        ensure!(
            matches!(
                current.mode,
                Some(SyncMode::IsolatedBackup { .. } | SyncMode::SharedGroup { .. })
            ),
            "explicit publisher mode required"
        );
        inner.mode_handles = inner
            .mode_handles
            .checked_add(1)
            .context("mode lease overflow")?;
        Ok(SyncModeLease {
            store: self.clone(),
            current,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("buddy-mode-{}", Uuid::new_v4())))
        }
        fn store(&self) -> Arc<Store> {
            Arc::new(
                Store::open(StorePaths {
                    data: self.0.join("data"),
                    cache: self.0.join("cache"),
                    credentials: self.0.join("secrets"),
                })
                .unwrap(),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn set(store: &Store, choice: SyncModeChoice) -> SyncModeReceipt {
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                choice,
            )
            .unwrap()
    }
    fn backup(receipt: &SyncModeReceipt) -> Uuid {
        match receipt.mode {
            SyncMode::IsolatedBackup { backup, .. } => backup,
            _ => panic!("expected isolated"),
        }
    }
    #[test]
    fn sync_mode_identities_replay_reentry_and_restart_are_separate() {
        let f = Fixture::new();
        let store = f.store();
        let before = store.sync_mode_snapshot().unwrap();
        assert!(before.mode.is_none());
        assert!(!path(&store).exists());
        let operation = Uuid::new_v4();
        let choice = SyncModeChoice::IsolatedBackup {
            binding_sha256: digest(b"account/app"),
        };
        let first = store
            .set_sync_mode(&before.token, operation, choice.clone())
            .unwrap();
        let device = store.sync_mode_snapshot().unwrap().device.unwrap();
        assert_ne!(device, store.actor_id);
        assert_ne!(backup(&first), device);
        assert_ne!(backup(&first), store.actor_id);
        assert_eq!(backup(&set(&store, choice.clone())), backup(&first));
        set(&store, SyncModeChoice::LocalOnly);
        let returned = set(&store, choice.clone());
        assert_ne!(backup(&returned), backup(&first));
        let changed = set(
            &store,
            SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"another account/app"),
            },
        );
        assert_ne!(backup(&changed), backup(&returned));
        let replay = store
            .set_sync_mode(&before.token, operation, choice)
            .unwrap();
        assert_eq!(replay.epoch, first.epoch);
        assert_eq!(backup(&replay), backup(&first));
        assert!(store
            .set_sync_mode(&before.token, operation, SyncModeChoice::LocalOnly)
            .is_err());
        assert!(store
            .set_sync_mode(&before.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
            .is_err());
        drop(store);
        let store = f.store();
        assert_eq!(store.sync_mode_snapshot().unwrap().device, Some(device));
        assert_eq!(
            store.sync_mode_snapshot().unwrap().epoch,
            Some(changed.epoch)
        );
    }
    #[test]
    fn sync_mode_leases_block_transition_but_do_not_disable_local_data() {
        let f = Fixture::new();
        let store = f.store();
        let old = store.sync_mode_snapshot().unwrap();
        let publisher = store.legacy_publisher_lease().unwrap();
        assert!(store
            .set_sync_mode(&old.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
            .is_err());
        drop(publisher);
        let domain = store.legacy_sync_lease().unwrap();
        set(&store, SyncModeChoice::LocalOnly);
        assert!(store.legacy_publisher_lease().is_err());
        assert!(store.legacy_sync_lease().is_ok());
        drop(domain);
        let group = Uuid::new_v4();
        let binding = digest(b"chosen binding");
        set(
            &store,
            SyncModeChoice::SharedGroup {
                group,
                binding_sha256: binding.clone(),
                root_id: "chosen-root".into(),
            },
        );
        let snapshot = store.sync_mode_snapshot().unwrap();
        let lease = store.sync_mode_lease(&snapshot.token).unwrap();
        let scope = selection::SelectionScope {
            group,
            key_sha256: digest(b"domain-owned key"),
            binding_sha256: binding,
        };
        lease.require_shared_scope(&scope).unwrap();
        let mut foreign = scope.clone();
        foreign.binding_sha256 = digest(b"different binding");
        assert!(lease.require_shared_scope(&foreign).is_err());
        assert!(store
            .set_sync_mode(&snapshot.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
            .is_err());
        drop(lease);
        store
            .set_sync_mode(&snapshot.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
            .unwrap();
        assert!(store.sync_mode_lease(&snapshot.token).is_err());
    }
    #[test]
    fn ordinary_mode_leases_pin_none_and_local_until_last_guard_drops() {
        for local in [false, true] {
            let f = Fixture::new();
            let store = f.store();
            if local {
                set(&store, SyncModeChoice::LocalOnly);
            }
            let current = store.sync_mode_snapshot().unwrap();
            let original = fs::read(path(&store)).ok();
            let first = store.ordinary_mode_lease(&current.token).unwrap();
            let second = store.ordinary_mode_lease(&current.token).unwrap();
            let data = store.legacy_sync_lease().unwrap();
            let choice = SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"ordinary transition"),
            };
            assert!(store
                .set_sync_mode(&current.token, Uuid::new_v4(), choice.clone())
                .is_err());
            assert_eq!(store.sync_mode_snapshot().unwrap().token, current.token);
            assert_eq!(fs::read(path(&store)).ok(), original);
            drop(first);
            assert!(store
                .set_sync_mode(&current.token, Uuid::new_v4(), choice.clone())
                .is_err());
            drop(second);
            // The independent ordinary-data handle remains usable and must not
            // become a mode fence as a side effect of the new guard.
            store
                .set_sync_mode(&current.token, Uuid::new_v4(), choice)
                .unwrap();
            drop(data);
        }
    }

    #[test]
    fn ordinary_mode_refuses_stale_foreign_nonlocal_and_malformed_without_leaking_fence() {
        let f = Fixture::new();
        let store = f.store();
        let foreign = Fixture::new();
        assert!(store
            .ordinary_mode_lease(&foreign.store().sync_mode_snapshot().unwrap().token)
            .is_err());
        let old = store.sync_mode_snapshot().unwrap().token;
        set(&store, SyncModeChoice::LocalOnly);
        assert!(store.ordinary_mode_lease(&old).is_err());
        for choice in [
            SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"ordinary foreign backup"),
            },
            SyncModeChoice::SharedGroup {
                group: Uuid::new_v4(),
                binding_sha256: digest(b"ordinary foreign group"),
                root_id: "chosen-root".into(),
            },
        ] {
            set(&store, choice);
            let current = store.sync_mode_snapshot().unwrap();
            let original = fs::read(path(&store)).unwrap();
            assert!(store.ordinary_mode_lease(&current.token).is_err());
            assert_eq!(fs::read(path(&store)).unwrap(), original);
            set(&store, SyncModeChoice::LocalOnly);
        }
        let current = store.sync_mode_snapshot().unwrap();
        let original = fs::read(path(&store)).unwrap();
        let broken = b"not a mode object";
        fs::write(path(&store), broken).unwrap();
        assert!(store.ordinary_mode_lease(&current.token).is_err());
        assert_eq!(fs::read(path(&store)).unwrap(), broken);
        fs::write(path(&store), original).unwrap();
        set(&store, SyncModeChoice::LocalOnly);
        let guard = store
            .ordinary_mode_lease(&store.sync_mode_snapshot().unwrap().token)
            .unwrap();
        drop(guard);
    }

    #[test]
    fn ordinary_mode_preserves_legacy_publisher_coexistence_and_nonmutating_replay() {
        for publisher_first in [false, true] {
            let f = Fixture::new();
            let store = f.store();
            let current = store.sync_mode_snapshot().unwrap();
            let mut publisher = publisher_first.then(|| store.legacy_publisher_lease().unwrap());
            let guard = store.ordinary_mode_lease(&current.token).unwrap();
            if !publisher_first {
                publisher = Some(store.legacy_publisher_lease().unwrap());
            }
            assert!(store
                .set_sync_mode(&current.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
                .is_err());
            drop(guard);
            assert!(store
                .set_sync_mode(&current.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
                .is_err());
            drop(publisher);
            let operation = Uuid::new_v4();
            let receipt = store
                .set_sync_mode(&current.token, operation, SyncModeChoice::LocalOnly)
                .unwrap();
            let local = store.sync_mode_snapshot().unwrap();
            let guard = store.ordinary_mode_lease(&local.token).unwrap();
            let original = fs::read(path(&store)).unwrap();
            assert_eq!(
                store
                    .set_sync_mode(&current.token, operation, SyncModeChoice::LocalOnly)
                    .unwrap()
                    .epoch,
                receipt.epoch
            );
            assert_eq!(store.sync_mode_snapshot().unwrap().token, local.token);
            assert_eq!(fs::read(path(&store)).unwrap(), original);
            assert!(store.legacy_publisher_lease().is_err());
            assert!(store.legacy_sync_lease().is_ok());
            assert!(store
                .set_sync_mode(&local.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
                .is_err());
            drop(guard);
            set(&store, SyncModeChoice::LocalOnly);
        }
    }

    #[test]
    fn sync_mode_persisted_backup_history_refuses_reuse_and_discontinuity() {
        for case in 0..3 {
            let f = Fixture::new();
            let store = f.store();
            let before = store.sync_mode_snapshot().unwrap();
            let operation = Uuid::new_v4();
            let choice = SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"binding A"),
            };
            let first = store
                .set_sync_mode(&before.token, operation, choice.clone())
                .unwrap();
            match case {
                0 => {
                    set(&store, SyncModeChoice::LocalOnly);
                }
                1 => {
                    set(&store, choice.clone());
                }
                _ => {
                    set(
                        &store,
                        SyncModeChoice::IsolatedBackup {
                            binding_sha256: digest(b"binding B"),
                        },
                    );
                }
            }
            if case == 0 {
                set(&store, choice.clone());
            }
            let valid_token = store.sync_mode_snapshot().unwrap().token;
            let original = fs::read(path(&store)).unwrap();
            let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
            let index = if case == 0 { 2 } else { 1 };
            changed["transitions"][index]["mode"]["backup"] = json!(if case == 1 {
                Uuid::new_v4()
            } else {
                backup(&first)
            });
            let corrupt = serde_json::to_vec(&changed).unwrap();
            fs::write(path(&store), &corrupt).unwrap();
            drop(store);
            let store = f.store();
            assert!(
                store.sync_mode_snapshot().is_err(),
                "corrupt history case {case}"
            );
            assert!(store.sync_mode_lease(&valid_token).is_err());
            assert!(store
                .set_sync_mode(&before.token, operation, choice.clone())
                .is_err());
            assert_eq!(fs::read(path(&store)).unwrap(), corrupt);
            fs::write(path(&store), original).unwrap();
            assert_eq!(store.sync_mode_snapshot().unwrap().token, valid_token);
            assert_eq!(
                backup(
                    &store
                        .set_sync_mode(&before.token, operation, choice)
                        .unwrap()
                ),
                backup(&first)
            );
        }
    }
    #[test]
    fn sync_mode_faults_recover_original_choice_and_refuse_foreign_metadata() {
        for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
            let f = Fixture::new();
            let store = f.store();
            let before = store.sync_mode_snapshot().unwrap();
            let operation = Uuid::new_v4();
            store.set_fault(fault).unwrap();
            assert!(store
                .set_sync_mode(&before.token, operation, SyncModeChoice::LocalOnly)
                .is_err());
            store.set_fault(Fault::None).unwrap();
            drop(store);
            let store = f.store();
            assert_eq!(
                store.sync_mode_snapshot().unwrap().mode.is_some(),
                fault == Fault::AfterCommit
            );
            let receipt = store
                .set_sync_mode(&before.token, operation, SyncModeChoice::LocalOnly)
                .unwrap();
            assert_eq!(receipt.mode, SyncMode::LocalOnly);
            let original = fs::read(path(&store)).unwrap();
            let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
            changed["actor"] = json!(Uuid::new_v4());
            fs::write(path(&store), serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(store.sync_mode_snapshot().is_err());
            fs::write(path(&store), original).unwrap();
            assert!(store.sync_mode_snapshot().is_ok());
        }
    }
}
