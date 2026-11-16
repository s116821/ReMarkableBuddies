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
fn resumed_group_bootstrap_publishes_first_fact_and_passive_store_adopts_it() {
    use crate::memory::{self, catalog::Identity, selected::SelectedMemory, Provider};

    let publisher_fixture = Fixture::new();
    let receiver_fixture = Fixture::new();
    let wire = Wire::new();
    let group = Uuid::new_v4();
    let binding = wire
        .client(publisher_fixture.0.join("provider/drive.json"), group)
        .binding_digest();
    let publisher = publisher_fixture.prepared(group, &binding);
    let req = request(&publisher, group, &binding);
    let mut control = launch(&wire, publisher.clone(), group);
    assert!(control
        .run(req.clone(), |stage| {
            ensure!(stage != Stage::Root, "synthetic root interruption");
            Ok(())
        })
        .is_err());
    let interrupted = control.state.selected.as_ref().unwrap().controls[&req.operation].clone();
    assert!(interrupted.root_verified && interrupted.accepted_catalog.is_none());
    drop(control);
    drop(publisher);

    let publisher = Arc::new(Store::open_existing_for_mode(publisher_fixture.paths()).unwrap());
    let mut control = launch(&wire, publisher.clone(), group);
    let outcome = control.run(req.clone(), |_| Ok(())).unwrap();
    assert!(outcome.bootstrap_complete);
    assert_eq!(outcome.operation, req.operation);
    let original = control.state.selected.as_ref().unwrap().controls[&req.operation].clone();
    assert_eq!(original.ids, interrupted.ids);
    assert_eq!(original.ids.len(), 5);
    assert_eq!(counts(&wire).0, 5);
    let root = original.root.as_ref().unwrap();
    let catalog = original.accepted_catalog.as_ref().unwrap();
    assert_eq!(outcome.root_id, original.ids[0]);
    assert_eq!(outcome.root_sha256, root.sha256);
    assert_eq!(root.bytes, interrupted.root.as_ref().unwrap().bytes);
    assert_eq!(catalog.bytes, interrupted.catalog.as_ref().unwrap().bytes);
    let root_body: Root = serde_json::from_slice(&root.bytes).unwrap();
    let catalog_body: Catalog = serde_json::from_slice(&catalog.bytes).unwrap();
    assert_eq!(root_body.group, req.group);
    assert_eq!(root_body.profile, req.profile);
    assert_eq!(root_body.binding_sha256, req.binding_sha256);
    assert_eq!(catalog.slot, root_body.catalog_slot);
    assert_eq!(catalog_body.root_sha256, outcome.root_sha256);
    assert!(catalog_body.entries.is_empty());
    let original_control = serde_json::to_value(&original).unwrap();
    drop(control);

    // Use the actual control output; no fixture root or catalog is installed.
    let identity = Identity {
        group: req.group,
        profile: req.profile,
        binding_sha256: req.binding_sha256.clone(),
    };
    let initial = memory::Fact {
        key: memory::Key {
            profile: req.profile,
            scope: memory::Scope::Global,
            fact: Uuid::new_v4(),
        },
        claim: "bootstrap transfer convention".into(),
        value: "initial local value".into(),
        subject: "units".into(),
        tags: vec![],
        provenance: memory::Provenance {
            basis: memory::Basis::UserConfirmed,
            description: "synthetic CreateGroup to Worker composition".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 1,
        expires_ms: None,
    };
    let scope = identity
        .aggregate_scope(&memory::catalog::Owner::GlobalFact {
            key: initial.key.clone(),
        })
        .unwrap();
    let producer = SelectedMemory::new(publisher.clone(), identity.clone()).unwrap();
    let initialized = producer
        .initialize_aggregate(memory::selected::InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: genesis_base(&outcome.root_sha256, &scope.key_sha256),
            fact: initial.clone(),
        })
        .unwrap();
    producer
        .initialize_selection(memory::selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: outcome.root_sha256.clone(),
            registrations: vec![initialized.registration.unwrap()],
        })
        .unwrap();
    let mut edited = initial.clone();
    edited.value = "first real edit after resumed CreateGroup".into();
    let edit = Uuid::new_v4();
    let committed = producer
        .put(memory::Mutation {
            operation: edit,
            expected: producer
                .inspect(&initial.key)
                .unwrap()
                .iter()
                .map(|head| head.revision_id)
                .collect(),
            fact: edited.clone(),
        })
        .unwrap();
    assert_eq!(committed, edit);
    let heads = producer.inspect(&initial.key).unwrap();
    assert_eq!(heads.len(), 1);
    let revision = heads[0].revision_id;
    let sent = publisher
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();

    struct TestWorker(Worker);
    impl TestWorker {
        fn request_stop(&self) {
            self.0.cancel.store(true, Ordering::Relaxed);
            let _ = self.0.wake.try_send(());
        }
        fn stop_and_join(&mut self) {
            self.request_stop();
            self.0
                .done
                .recv_timeout(Duration::from_secs(15))
                .expect("original Worker did not stop before phase boundary");
            self.0
                .handle
                .take()
                .expect("original Worker handle absent")
                .join()
                .expect("original Worker panicked");
        }
    }
    impl Drop for TestWorker {
        fn drop(&mut self) {
            // Also request cancellation if a postcondition/assertion unwinds.
            self.request_stop();
        }
    }
    let start = |store: Arc<Store>| {
        let transport = wire.client(store.paths.credentials.join("drive.json"), req.group);
        let policy = SyncPolicy {
            batch_items: 1,
            poll_seconds: 5,
            ..Default::default()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = Engine::new(
            store.clone(),
            policy.clone(),
            transport,
            Binding {
                group: req.group,
                root_id: outcome.root_id.clone(),
                digest: req.binding_sha256.clone(),
            },
            cancel.clone(),
        )
        .unwrap();
        TestWorker(
            Worker::launch(store, policy, cancel, engine)
                .unwrap()
                .unwrap(),
        )
    };
    // Same bounded, postcondition-driven wake/wait as the existing Worker tests.
    let wait = |worker: &Worker, condition: &mut dyn FnMut() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "Worker postcondition missing; health: {:?}",
                fs::read_to_string(worker.store.paths.data.join("sync/health.json"))
            );
            let _ = worker.wake.try_send(());
            thread::sleep(Duration::from_millis(20));
        }
    };
    let before_publish = wire.requests().len();
    let mut publishing = start(publisher.clone());
    wait(&publishing.0, &mut || {
        publisher
            .selected_attempt_inventory()
            .unwrap()
            .iter()
            .any(|attempt| {
                attempt.selection.scope == scope
                    && attempt.selection.operation == sent.transaction.operation
                    && attempt.state == SelectedAttemptState::Withdrawn
            })
    });
    publishing.stop_and_join();
    let state: State = files::json(
        &publisher.paths.data.join("sync/state.json"),
        MAX_METADATA as u64,
    )
    .unwrap();
    let selected = state.selected.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(&selected.controls[&req.operation]).unwrap(),
        original_control
    );
    let epoch = publisher.sync_mode_snapshot().unwrap().epoch.unwrap();
    let checkpoint = &selected.epochs[&epoch];
    assert_eq!(
        checkpoint.root.as_ref().unwrap().sha256,
        outcome.root_sha256
    );
    assert!(checkpoint
        .catalog
        .iter()
        .any(|entry| entry.bytes == catalog.bytes));
    assert!(wire.requests()[before_publish..]
        .iter()
        .any(|request| request.starts_with("POST ")));

    let receiver = receiver_fixture.prepared(req.group, &req.binding_sha256);
    assert_ne!(receiver.actor_id, publisher.actor_id);
    receiver
        .set_sync_mode(
            &receiver.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::SharedGroup {
                group: req.group,
                binding_sha256: req.binding_sha256.clone(),
                root_id: outcome.root_id.clone(),
            },
        )
        .unwrap();
    let consumer = SelectedMemory::new(receiver.clone(), identity).unwrap();
    assert!(consumer.inspect(&initial.key).is_err());
    let before_receive = wire.requests().len();
    let mut receiving = start(receiver.clone());
    wait(&receiving.0, &mut || {
        consumer.inspect(&initial.key).is_ok_and(|heads| {
            heads.len() == 1
                && heads[0].revision_id == revision
                && heads[0].payload == serde_json::to_value(&edited).unwrap()
        })
    });
    receiving.stop_and_join();
    let received = receiver
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    // The recipient mints its local transaction identity, not new portable bytes.
    let mut expected = sent.transaction.selected.clone();
    expected.transaction_id = received.transaction.operation;
    assert_eq!(received.transaction.selected, expected);
    for reference in &sent.transaction.selected.records {
        assert_eq!(
            publisher.read_object(reference).unwrap(),
            receiver.read_object(reference).unwrap()
        );
    }
    assert_ne!(received.transaction.operation, sent.transaction.operation);
    assert_ne!(
        received.token.store_generation(),
        sent.token.store_generation()
    );
    assert!(receiver.selected_attempt_inventory().unwrap().is_empty());
    let requests = wire.requests();
    assert!(!requests[before_receive..].is_empty());
    assert!(requests[before_receive..]
        .iter()
        .all(|request| request.starts_with("GET ")));
    let query = consumer
        .query(&memory::Query::new("units".into(), 2))
        .unwrap();
    assert_eq!(query.status, memory::Status::Complete);
    assert_eq!(query.context.facts.len(), 1);
    assert_eq!(
        query.context.facts[0].record,
        initial.key.record_id().unwrap()
    );
    assert_eq!(query.context.facts[0].revision, revision);
    assert_eq!(query.context.facts[0].fact, edited);
    let mut ready = false;
    consumer
        .with_current(&query, &mut || {
            ready = true;
            Ok(())
        })
        .unwrap();
    assert!(ready);
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
