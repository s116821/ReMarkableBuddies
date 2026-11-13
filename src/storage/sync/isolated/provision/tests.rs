use super::*;
use crate::storage::{
    drive::shared_fixture::Wire,
    ordinary_restore::tests::{record, Fixture},
};

struct Setup {
    root: Fixture,
    wire: Wire,
    group: Uuid,
    grant: String,
    store: Arc<Store>,
}

impl Setup {
    fn new() -> Self {
        let root = Fixture::new();
        let store = root.open();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let grant = wire
            .client(store.paths.credentials.join("drive.json"), group)
            .binding_digest();
        Self {
            root,
            wire,
            group,
            grant,
            store,
        }
    }

    fn choose(&self, choice: mode::SyncModeChoice) {
        let before = self.store.sync_mode_snapshot().unwrap();
        self.store
            .set_sync_mode(&before.token, Uuid::new_v4(), choice)
            .unwrap();
    }

    fn isolated(&self) -> Request {
        self.choose(mode::SyncModeChoice::IsolatedBackup {
            binding_sha256: self.grant.clone(),
        });
        let snapshot = self.store.sync_mode_snapshot().unwrap();
        Request::parse(
            &serde_json::to_vec(&json!({
                "control_version":1, "operation":Uuid::new_v4(), "expected":snapshot.token,
            }))
            .unwrap(),
        )
        .unwrap()
    }

    fn client(&self) -> GoogleDrive {
        self.wire
            .client(self.store.paths.credentials.join("drive.json"), self.group)
    }

    fn execute(&self, request: &Request) -> Outcome {
        run_with(self.store.clone(), request.clone(), Ok(self.client())).unwrap()
    }

    fn checkpoint(&self, outcome: &Outcome) -> PathBuf {
        checkpoint_dir(&self.store, outcome.backup).join("publisher.json")
    }

    fn allocations(&self) -> usize {
        self.wire
            .requests()
            .iter()
            .filter(|line| line.contains("/generateIds?"))
            .count()
    }
}

#[test]
fn root_lost_ack_reopens_original_and_worker_publishes_existing_v1_package() {
    let test = Setup::new();
    let original = test.isolated();
    let token = test.store.sync_mode_snapshot().unwrap().token;
    let observed_store = Arc::downgrade(&test.store);
    test.wire.observe_requests(move |_| {
        if let Some(store) = observed_store.upgrade() {
            drop(store.inner.try_lock().expect("provider holds Store mutex"));
        }
    });
    test.wire.lose_root_once();
    let unknown = test.execute(&original);
    assert_eq!(unknown.outcome, State::OutcomeUnknown);
    let path = test.checkpoint(&unknown);
    let candidate: Candidate = read_private(&path).unwrap();
    let root_id = candidate.root.unwrap();
    assert_eq!(
        test.wire.slot_posts().as_slice(),
        std::slice::from_ref(&root_id)
    );
    assert!(test.store.sync_mode_snapshot().unwrap().token == token);
    let Setup {
        root,
        wire,
        group,
        grant,
        store,
    } = test;
    drop(store);
    let store = root.open();
    let test = Setup {
        root,
        wire,
        group,
        grant,
        store,
    };
    assert!(test.store.sync_mode_snapshot().unwrap().token == token);
    let confirmed = test.execute(&original);
    assert_eq!(confirmed.outcome, State::RootVerified);
    assert_eq!(
        test.wire.slot_posts().as_slice(),
        std::slice::from_ref(&root_id)
    );
    assert_eq!(test.allocations(), 1);
    let published: Checkpoint = read_private(&path).unwrap();
    assert_eq!(published.binding.root, root_id);
    assert!(published.latest.is_none());
    test.store
        .commit(vec![record(&test.store)], BTreeMap::new())
        .unwrap();
    let worker = launch_rooted(
        test.store.clone(),
        SyncPolicy::default(),
        &test.root.root.join("config.json"),
        Ok(test.client()),
    )
    .unwrap()
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        if let Ok(value) =
            files::json::<Value>(&test.store.paths.data.join("sync/health.json"), 8 * 1024)
        {
            assert_ne!(value["status"], "recovery-required", "{value}");
            if value["latest_verified"].is_object() {
                assert!(!value.to_string().contains(&root_id));
                break;
            }
        }
        assert!(std::time::Instant::now() < deadline);
        let _ = worker.wake.try_send(());
        thread::sleep(Duration::from_millis(20));
    }
    drop(worker);
}

