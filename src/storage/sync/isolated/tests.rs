use super::*;
use crate::{
    config::Config,
    conversation::Ledger,
    reader_runtime::{self, StartupDecision},
    storage::{
        drive::shared_fixture::Wire,
        ordinary_restore::tests::{record, Fixture},
    },
    workflow::conversation_dispatch::Dispatch,
};

const FIXTURE_ROOT: &str = "original-isolated-root";

struct Local {
    fixture: Fixture,
    store: Arc<Store>,
    wire: Wire,
    group: Uuid,
    binding: Binding,
}

impl Local {
    fn new() -> Self {
        let fixture = Fixture::new();
        let store = fixture.open();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let client = wire.client(store.paths.credentials.join("drive.json"), group);
        let grant = client.binding_digest();
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                mode::SyncModeChoice::IsolatedBackup {
                    binding_sha256: grant.clone(),
                },
            )
            .unwrap();
        let snapshot = store.sync_mode_snapshot().unwrap();
        let Some(mode::SyncMode::IsolatedBackup { backup, .. }) = snapshot.mode else {
            panic!()
        };
        let binding = Binding {
            actor: store.actor_id,
            device: snapshot.device.unwrap(),
            backup,
            epoch: snapshot.epoch.unwrap(),
            grant,
            root: FIXTURE_ROOT.into(),
        };
        wire.immutable(FIXTURE_ROOT, &binding.root_properties(), Vec::new());
        let dir = store.paths.data.join("sync/isolated");
        files::directory(&dir).unwrap();
        let state = Checkpoint {
            format: 1,
            binding: binding.clone(),
            latest: None,
            attempt: None,
            settled_stage: None,
        };
        files::create_identity(&dir.join("publisher.json"), &state).unwrap();
        Self {
            fixture,
            store,
            wire,
            group,
            binding,
        }
    }

    fn engine(&self, group: Uuid) -> Result<Engine> {
        let store = &self.store;
        let binding = self.binding.clone();
        let snapshot = store.sync_mode_snapshot()?;
        ensure!(
            binding.matches(store, &snapshot),
            "fixture binding does not match local mode"
        );
        validate_properties(&binding.root_properties())?;
        let lease = store.sync_mode_lease(&snapshot.token)?;
        let directory = store.paths.data.join("sync/isolated");
        let checkpoint = directory.join("publisher.json");
        let lock_path = directory.join("publisher.lock");
        files::safe_path(&lock_path)?;
        let publisher = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        publisher.try_lock()?;
        let state = Engine::read_checkpoint(&checkpoint, &binding)?;
        let staging = self
            .fixture
            .root
            .join("staging")
            .join(binding.backup.to_string());
        let sessions = store
            .paths
            .credentials
            .join("isolated")
            .join(binding.backup.to_string());
        files::directory(&staging)?;
        files::directory(&sessions)?;
        Ok(Engine {
            store: store.clone(),
            lease,
            binding,
            saved_config: self.fixture.root.join("config.json"),
            checkpoint,
            staging,
            sessions,
            transport: self
                .wire
                .client(store.paths.credentials.join("drive.json"), group),
            policy: SyncPolicy {
                poll_seconds: 5,
                ..SyncPolicy::default()
            },
            cancel: Arc::new(AtomicBool::new(false)),
            state,
            search: Search::default(),
            members: None,
            checked: 0,
            observe_cursor: 0,
            stale: false,
            _publisher: publisher,
        })
    }

    fn commit(&self) {
        self.store
            .commit(vec![record(&self.store)], BTreeMap::new())
            .unwrap();
    }

    fn advance(engine: &mut Engine, until: impl Fn(&Engine, Status) -> bool) {
        for _ in 0..110 {
            let status = engine.step().unwrap();
            if until(engine, status) {
                return;
            }
            assert_ne!(
                status,
                Status::RecoveryRequired,
                "{}",
                engine.health(status)
            );
        }
        panic!("isolated worker did not settle");
    }

    fn finish(engine: &mut Engine) {
        Self::advance(engine, |engine, status| {
            engine.state.attempt.is_none()
                && matches!(status, Status::Current | Status::IncompleteMedia)
        });
    }
}

