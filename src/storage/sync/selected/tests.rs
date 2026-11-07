use super::*;
use crate::memory::{self, catalog, selected as selected_memory, Provider};
use crate::storage::drive::shared_fixture::Wire;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-shared-worker-{}", Uuid::new_v4())))
    }
    fn store(&self, name: &str) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join(name).join("data"),
                cache: self.0.join(name).join("cache"),
                credentials: self.0.join(name).join("credentials"),
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

fn choose(store: &Store, group: Uuid, binding: &str) {
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::SharedGroup {
                group,
                binding_sha256: binding.into(),
                root_id: "shared-root".into(),
            },
        )
        .unwrap();
}

fn launch(wire: &Wire, store: Arc<Store>, group: Uuid, batch: usize) -> Worker {
    struct Observed(Engine);
    impl WorkerEngine for Observed {
        fn step(&mut self) -> Result<Status> {
            let result = self.0.step();
            if let Err(error) = &result {
                eprintln!("synthetic selected Worker: {error:#}");
            }
            result
        }
        fn health(&self, status: Status) -> serde_json::Value {
            self.0.health(status)
        }
    }
    let transport = wire.client(store.paths.credentials.join("drive.json"), group);
    let binding = Binding {
        group,
        root_id: "shared-root".into(),
        digest: transport.binding_digest(),
    };
    let policy = SyncPolicy {
        batch_items: batch,
        poll_seconds: 5,
        ..Default::default()
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let engine = Engine::new(
        store.clone(),
        policy.clone(),
        transport,
        binding,
        cancel.clone(),
    )
    .unwrap();
    Worker::launch(store, policy, cancel, Observed(engine))
        .unwrap()
        .unwrap()
}

fn until(worker: &Worker, mut condition: impl FnMut() -> bool) {
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
}

fn fact(profile: Uuid) -> memory::Fact {
    memory::Fact {
        key: memory::Key {
            profile,
            scope: memory::Scope::Global,
            fact: Uuid::new_v4(),
        },
        claim: "shared http convention".into(),
        value: "original local value".into(),
        subject: "units".into(),
        tags: vec![],
        provenance: memory::Provenance {
            basis: memory::Basis::UserConfirmed,
            description: "synthetic Worker fixture".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 1,
        expires_ms: None,
    }
}

fn existing_group(
    wire: &Wire,
    group: Uuid,
    binding: String,
    profile: Uuid,
    scope: &SelectionScope,
    owner: catalog::Owner,
) -> String {
    let root = Root {
        schema: WIRE,
        group,
        protocol: "buddy-shared-fixed-slot-v1".into(),
        binding_sha256: binding,
        profile,
        catalog_slot: "catalog-first".into(),
    };
    let root_bytes = serde_json::to_vec(&root).unwrap();
    let root_sha = digest(&root_bytes);
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: root_key(group),
            slot_id: "shared-root".into(),
        },
        "root-body",
        root_bytes,
    );
    let catalog = Catalog {
        schema: WIRE,
        group,
        root_sha256: root_sha.clone(),
        base_sha256: root_sha.clone(),
        slot: "catalog-first".into(),
        operation: Uuid::new_v4(),
        entries: BTreeMap::from([(
            scope.key_sha256.clone(),
            Entry {
                owner: owner.into(),
                genesis_slot: "fact-first".into(),
            },
        )]),
        next_slot: "catalog-next".into(),
    };
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: catalog_key(group),
            slot_id: "catalog-first".into(),
        },
        "catalog-body",
        serde_json::to_vec(&catalog).unwrap(),
    );
    root_sha
}

#[test]
fn selected_memory_edit_crosses_existing_worker_http_and_receiving_local_catalog() {
    transfer(false, false, false);
}

#[test]
fn selected_worker_lost_create_ack_and_passive_restart_keep_original_custody() {
    transfer(true, false, false);
}