#[test]
fn stale_original_can_observe_and_reentry_has_new_root_and_custody() {
    let test = Setup::new();
    let original = test.isolated();
    test.wire.lose_root_once();
    let unknown = test.execute(&original);
    let candidate: Candidate = read_private(&test.checkpoint(&unknown)).unwrap();
    test.choose(mode::SyncModeChoice::LocalOnly);
    let next = test.isolated();
    let verified = test.execute(&original);
    assert_eq!(verified.outcome, State::RootVerified);
    let report = serde_json::to_value(verified).unwrap();
    assert_eq!(report["currency"], "historical");
    assert_eq!(test.allocations(), 1);
    let new = test.execute(&next);
    assert_eq!(new.outcome, State::RootVerified);
    assert_ne!(new.backup, unknown.backup);
    let checkpoint: Checkpoint = read_private(&test.checkpoint(&new)).unwrap();
    assert_ne!(Some(checkpoint.binding.root), candidate.root);
    assert_eq!(test.allocations(), 2);
    fs::remove_file(test.checkpoint(&new)).unwrap();
    let missing = test.execute(&next);
    assert_eq!(missing.outcome, State::RecoveryRequired);
    assert_eq!(test.allocations(), 2);
}

#[test]
fn same_backup_new_epoch_readmits_verified_root_without_allocation() {
    let test = Setup::new();
    let original = test.isolated();
    let first = test.execute(&original);
    assert_eq!(first.outcome, State::RootVerified);
    let next = test.isolated();
    let readmitted = test.execute(&next);
    assert_eq!(readmitted.outcome, State::RootVerified);
    assert_eq!(readmitted.backup, first.backup);
    assert_ne!(readmitted.epoch, first.epoch);
    let checkpoint: Checkpoint = read_private(&test.checkpoint(&readmitted)).unwrap();
    assert_eq!(checkpoint.binding.epoch, readmitted.epoch);
    assert_eq!(checkpoint.provisioning.unwrap().binding.epoch, first.epoch);
    assert_eq!(test.allocations(), 1);
    assert_eq!(test.wire.slot_posts().len(), 1);
    assert_eq!(
        serde_json::to_value(test.execute(&original)).unwrap()["currency"],
        "historical"
    );
}

#[test]
fn changed_protected_grant_is_checked_at_dispatch() {
    let test = Setup::new();
    let original = test.isolated();
    let credential = test.store.paths.credentials.join("drive.json");
    test.wire.observe_requests(move |line| {
        if line.contains("/files/generated-1?") {
            let mut raw = fs::read(&credential).unwrap();
            raw.push(b'\n');
            files::atomic(&credential, &raw).unwrap();
        }
    });
    let refused = test.execute(&original);
    assert!(matches!(
        refused.outcome,
        State::OutcomeUnknown | State::AuthorizationRequired
    ));
    assert!(test.wire.slot_posts().is_empty());
    assert_eq!(test.allocations(), 1);
    let confirmed = test.execute(&original);
    // The hook changes the replacement again before the create.
    assert!(matches!(
        confirmed.outcome,
        State::OutcomeUnknown | State::AuthorizationRequired
    ));
    assert_eq!(test.allocations(), 1);
}

#[test]
fn current_epoch_cannot_rebind_unsettled_package_or_adopt_foreign_root() {
    let test = Setup::new();
    let request = test.isolated();
    let first = test.execute(&request);
    let path = test.checkpoint(&first);
    let mut state: Checkpoint = read_private(&path).unwrap();
    state.attempt = Some(Attempt {
        id: Uuid::new_v4(),
        predecessor: state.binding.root.clone(),
        source: PublicationSource {
            generation: Uuid::new_v4(),
            history: digest(b"history"),
            config_path: digest(b"config path"),
            config_sha256: digest(b"config bytes"),
        },
        marker: None,
        objects: BTreeMap::new(),
        marker_id: None,
        marker_observed: false,
    });
    write_private(&path, &state).unwrap();
    let next = test.isolated();
    assert_eq!(test.execute(&next).outcome, State::RecoveryRequired);
    let retained: Checkpoint = read_private(&path).unwrap();
    assert_eq!(
        retained.attempt.unwrap().id,
        state.attempt.take().unwrap().id
    );
    assert_eq!(retained.binding.epoch, first.epoch);
    write_private(&path, &state).unwrap();
    let foreign = BTreeMap::from([("kind".into(), "foreign".into())]);
    test.wire
        .immutable(&state.binding.root, &foreign, Vec::new());
    assert_eq!(test.execute(&next).outcome, State::RecoveryRequired);
    assert_eq!(test.allocations(), 1);
    assert_eq!(test.wire.slot_posts().len(), 1);
    let missing: Result<GoogleDrive> = Err(anyhow::anyhow!("fixture missing protected grant"));
    assert!(launch_rooted(
        test.store.clone(),
        SyncPolicy::default(),
        &test.root.root.join("config.json"),
        missing
    )
    .unwrap()
    .is_none());
    let health: Value = files::json(&test.store.paths.data.join("sync/health.json"), 8192).unwrap();
    assert_eq!(health["reason"], "root-admission-stale");
}