#[test]
fn scheduler_uploads_marker_last_and_rebuilds_exact_package() {
    let local = Local::new();
    local.commit();
    let engine = local.engine(local.group).unwrap();
    assert!(
        local.engine(local.group).is_err(),
        "second local publisher admitted"
    );
    let checkpoint = engine.checkpoint.clone();
    let observed_store = local.store.clone();
    local.wire.observe_requests(move |_| {
        drop(
            observed_store
                .inner
                .try_lock()
                .expect("provider call held Store mutex"),
        );
    });
    let worker = Worker::launch(
        local.store.clone(),
        engine.policy.clone(),
        engine.cancel.clone(),
        engine,
    )
    .unwrap()
    .unwrap();
    let health_path = local.store.paths.data.join("sync/health.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(18);
    let mut last = None;
    let health = loop {
        let bytes = files::read(&health_path, 8192).ok();
        if let Some(bytes) = bytes.as_ref() {
            let value: Value = serde_json::from_slice(bytes).unwrap();
            if value["status"] == "current" && value["latest_verified"].is_object() {
                break value;
            }
            assert_ne!(value["status"], "recovery-required", "{value}");
        }
        if bytes.is_some() {
            last = bytes;
        }
        let _ = worker.wake.try_send(());
        assert!(
            std::time::Instant::now() < deadline,
            "scheduler did not publish: {:?}",
            last.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        );
        thread::sleep(Duration::from_millis(18));
    };
    drop(worker);
    let state = Engine::read_checkpoint(&checkpoint, &local.binding).unwrap();
    let revision = state.latest.unwrap();
    let uploaded = local.wire.uploads_sent();
    assert!(uploaded.len() > 2);
    assert!(uploaded[..uploaded.len() - 1]
        .iter()
        .all(|(_, value)| value["kind"] == "object"));
    assert_eq!(uploaded.last().unwrap().0, revision.id);
    assert_eq!(uploaded.last().unwrap().1["kind"], "marker");
    assert_eq!(
        health["latest_verified"]["marker_sha256"],
        revision.marker.sha256
    );
    let mut receiver = local.wire.client(
        local.store.paths.credentials.join("drive.json"),
        local.group,
    );
    receiver.authorize_grant(&local.binding.grant).unwrap();
    let rebuilt = local.fixture.root.join("download");
    files::directory(&rebuilt.join("data/objects")).unwrap();
    let marker = receiver
        .download(&revision.id, revision.marker.bytes)
        .unwrap();
    let manifest: Value = serde_json::from_slice(&marker).unwrap();
    let download = |receiver: &mut GoogleDrive, reference: &Value, destination: &Path| {
        let reference: ObjectRef = serde_json::from_value(reference.clone()).unwrap();
        let page = receiver
            .list_properties(&local.binding.object_properties(&reference), None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let id = &page.items[0].id;
        receiver.download_to(id, &reference, destination).unwrap();
    };
    download(
        &mut receiver,
        &manifest["data_backup"],
        &rebuilt.join("data/backup.json"),
    );
    download(
        &mut receiver,
        &manifest["portable_configuration"],
        &rebuilt.join("portable-configuration.json"),
    );
    let archive: Value =
        files::json(&rebuilt.join("data/backup.json"), MAX_METADATA as u64).unwrap();
    for reference in archive["objects"].as_array().unwrap() {
        let name = reference["sha256"].as_str().unwrap();
        download(
            &mut receiver,
            reference,
            &rebuilt.join("data/objects").join(name),
        );
    }
    files::atomic(&rebuilt.join(MARKER), &marker).unwrap();
    Store::inspect_isolated_package(&rebuilt, Some(&revision.marker.sha256)).unwrap();
    assert!(!health.to_string().contains("PRIVATE"));
    assert!(!health.to_string().contains(FIXTURE_ROOT));
    assert!(!health.to_string().contains(&revision.id));
}

#[test]
fn original_marker_is_reconciled_after_lost_response_even_with_stale_config() {
    let local = Local::new();
    local.commit();
    let mut first = local.engine(local.group).unwrap();
    Local::finish(&mut first);
    let previous = first.state.latest.clone().unwrap();
    local.commit();
    Local::advance(&mut first, |e, _| {
        e.state
            .attempt
            .as_ref()
            .is_some_and(|a| a.marker_id.is_some())
    });
    let original = first
        .state
        .attempt
        .as_ref()
        .unwrap()
        .marker_id
        .clone()
        .unwrap();
    local.wire.lose_marker_once();
    let mut lost = false;
    for _ in 0..32 {
        match first.step() {
            Ok(Status::Pending) => continue,
            Err(_) => {
                lost = true;
                break;
            }
            result => panic!("unexpected result before lost acknowledgment: {result:?}"),
        }
    }
    assert!(
        lost,
        "original marker dispatch did not reach the lost acknowledgment"
    );
    assert_eq!(
        first.health(Status::RetryLater)["latest_verified"]["marker_sha256"],
        previous.marker.sha256
    );
    let count = local.wire.uploads_sent().len();
    drop(first);
    let path = local.fixture.root.join("config.json");
    let mut changed = fs::read(&path).unwrap();
    changed.push(b'\n');
    fs::write(&path, changed).unwrap();
    // An alternate app/account grant cannot inspect or resend original IDs.
    let mut foreign = local.engine(Uuid::new_v4()).unwrap();
    let requests = local.wire.requests().len();
    let error = foreign.step().unwrap_err();
    assert!(
        error
            .downcast_ref::<TransportFailure>()
            .unwrap()
            .authorization_required
    );
    assert_eq!(local.wire.requests().len(), requests);
    assert_eq!(
        foreign.health(Status::AuthorizationRequired)["latest_verified"]["marker_sha256"],
        previous.marker.sha256
    );
    drop(foreign);

    let mut resumed = local.engine(local.group).unwrap();
    Local::advance(&mut resumed, |engine, _| engine.state.attempt.is_none());
    assert_eq!(resumed.state.latest.as_ref().unwrap().id, original);
    assert_eq!(local.wire.uploads_sent().len(), count);
    assert_eq!(
        local
            .wire
            .uploads_sent()
            .iter()
            .filter(|(id, _)| id == &original)
            .count(),
        1
    );
}

#[test]
fn empty_store_and_offline_startup_leave_credentials_unopened() {
    let local = Local::new();
    let mut engine = local.engine(local.group).unwrap();
    let config_path = local.fixture.root.join("config.json");
    fs::remove_file(config_path).unwrap();
    assert_eq!(engine.step().unwrap(), Status::Current);
    assert!(local.wire.requests().is_empty());
    drop(engine);
    // Reopen through the ordinary startup admission, with an intentionally
    // absent credential and with local-memory initialization enabled.
    let Local {
        fixture,
        store,
        wire,
        binding,
        ..
    } = local;
    drop(store);
    let paths = fixture.paths();
    fs::remove_file(paths.credentials.join("drive.json")).unwrap();
    let path = fixture.root.join("config.json");
    files::atomic_json(
        &path,
        &json!({"paths":paths,"memory":{"enabled":true,"profile":Uuid::new_v4()}}),
    )
    .unwrap();
    let config = Config::load(Some(path.clone())).unwrap();
    assert_eq!(config.saved_path(), Some(path.as_path()));
    let admitted = match reader_runtime::startup(&config) {
        StartupDecision::Ordinary(admitted) => admitted,
        StartupDecision::Report(report) => panic!("{}", report.into_ordinary_startup_error()),
    };
    let store = reader_runtime::ordinary_store(&config, admitted)
        .unwrap_or_else(|_| panic!("ordinary Store refused"));
    let ledger = Ledger::new(store.clone());
    let dispatch = Dispatch::local(&config, &ledger).unwrap();
    assert!(dispatch.local_memory().is_some());
    assert!(crate::mode_maintenance::admit_ordinary(&store, false).is_err());
    assert!(Worker::isolated(store.clone(), config.sync.clone(), &path)
        .unwrap()
        .is_none());
    let health: Value = files::json(&store.paths.data.join("sync/health.json"), 8192).unwrap();
    assert_eq!(health["reason"], "provider-binding-unqualified");
    assert_eq!(health["backup"], binding.backup.to_string());
    assert!(wire.requests().is_empty());
    let mut legacy = config.clone();
    legacy.sync.enabled = true;
    assert!(Dispatch::local(&legacy, &ledger).is_err());
    assert!(reader_runtime::ordinary_store(&legacy, Some(store.clone())).is_err());
    let mut selected = config.clone();
    selected.reader.selection = Some(selection::SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"selected key"),
        binding_sha256: digest(b"selected binding"),
    });
    assert!(Dispatch::local(&selected, &ledger).is_err());
    assert!(reader_runtime::ordinary_store(&selected, Some(store.clone())).is_err());
    let mut shared_memory = config.clone();
    shared_memory.memory.selection = Some(crate::memory::catalog::Identity {
        group: Uuid::new_v4(),
        profile: shared_memory.memory.profile.unwrap(),
        binding_sha256: digest(b"memory binding"),
    });
    assert!(Dispatch::local(&shared_memory, &ledger).is_err());
    assert!(store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::LocalOnly
        )
        .is_err());
    drop(dispatch);
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    assert!(Dispatch::local(&config, &ledger).is_ok());
}