#[test]
fn selected_passive_activation_before_checkpoint_resumes_exact_original_receipt() {
    transfer(false, true, false);
}

#[test]
fn compound_stores_use_the_existing_worker_and_original_activation_recovery() {
    transfer(true, true, true);
}

fn compound(store: Arc<Store>, group: Uuid, binding: &str) -> Arc<Store> {
    use crate::storage::compound_generation::{ModeEvidence, Request};
    let token = serde_json::to_value(&store.sync_mode_snapshot().unwrap().token).unwrap();
    let request = Request {
        operation: Uuid::new_v4(),
        actor: store.actor_id,
        source_generation: files::json(&store.paths.data.join("CURRENT"), 128).unwrap(),
        group,
        binding_sha256: binding.into(),
        expected_mode: ModeEvidence {
            actor: store.actor_id,
            sha256: token["sha256"].as_str().unwrap().into(),
        },
    };
    let paths = store.paths.clone();
    drop(store);
    Arc::new(Store::prepare_shared_notebook(paths, request).unwrap())
}

fn transfer(lost_ack: bool, activation_fault: bool, use_compound: bool) {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group = Uuid::new_v4();
    let first = fixture.store("publisher");
    let second = fixture.store("receiver");
    let binding = wire
        .client(first.paths.credentials.join("drive.json"), group)
        .binding_digest();
    let (first, second) = if use_compound {
        (
            compound(first, group, &binding),
            compound(second, group, &binding),
        )
    } else {
        (first, second)
    };
    choose(&first, group, &binding);
    choose(&second, group, &binding);
    let profile = Uuid::new_v4();
    let identity = catalog::Identity {
        group,
        profile,
        binding_sha256: binding.clone(),
    };
    let initial = fact(profile);
    let owner = catalog::Owner::GlobalFact {
        key: initial.key.clone(),
    };
    let scope = identity.aggregate_scope(&owner).unwrap();
    let root_sha = existing_group(&wire, group, binding, profile, &scope, owner);
    // Root/catalog preloading models an already-created group. This test does
    // not claim CreateGroup or production provider qualification coverage.
    let producer = selected_memory::SelectedMemory::new(first.clone(), identity.clone()).unwrap();
    let initialized = producer
        .initialize_aggregate(selected_memory::InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: genesis_base(&root_sha, &scope.key_sha256),
            fact: initial.clone(),
        })
        .unwrap();
    producer
        .initialize_selection(selected_memory::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: root_sha,
            registrations: vec![initialized.registration.unwrap()],
        })
        .unwrap();
    let mut edited = initial.clone();
    edited.value = "edited through the real selected-memory producer".into();
    let edit = Uuid::new_v4();
    producer
        .put(memory::Mutation {
            operation: edit,
            expected: producer
                .inspect(&initial.key)
                .unwrap()
                .into_iter()
                .map(|e| e.revision_id)
                .collect(),
            fact: edited.clone(),
        })
        .unwrap();
    let sent = first.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    if lost_ack {
        wire.lose_create("fact-first");
    }
    let publisher = launch(&wire, first.clone(), group, 32);
    until(&publisher, || {
        first
            .selected_attempt_inventory()
            .unwrap()
            .iter()
            .any(|a| a.selection.scope == scope && a.state == SelectedAttemptState::Withdrawn)
    });
    drop(publisher);
    let before_receive = wire.requests().len();
    if activation_fault {
        second.set_fault(Fault::AfterActivation).unwrap();
    }
    let receiver = launch(&wire, second.clone(), group, 1);
    let mut original_receive = None;
    let receiver = if activation_fault {
        until(&receiver, || {
            let Ok(state) = files::json::<State>(
                &second.paths.data.join("sync/state.json"),
                MAX_METADATA as u64,
            ) else {
                return false;
            };
            let Some(receive) = state
                .selected
                .as_ref()
                .and_then(|s| s.epochs.values().next())
                .and_then(|cp| cp.scopes.get(&scope.key_sha256))
                .and_then(|p| p.receive.as_ref())
            else {
                return false;
            };
            if second
                .selected_receipt(&scope, receive.operation)
                .unwrap()
                .is_some()
            {
                original_receive = Some(receive.operation);
                true
            } else {
                false
            }
        });
        drop(receiver);
        assert!(
            second
                .selected_snapshot(&identity.scope().unwrap(), MAX_ITEMS)
                .unwrap()
                .is_none(),
            "fault must interrupt before receiver-local catalog publication"
        );
        launch(&wire, second.clone(), group, 1)
    } else {
        receiver
    };
    let consumer = selected_memory::SelectedMemory::new(second.clone(), identity.clone()).unwrap();
    until(&receiver, || {
        consumer.inspect(&initial.key).is_ok_and(|heads| {
            heads.iter().any(|e| {
                e.kind == Kind::Value && e.payload == serde_json::to_value(&edited).unwrap()
            })
        })
    });
    drop(receiver);
    let received = second
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    if let Some(original) = original_receive {
        assert_eq!(received.transaction.operation, original);
    }
    assert_eq!(
        received.transaction.selected.records,
        sent.transaction.selected.records
    );
    for reference in &sent.transaction.selected.records {
        assert_eq!(
            first.read_object(reference).unwrap(),
            second.read_object(reference).unwrap()
        );
    }
    assert_ne!(received.transaction.operation, sent.transaction.operation);
    assert_ne!(
        received.token.store_generation(),
        sent.token.store_generation()
    );
    assert!(second.selected_attempt_inventory().unwrap().is_empty());
    assert!(wire.requests()[before_receive..]
        .iter()
        .all(|request| request.starts_with("GET ")));
    // A normal product query succeeds only after receiving-local registration.
    let queried = consumer
        .query(&memory::Query::new("units".into(), 2))
        .unwrap();
    let mut consumed = false;
    consumer
        .with_current(&queried, &mut || {
            consumed = true;
            Ok(())
        })
        .unwrap();
    assert!(consumed);

    // A subsequent genuine product edit uses the accepted remote base. Restart
    // both existing Worker instances, then observe the receiver's cached lineage
    // advancing through its agreed successor with no outbound intent.
    edited.value = "second accepted edit after Worker restart".into();
    producer
        .put(memory::Mutation {
            operation: Uuid::new_v4(),
            expected: producer
                .inspect(&initial.key)
                .unwrap()
                .into_iter()
                .map(|e| e.revision_id)
                .collect(),
            fact: edited.clone(),
        })
        .unwrap();
    let publisher = launch(&wire, first.clone(), group, 1);
    until(&publisher, || {
        first
            .selected_attempt_inventory()
            .unwrap()
            .iter()
            .filter(|a| a.selection.scope == scope && a.state == SelectedAttemptState::Withdrawn)
            .count()
            == 2
    });
    drop(publisher);
    let before_update = wire.requests().len();
    let receiver = launch(&wire, second.clone(), group, 1);
    until(&receiver, || {
        consumer.inspect(&initial.key).is_ok_and(|heads| {
            heads.iter().any(|e| {
                e.kind == Kind::Value && e.payload == serde_json::to_value(&edited).unwrap()
            })
        })
    });
    drop(receiver);
    let updated = second
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_ne!(
        updated.transaction.operation,
        received.transaction.operation
    );
    assert!(second
        .selected_receipt(&scope, received.transaction.operation)
        .unwrap()
        .is_some());
    assert_eq!(
        updated.transaction.selected.records,
        first
            .selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .transaction
            .selected
            .records
    );
    assert!(second.selected_attempt_inventory().unwrap().is_empty());
    assert!(wire.requests()[before_update..]
        .iter()
        .all(|request| request.starts_with("GET ")));
    assert_eq!(
        wire.requests()
            .iter()
            .filter(|request| request.starts_with("POST /drive/v3/files?"))
            .count(),
        2,
        "one fixed-slot create per actual producer edit; no replay or phantom successor"
    );
}