#[test]
fn rooted_worker_keeps_local_store_usable_when_private_paths_are_blocked() {
    for blocked_path in [
        "checkpoint-parent",
        "checkpoint",
        "lock",
        "staging",
        "sessions",
    ] {
        let test = Setup::new();
        let request = test.isolated();
        let snapshot = test.store.sync_mode_snapshot().unwrap();
        let Some(mode::SyncMode::IsolatedBackup { backup, .. }) = snapshot.mode else {
            panic!("isolated fixture mode unavailable");
        };
        let directory = checkpoint_dir(&test.store, backup);
        if !matches!(blocked_path, "checkpoint-parent" | "checkpoint") {
            let verified = test.execute(&request);
            assert_eq!(verified.outcome, State::RootVerified);
            let path = test.checkpoint(&verified);
            let mut state: Checkpoint = read_private(&path).unwrap();
            state.latest = Some(Revision {
                id: "marker-fixture".into(),
                predecessor: state.binding.root.clone(),
                marker: ObjectRef {
                    sha256: digest(b"last marker"),
                    bytes: 11,
                },
                complete_media: true,
                source: PublicationSource {
                    generation: Uuid::new_v4(),
                    history: digest(b"history"),
                    config_path: digest(b"path"),
                    config_sha256: digest(b"config"),
                },
                verified_at_unix_seconds: 17,
            });
            write_private(&path, &state).unwrap();
        }
        let obstruction = match blocked_path {
            "checkpoint-parent" => test.store.paths.data.join("sync/isolated"),
            "checkpoint" => directory.join("publisher.json"),
            "lock" => directory.join("publisher.lock"),
            "staging" => test
                .root
                .root
                .join(format!(".buddy-isolated-{}", test.store.actor_id)),
            "sessions" => test.store.paths.credentials.join("isolated"),
            _ => unreachable!(),
        };
        if blocked_path == "checkpoint" {
            files::directory(&obstruction).unwrap();
            let sentinel = obstruction.join("keep");
            fs::write(&sentinel, b"PRIVATE_BLOCKED_PATH").unwrap();
            assert!(test.launch_without_provider().is_none());
            assert_eq!(test.health()["reason"], "root-custody-invalid");
            assert_eq!(fs::read(sentinel).unwrap(), b"PRIVATE_BLOCKED_PATH");
        } else if blocked_path == "lock" {
            // Use the real lock owner to refuse a competing publisher without
            // altering the legitimate file.
            let lock = publisher_lock(&directory).unwrap();
            assert!(test.launch_without_provider().is_none());
            let health = test.health();
            assert_eq!(health["status"], "recovery-required");
            assert_eq!(health["reason"], "publisher-storage-unavailable");
            assert!(obstruction.is_file());
            drop(lock);
        } else {
            fs::write(&obstruction, b"PRIVATE_BLOCKED_PATH").unwrap();
            assert!(test.launch_without_provider().is_none());
            let health = test.health();
            assert_eq!(health["status"], "recovery-required");
            assert_eq!(
                health["reason"],
                if blocked_path == "checkpoint-parent" {
                    "root-custody-invalid"
                } else {
                    "publisher-storage-unavailable"
                }
            );
            assert!(!health.to_string().contains("PRIVATE"));
            assert_eq!(fs::read(&obstruction).unwrap(), b"PRIVATE_BLOCKED_PATH");
        }
        if !matches!(blocked_path, "checkpoint-parent" | "checkpoint") {
            assert_eq!(
                test.health()["latest_verified"]["marker_sha256"],
                digest(b"last marker")
            );
        }
        let entry = record(&test.store);
        test.store
            .commit(vec![entry.clone()], BTreeMap::new())
            .unwrap();
        assert_eq!(
            test.store
                .heads(entry.namespace, entry.record_id)
                .unwrap()
                .len(),
            1
        );
    }
}

impl Setup {
    fn launch_without_provider(&self) -> Option<Worker> {
        let before = self.wire.requests();
        let worker = launch_rooted(
            self.store.clone(),
            SyncPolicy::default(),
            &self.root.root.join("config.json"),
            Ok(self.client()),
        )
        .unwrap();
        assert_eq!(self.wire.requests(), before);
        worker
    }

    fn health(&self) -> Value {
        files::json(&self.store.paths.data.join("sync/health.json"), 8192).unwrap()
    }
}