#[test]
fn competing_head_and_corrupt_checkpoint_require_recovery() {
    let local = Local::new();
    local.commit();
    let mut engine = local.engine(local.group).unwrap();
    Local::advance(&mut engine, |e, _| {
        e.state
            .attempt
            .as_ref()
            .is_some_and(|a| a.marker_id.is_some())
    });
    let attempt = engine.state.attempt.as_ref().unwrap();
    let marker = attempt.marker.as_ref().unwrap();
    let props = local
        .binding
        .marker_properties(&attempt.predecessor, marker);
    local.wire.immutable("competing-marker", &props, vec![]);
    Local::advance(&mut engine, |_, status| status == Status::RecoveryRequired);
    assert!(local
        .wire
        .uploads_sent()
        .iter()
        .all(|(_, value)| value["kind"] == "object"));
    files::atomic(&engine.checkpoint, b"{\"format\":1,\"format\":1}").unwrap();
    assert_eq!(engine.step().unwrap(), Status::RecoveryRequired);
}

fn with_media(local: &Local, size: usize, omitted: bool) -> ObjectRef {
    let bytes = vec![42_u8; size];
    let reference = ObjectRef {
        sha256: digest(&bytes),
        bytes: size as u64,
    };
    let mut entry = record(&local.store);
    entry.media_descriptors.push(Media {
        sha256: reference.sha256.clone(),
        bytes: reference.bytes,
        media_type: "image/png".into(),
    });
    if omitted {
        let encoded = serde_json::to_vec(&entry).unwrap();
        let object = ObjectRef {
            sha256: digest(&encoded),
            bytes: encoded.len() as u64,
        };
        local
            .store
            .import(
                Manifest {
                    format: FORMAT,
                    transaction_id: Uuid::new_v4(),
                    scope: Scope::LocalTransaction,
                    records: vec![object.clone()],
                    record_namespaces: BTreeMap::from([(object.sha256.clone(), entry.namespace)]),
                    media: vec![],
                    media_coverage: vec![Coverage::Omitted {
                        sha256: reference.sha256.clone(),
                        reason: Omission::SizeDeferred,
                    }],
                },
                BTreeMap::from([(object.sha256, encoded)]),
            )
            .unwrap();
    } else {
        local
            .store
            .commit(
                vec![entry],
                BTreeMap::from([(reference.sha256.clone(), bytes)]),
            )
            .unwrap();
    }
    reference
}

