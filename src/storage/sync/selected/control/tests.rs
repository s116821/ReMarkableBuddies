use super::*;
use crate::storage::{compound_generation, drive::shared_fixture::Wire};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-group-control-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn prepared(&self, group: Uuid, binding: &str) -> Arc<Store> {
        let store = Store::open(self.paths()).unwrap();
        let token = serde_json::to_value(&store.sync_mode_snapshot().unwrap().token).unwrap();
        let request = compound_generation::Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: files::json(&self.paths().data.join("CURRENT"), 128).unwrap(),
            group,
            binding_sha256: binding.into(),
            expected_mode: serde_json::from_value(token).unwrap(),
        };
        drop(store);
        Arc::new(Store::prepare_shared_notebook(self.paths(), request).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(store: &Store, group: Uuid, binding: &str) -> CreateGroupRequest {
    CreateGroupRequest {
        control_version: 1,
        operation: Uuid::new_v4(),
        expected: serde_json::from_value(
            serde_json::to_value(&store.sync_mode_snapshot().unwrap().token).unwrap(),
        )
        .unwrap(),
        group,
        binding_sha256: binding.into(),
        profile: Uuid::new_v4(),
    }
}
fn launch(wire: &Wire, store: Arc<Store>, group: Uuid) -> Controller {
    let transport = wire.client(store.paths.credentials.join("drive.json"), group);
    let application = Application {
        group,
        digest: transport.binding_digest(),
    };
    Controller::new(store, transport, application).unwrap()
}
fn counts(wire: &Wire) -> (usize, usize) {
    let requests = wire.requests();
    (
        requests
            .iter()
            .filter(|r| r.starts_with("GET /drive/v3/files/generateIds?"))
            .count(),
        requests.iter().filter(|r| r.starts_with("POST ")).count(),
    )
}

#[test]
fn original_control_resumes_each_stage_with_same_ids_and_complete_bootstrap() {
    for stop in [
        Stage::Original,
        Stage::Allocated,
        Stage::Bodies,
        Stage::Mode,
        Stage::Root,
        Stage::Catalog,
    ] {
        let fixture = Fixture::new();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let binding = wire
            .client(fixture.0.join("provider/drive.json"), group)
            .binding_digest();
        let store = fixture.prepared(group, &binding);
        let req = request(&store, group, &binding);
        let mut control = launch(&wire, store.clone(), group);
        assert!(control
            .run(req.clone(), |stage| {
                ensure!(stage != stop, "synthetic control interruption");
                Ok(())
            })
            .is_err());
        let before = control.state.selected.as_ref().unwrap().controls[&req.operation].clone();
        let generation = control.generation().unwrap();
        drop(control);
        drop(store);
        let store = Arc::new(Store::open_existing_for_mode(fixture.paths()).unwrap());
        let mut control = launch(&wire, store.clone(), group);
        let outcome = control.run(req.clone(), |_| Ok(())).unwrap();
        assert!(outcome.bootstrap_complete);
        let after = &control.state.selected.as_ref().unwrap().controls[&req.operation];
        assert!(after.ids.starts_with(&before.ids));
        assert_eq!(after.ids.len(), 5);
        assert_eq!(counts(&wire).0, 5);
        assert_eq!(after.source_generation, generation);
        assert!(after.root_verified && after.accepted_catalog.is_some());
        let requests = wire.requests().len();
        let posts = counts(&wire).1;
        control.run(req.clone(), |_| Ok(())).unwrap();
        assert!(wire.requests().len() > requests); // Read verified slots again.
        assert_eq!(counts(&wire).1, posts); // No duplicate object or pointer POST.
        let mut changed = req;
        changed.profile = Uuid::new_v4();
        let before = wire.requests().len();
        assert!(control.run(changed, |_| Ok(())).is_err());
        assert_eq!(wire.requests().len(), before);
    }
}

#[test]
fn changed_mode_before_selection_or_after_receipt_stops_all_original_publication() {
    for stage in [Stage::BeforeMode, Stage::Mode, Stage::Root] {
        let fixture = Fixture::new();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let binding = wire
            .client(fixture.0.join("provider/drive.json"), group)
            .binding_digest();
        let store = fixture.prepared(group, &binding);
        let req = request(&store, group, &binding);
        let mut control = launch(&wire, store.clone(), group);
        let mut changed = false;
        let result = control.run(req.clone(), |now| {
            if now == stage {
                if now == Stage::Root {
                    // Root publication holds the shared lease through catalog.
                    assert!(store
                        .set_sync_mode(
                            &store.sync_mode_snapshot()?.token,
                            Uuid::new_v4(),
                            mode::SyncModeChoice::LocalOnly
                        )
                        .is_err());
                    anyhow::bail!("synthetic root interruption");
                }
                store.set_sync_mode(
                    &store.sync_mode_snapshot()?.token,
                    Uuid::new_v4(),
                    mode::SyncModeChoice::LocalOnly,
                )?;
                changed = true;
            }
            Ok(())
        });
        assert!(result.is_err());
        if !changed {
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    mode::SyncModeChoice::LocalOnly,
                )
                .unwrap();
        }
        let before = wire.requests().len();
        assert!(control.run(req, |_| Ok(())).is_err());
        assert_eq!(wire.requests().len(), before);
        if stage != Stage::Root {
            assert_eq!(counts(&wire).1, 0);
        }
    }
}