#[test]
fn selected_writer_real_model_entry_publishes_exact_text_source_through_worker() {
    transfer_writer_document(false);
}

#[cfg(target_os = "linux")]
#[test]
fn resident_reader_policy_requires_received_registration_and_current_config() {
    transfer_writer_document(true);
}

fn transfer_writer_document(ready_reader: bool) {
    use crate::{
        conversation::*,
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
        writer,
    };
    struct Model(usize);
    impl crate::LLMEngine for Model {
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, _: &str) {
            panic!("selected text Writer is not recognition");
        }
        fn clear_content(&mut self) {}
        fn execute(&mut self) -> Result<String> {
            self.0 += 1;
            Ok("# Study\n\nx = 7".into())
        }
    }
    let fixture = Fixture::new();
    let wire = Wire::new();
    let first = fixture.store("writer");
    let second = fixture.store("reader");
    let group = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(first.paths.credentials.join("drive.json"), group)
        .binding_digest();
    let second = if ready_reader {
        compound(second, group, &binding)
    } else {
        second
    };
    let identity = catalog::Identity {
        group,
        profile,
        binding_sha256: binding.clone(),
    };
    let document = Uuid::new_v4();
    let conversation = Uuid::new_v4();
    let owner = catalog::Owner::Document { document };
    let scope = identity.aggregate_scope(&owner).unwrap();
    let root_sha = existing_group(&wire, group, binding.clone(), profile, &scope, owner);
    // Seed an ordinary conversation before explicit group selection. The actual
    // selected Writer below creates its Source2/turn/receipt through its real
    // model handoff; this does not stand in for the Source3 notebook runner.
    Ledger::new(first.clone())
        .create(conversation, Uuid::new_v4(), 1)
        .unwrap();
    let root = first
        .heads(Namespace::Conversation, conversation)
        .unwrap()
        .pop()
        .unwrap();
    choose(&first, group, &binding);
    choose(&second, group, &binding);
    let bytes = serde_json::to_vec(&root).unwrap();
    let reference = ObjectRef {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
    };
    let operation = Uuid::new_v4();
    let initialized = first
        .initialize_selected(
            &scope,
            crate::storage::selection::SelectionChange {
                operation,
                accepted_base_sha256: genesis_base(&root_sha, &scope.key_sha256),
                selected: Manifest {
                    format: FORMAT,
                    transaction_id: operation,
                    scope: Scope::SelectedRecords,
                    records: vec![reference.clone()],
                    record_namespaces: BTreeMap::from([(
                        reference.sha256.clone(),
                        Namespace::Conversation,
                    )]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(reference.sha256.clone(), bytes)]),
        )
        .unwrap();
    let writer = SelectedAdmission::new(first.clone(), scope.clone()).unwrap();
    let selected = writer::selected::Selection {
        accepted: initialized.token,
        root: IntentRecordRef {
            namespace: root.namespace,
            record_id: root.record_id,
            revision_id: root.revision_id,
            object: reference,
        },
        refine: None,
    };
    let request = writer::Request::Generate {
        conversation,
        operation: Uuid::new_v4(),
        request: "study notes".into(),
        document: NormalizedDocument {
            document,
            format: SourceFormat::Notebook,
            source_sha256: digest(b"source text"),
            extractor_revision: "synthetic-text-source".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "x = 7".into(),
            }],
        },
        selection: TextScope::SuppliedNotebook,
        guidance: None,
    };
    let mut model = Model(0);
    let generated = writer::selected::run(
        &writer,
        &selected,
        &mut model,
        &request,
        &crate::config::Personalization::default()
            .snapshot()
            .unwrap(),
        None,
        &mut || false,
    );
    assert_eq!(model.0, 1);
    assert!(matches!(
        generated.publication,
        Some(SelectedWriterPublication::Published { .. })
    ));
    let sent = first.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    let publisher = launch(&wire, first.clone(), group, 1);
    until(&publisher, || {
        first
            .selected_attempt_inventory()
            .unwrap()
            .iter()
            .any(|a| a.state == SelectedAttemptState::Withdrawn)
    });
    drop(publisher);
    let before = wire.requests().len();
    let receiver = launch(&wire, second.clone(), group, 1);
    until(&receiver, || {
        second
            .selected_snapshot(&identity.scope().unwrap(), MAX_ITEMS)
            .unwrap()
            .is_some()
    });
    drop(receiver);
    let received = second
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_eq!(
        sent.transaction.selected.records,
        received.transaction.selected.records
    );
    for reference in &sent.transaction.selected.records {
        assert_eq!(
            first.read_object(reference).unwrap(),
            second.read_object(reference).unwrap()
        );
    }
    let reader = SelectedAdmission::new(second.clone(), scope).unwrap();
    reader.with_current(&received.token,|snapshot| {
        let projected = SelectedDomainProjection::from_snapshot(snapshot)?;
        let records = projected.inspect(conversation)?;
        ensure!(records.iter().any(|r|matches!(r,Record::Turn(turn) if turn.text.as_deref()==Some("# Study\n\nx = 7"))),
            "generated draft absent from actual receiving projection");
        Ok(())
    }).unwrap();
    assert!(second.selected_attempt_inventory().unwrap().is_empty());
    assert!(wire.requests()[before..]
        .iter()
        .all(|r| r.starts_with("GET ")));

    #[cfg(target_os = "linux")]
    if ready_reader {
        let path = fixture.0.join("reader-config.json");
        let config = |scope: &SelectionScope, enabled| {
            serde_json::json!({
                "paths": second.paths,
                "reader": { "selection": scope },
                "product_settings": { "reader_enabled": enabled }
            })
        };
        let open = |reader: &SelectedAdmission| {
            ReaderPreferences::from_binding(
                second.clone(),
                path.clone(),
                reader,
                Instant::now(),
                super::Binding {
                    group,
                    root_id: "shared-root".into(),
                    digest: identity.binding_sha256.clone(),
                },
                |store| Ok(wire.client(store.paths.credentials.join("drive.json"), group)),
            )
        };
        files::atomic_json(&path, &config(reader.scope(), true)).unwrap();
        let (policy, token) = open(&reader).unwrap();
        assert_eq!(token, received.token);
        policy.check_progress(&reader, &token).unwrap();

        let mut document_fact = fact(profile);
        document_fact.key.scope = memory::Scope::Document { document };
        selected_memory::SelectedMemory::new(second.clone(), identity.clone())
            .unwrap()
            .put(memory::Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact: document_fact,
            })
            .unwrap();
        assert!(policy.check_progress(&reader, &token).is_err());
        let (policy, token) = open(&reader).unwrap();
        assert_ne!(token, received.token);
        assert_eq!(
            token.accepted_base_sha256(),
            received.token.accepted_base_sha256()
        );
        policy.check_progress(&reader, &token).unwrap();

        let mut active = config(reader.scope(), true);
        active["personalization"] =
            serde_json::json!({"enabled": true, "text": "Keep the answer concise."});
        active["memory"] =
            serde_json::json!({"enabled": true, "profile": profile, "selection": identity});
        files::atomic_json(&path, &active).unwrap();
        let (policy, token) = open(&reader).unwrap();
        resident_reader_fixture_generated(&second, &reader, policy, token, document);

        let (policy, token) = open(&reader).unwrap();
        files::atomic_json(&path, &config(reader.scope(), false)).unwrap();
        assert!(policy.check_progress(&reader, &token).is_err());
        assert!(open(&reader).is_err());
        let foreign = SelectionScope {
            key_sha256: digest(b"unregistered Reader fixture"),
            ..reader.scope().clone()
        };
        files::atomic_json(&path, &config(&foreign, true)).unwrap();
        let unregistered = SelectedAdmission::new(second.clone(), foreign).unwrap();
        assert!(open(&unregistered).is_err());
    }
}