#[test]
fn resumable_expiration_restarts_the_same_id_and_settles_with_verified_media() {
    let local = Local::new();
    let media = with_media(&local, 600 * 1024, false);
    let mut engine = local.engine(local.group).unwrap();
    Local::advance(&mut engine, |e, _| {
        e.state
            .attempt
            .as_ref()
            .and_then(|a| a.objects.get(&media.sha256).map(|o| (a.id, &o.id)))
            .is_some_and(|(attempt, id)| e.session_path(attempt, id).is_file())
    });
    let attempt = engine.state.attempt.as_ref().unwrap();
    let id = attempt.objects[&media.sha256].id.clone();
    assert!(
        files::json::<UploadSession>(&engine.session_path(attempt.id, &id), 8192)
            .unwrap()
            .uri
            .is_some()
    );
    drop(engine);
    local.wire.expire_once();
    let mut engine = local.engine(local.group).unwrap();
    Local::finish(&mut engine);
    let ids: Vec<_> = local
        .wire
        .uploads_sent()
        .into_iter()
        .filter(|(_, props)| props["sha256"] == media.sha256)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, [id.clone(), id]);
    assert!(engine.state.latest.as_ref().unwrap().complete_media);
}

#[test]
fn partial_media_is_verified_but_missing_promised_bytes_and_capacity_refuse() {
    let local = Local::new();
    with_media(&local, 300 * 1024, true);
    let mut engine = local.engine(local.group).unwrap();
    Local::finish(&mut engine);
    let previous = engine.state.latest.as_ref().unwrap().marker.sha256.clone();
    assert_eq!(
        engine.health(Status::IncompleteMedia)["latest_verified"]["complete_media"],
        false
    );
    let promised = with_media(&local, 320 * 1024, false);
    engine.policy.max_media_bytes = 300 * 1024;
    Local::advance(&mut engine, |_, status| status == Status::RecoveryRequired);
    assert_eq!(
        engine.health(Status::RecoveryRequired)["latest_verified"]["marker_sha256"],
        previous
    );
    assert_eq!(
        local
            .wire
            .uploads_sent()
            .iter()
            .filter(|(_, props)| props["kind"] == "marker")
            .count(),
        1
    );
    engine.policy.max_media_bytes = 8 * 1024 * 1024;
    Local::advance(&mut engine, |e, _| {
        e.state
            .attempt
            .as_ref()
            .is_some_and(|a| a.marker_id.is_some())
    });
    let upload = &engine.state.attempt.as_ref().unwrap().objects[&promised.sha256];
    local.wire.remove_fixture_object(&upload.id);
    Local::advance(&mut engine, |_, status| status == Status::RecoveryRequired);
    assert!(engine.state.attempt.is_some());
    assert_eq!(
        engine.health(Status::RecoveryRequired)["latest_verified"]["marker_sha256"],
        previous
    );
}