#[test]
fn lost_root_ack_observes_original_pointer_without_reupload() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group)
        .binding_digest();
    let store = fixture.prepared(group, &binding);
    let req = request(&store, group, &binding);
    let mut control = launch(&wire, store.clone(), group);
    wire.lose_create("generated-1");
    // The maintained fixed-slot transport resolves this lost response with
    // its exact-ID observation in the same call; no artificial retry layer.
    control.run(req.clone(), |_| Ok(())).unwrap();
    let staged = control.state.selected.as_ref().unwrap().controls[&req.operation].clone();
    assert!(staged.root_verified);
    assert!(staged.mode.is_some());
    drop(control);
    let mut control = launch(&wire, store, group);
    control.run(req, |_| Ok(())).unwrap();
    assert_eq!(counts(&wire), (5, 4)); // root+catalog descriptor and pointer, each once.
}

#[test]
fn existing_valid_nonempty_catalog_is_adopted_and_never_replaced_with_empty() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group)
        .binding_digest();
    let store = fixture.prepared(group, &binding);
    let req = request(&store, group, &binding);
    let mut control = launch(&wire, store, group);
    assert!(control
        .run(req.clone(), |stage| {
            ensure!(stage != Stage::Root, "synthetic root interruption");
            Ok(())
        })
        .is_err());
    let original = &control.state.selected.as_ref().unwrap().controls[&req.operation];
    let root = original.root.as_ref().unwrap();
    let mut catalog: Catalog =
        serde_json::from_slice(&original.catalog.as_ref().unwrap().bytes).unwrap();
    let owner: Owner = crate::memory::catalog::Owner::Document {
        document: Uuid::new_v4(),
    }
    .into();
    catalog.entries.insert(
        owner.key_sha256().unwrap(),
        Entry {
            owner,
            genesis_slot: "existing-document".into(),
        },
    );
    catalog.operation = Uuid::new_v4();
    let bytes = serde_json::to_vec(&catalog).unwrap();
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: catalog_key(group),
            slot_id: catalog.slot.clone(),
        },
        "existing-catalog-body",
        bytes.clone(),
    );
    let root_digest = root.sha256.clone();
    let posts = counts(&wire).1;
    let result = control.run(req.clone(), |_| Ok(())).unwrap();
    assert_eq!(result.root_sha256, root_digest);
    assert_eq!(counts(&wire).1, posts);
    assert_eq!(
        control.state.selected.as_ref().unwrap().controls[&req.operation]
            .accepted_catalog
            .as_ref()
            .unwrap()
            .bytes,
        bytes
    );
}

#[test]
fn pull_only_joiner_keeps_missing_catalog_joining_without_any_upload() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group)
        .binding_digest();
    let store = fixture.prepared(group, &binding);
    let req = request(&store, group, &binding);
    let mut control = launch(&wire, store.clone(), group);
    assert!(control
        .run(req.clone(), |stage| {
            ensure!(stage != Stage::Root, "synthetic root interruption");
            Ok(())
        })
        .is_err());
    let root_id = control.state.selected.as_ref().unwrap().controls[&req.operation].ids[0].clone();
    drop(control);
    let transport = wire.client(fixture.paths().credentials.join("drive.json"), group);
    let mut engine = Engine::new(
        store,
        SyncPolicy::default(),
        transport,
        Binding {
            group,
            root_id,
            digest: binding,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let posts = counts(&wire).1;
    assert_eq!(engine.step().unwrap(), Status::Joining);
    assert_eq!(counts(&wire).1, posts);
    assert_eq!(engine.step().unwrap(), Status::Joining);
    assert_eq!(counts(&wire).1, posts);
}

#[test]
fn actual_http_allocation_and_publication_hold_the_right_lease_without_store_mutex() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group)
        .binding_digest();
    let store = fixture.prepared(group, &binding);
    let req = request(&store, group, &binding);
    let observed = store.clone();
    let counts = Arc::new(std::sync::Mutex::new((0usize, 0usize)));
    let captured = counts.clone();
    wire.observe_requests(move |request| {
        // This callback executes inside the real HTTP server before replying.
        // try_lock makes an accidental mutex-held HTTP request deterministic.
        drop(observed.inner.try_lock().expect("HTTP must not hold Store mutex"));
        let snapshot = observed.sync_mode_snapshot().unwrap();
        if request.starts_with("GET /drive/v3/files/generateIds?") {
            assert!(snapshot.mode.is_none());
            captured.lock().unwrap().0 += 1;
        } else if request.starts_with("POST ") {
            assert!(matches!(snapshot.mode, Some(mode::SyncMode::SharedGroup {group: actual, ..}) if actual == group));
            captured.lock().unwrap().1 += 1;
        }
        assert!(observed.set_sync_mode(&snapshot.token, Uuid::new_v4(), mode::SyncModeChoice::LocalOnly).is_err());
    });
    launch(&wire, store, group).run(req, |_| Ok(())).unwrap();
    assert_eq!(*counts.lock().unwrap(), (5, 4));
}

#[test]
fn production_refuses_before_credentials_checkpoint_or_mode_mutation() {
    let fixture = Fixture::new();
    let group = Uuid::new_v4();
    let binding = digest(b"unqualified");
    let store = fixture.prepared(group, &binding);
    let req = request(&store, group, &binding);
    let before = serde_json::to_vec(&store.sync_mode_snapshot().unwrap().token).unwrap();
    assert!(!fixture.paths().credentials.join("drive.json").exists());
    assert!(create_group(store.clone(), req).is_err());
    assert!(!fixture.paths().credentials.join("drive.json").exists());
    assert!(!fixture.paths().data.join("sync/state.json").exists());
    assert_eq!(
        serde_json::to_vec(&store.sync_mode_snapshot().unwrap().token).unwrap(),
        before
    );
}