#[cfg(target_os = "linux")]
fn resident_reader_fixture_generated(
    store: &Arc<Store>,
    reader: &SelectedAdmission,
    policy: ReaderPreferences,
    token: SelectionToken,
    document: Uuid,
) {
    use crate::conversation::{
        resident_capture::tests::capture_for, selected_reader_prepare::ResidentPrepared, Ledger,
        Outcome, Record, SelectedDomainProjection,
    };
    use crate::llm::ModelHandoff;
    use crate::workflow::{orchestrator::Orchestrator, Workflow};
    use std::sync::Mutex;

    struct Model {
        images: Vec<String>,
        submitted: Arc<Mutex<Vec<Vec<String>>>>,
    }
    impl crate::LLMEngine for Model {
        fn guarded_timeout(&self) -> Option<Duration> {
            Some(Duration::from_secs(1))
        }
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, image: &str) {
            self.images.push(image.into());
        }
        fn clear_content(&mut self) {
            self.images.clear();
        }
        fn execute(&mut self) -> Result<String> {
            panic!("resident Reader bypassed owned model submission");
        }
        fn execute_with_handoff(
            &mut self,
            progress: &mut dyn FnMut() -> Result<()>,
            handoff: &mut ModelHandoff<'_>,
        ) -> Result<String> {
            progress()?;
            let mut stage = 0;
            handoff(&mut || {
                let mut requests = self.submitted.lock().unwrap();
                requests.push(self.images.clone());
                stage = requests.len();
                Ok(())
            })?;
            progress()?;
            Ok(match stage {
                1 => "QUESTION: What is x?\nQUESTION_BOX: 10,10,100,100\nSELECTION_CENTER: 100,100\n---\nANSWER: proposed",
                2 => "TRANSCRIPTION: What is x?",
                3 => "ANSWER: x = 7",
                _ => panic!("resident Reader exceeded its three model calls"),
            }.into())
        }
    }

    let capture = capture_for(document);
    let prepared = ResidentPrepared::new(store, &token, &capture, Uuid::new_v4(), 1).unwrap();
    let (token, acknowledgment) = {
        let mut reads = policy.reads(ReaderPhase::Acquisition).unwrap();
        let [catalog, preferences] = policy.gates();
        SelectedAdmission::with_gates(&[reader, catalog, preferences], |store| {
            policy.verify_locked(store, reader, &token, &mut reads)?;
            prepared.publish_locked(store, &token, &mut reads)
        })
        .unwrap()
    };
    let root = acknowledgment.root.record_id;
    let submitted = Arc::new(Mutex::new(vec![]));
    let workflow = Workflow::selected_owned_reader_fixture(Arc::new(reader.clone()));
    let model = Model {
        images: vec![],
        submitted: submitted.clone(),
    };
    let mut receiver = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
    receiver.set_personalization(policy.config().personalization_snapshot().unwrap());
    receiver.set_selected_subject_memory(
        memory::configured(store.clone(), &policy.config().memory).unwrap(),
    );
    receiver
        .receive_resident_prepared(token, acknowledgment, policy)
        .unwrap();

    let requests = submitted.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].len(), 4);
    assert!(requests.iter().all(|images| images == &requests[0]));
    let selected = store
        .selected_snapshot(reader.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let projection = SelectedDomainProjection::from_snapshot(&selected).unwrap();
    assert!(projection.inspect(root).unwrap().iter().any(|record| {
        matches!(record, Record::Turn(turn) if turn.outcome == Outcome::Generated && turn.text.as_ref().is_some_and(|text| text.contains("x = 7")))
    }));
}