#[test]
fn stale_config_cannot_create_absent_marker_and_original_binding_must_match() {
    let local = Local::new();
    local.commit();
    let mut engine = local.engine(local.group).unwrap();
    Local::advance(&mut engine, |e, _| {
        e.state
            .attempt
            .as_ref()
            .is_some_and(|a| a.marker_id.is_some())
    });
    let before = local.wire.uploads_sent().len();
    let original = engine
        .state
        .attempt
        .as_ref()
        .unwrap()
        .marker_id
        .clone()
        .unwrap();
    let config = local.fixture.root.join("config.json");
    let mut bytes = fs::read(&config).unwrap();
    bytes.push(b'\n');
    fs::write(&config, bytes).unwrap();
    let requests = local.wire.requests().len();
    assert_eq!(engine.step().unwrap(), Status::Pending);
    assert!(local.wire.requests()[requests..]
        .iter()
        .any(|line| line.contains(&format!("/files/{original}?"))));
    assert_eq!(local.wire.uploads_sent().len(), before);
    let mut other_root = local.binding.clone();
    other_root.root = "second-root".into();
    assert!(Engine::read_checkpoint(&engine.checkpoint, &other_root).is_err());
    local.wire.about_status(401);
    let problem = engine.step().unwrap_err();
    assert!(
        problem
            .downcast_ref::<TransportFailure>()
            .unwrap()
            .authorization_required
    );
    drop(engine);
    local
        .store
        .set_sync_mode(
            &local.store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    assert!(local.engine(local.group).is_err());
}
