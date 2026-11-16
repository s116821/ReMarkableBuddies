use super::*;
use crate::{
    conversation::{
        IntentRecordRef, Ledger, SelectedDomainProjection, SelectedWriterPrepare,
        SelectedWriterSource, TextScope,
    },
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::selection::{
        SelectedReadBudget, SelectedSnapshot, SelectionChange, SelectionMutation,
    },
    writer,
};

fn document_source(document: Uuid) -> NormalizedDocument {
    NormalizedDocument {
        document,
        format: SourceFormat::Notebook,
        source_sha256: digest(b"mixed receiving source"),
        extractor_revision: "receiving-readiness-fixture".into(),
        units: vec![SourceUnit {
            page: Some(1),
            section: None,
            text: "Use SI units.".into(),
        }],
    }
}

fn produce_mixed_document(
    store: Arc<Store>,
    identity: &catalog::Identity,
    scope: &SelectionScope,
    document: Uuid,
    conversation: Uuid,
    remote_root: &str,
) -> (memory::Fact, SelectedSnapshot) {
    let root = store
        .heads(Namespace::Conversation, conversation)
        .unwrap()
        .pop()
        .unwrap();
    let bytes = serde_json::to_vec(&root).unwrap();
    let object = ObjectRef {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
    };
    let operation = Uuid::new_v4();
    let initial = store
        .initialize_selected(
            scope,
            SelectionChange {
                operation,
                accepted_base_sha256: genesis_base(remote_root, &scope.key_sha256),
                selected: Manifest {
                    format: FORMAT,
                    transaction_id: operation,
                    scope: Scope::SelectedRecords,
                    records: vec![object.clone()],
                    record_namespaces: BTreeMap::from([(
                        object.sha256.clone(),
                        Namespace::Conversation,
                    )]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(object.sha256.clone(), bytes)]),
        )
        .unwrap();
    let admission = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
    let mut model = DraftModel(0);
    let generated = writer::selected::run(
        &admission,
        &writer::selected::Selection {
            accepted: initial.token,
            root: IntentRecordRef {
                namespace: root.namespace,
                record_id: root.record_id,
                revision_id: root.revision_id,
                object,
            },
            refine: None,
        },
        &mut model,
        &writer::Request::Generate {
            conversation,
            operation: Uuid::new_v4(),
            request: "Write study notes".into(),
            document: document_source(document),
            selection: TextScope::SuppliedNotebook,
            guidance: None,
        },
        &crate::config::Personalization::default()
            .snapshot()
            .unwrap(),
        None,
        &mut || false,
    );
    assert_eq!(model.0, 1);
    assert_eq!(generated.report.status, writer::Status::Generated);
    super::recovery::register_document(
        store.clone(),
        identity,
        catalog::Owner::Document { document },
        remote_root,
    );
    let provider = selected_memory::SelectedMemory::new(store.clone(), identity.clone()).unwrap();
    let mut note = fact(identity.profile);
    note.key.scope = memory::Scope::Document { document };
    provider
        .put(memory::Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: note.clone(),
        })
        .unwrap();
    let snapshot = store.selected_snapshot(scope, MAX_ITEMS).unwrap().unwrap();
    (note, snapshot)
}

struct DraftModel(usize);
impl crate::LLMEngine for DraftModel {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, _: &str) {
        panic!("text Writer sent an image");
    }
    fn clear_content(&mut self) {}
    fn execute(&mut self) -> Result<String> {
        self.0 += 1;
        Ok("# Study notes\n\nUse SI units.".into())
    }
}

fn writer_input(
    admission: &SelectedAdmission,
    snapshot: &SelectedSnapshot,
    conversation: Uuid,
    document: Uuid,
) -> Result<crate::conversation::SelectedWriterInput> {
    let operation = Uuid::new_v4();
    let root = SelectedDomainProjection::from_snapshot(snapshot)?
        .current_reference(Namespace::Conversation, conversation)?;
    let normalized = crate::conversation::NormalizedTextSource::new(
        conversation,
        writer::stage(operation, "user"),
        document_source(document),
        TextScope::SuppliedNotebook,
        None,
    )?;
    admission.selected_writer_input(
        &snapshot.token,
        &SelectedWriterPrepare {
            operation,
            root,
            request: "Check the selected notes.".into(),
            source: SelectedWriterSource::Generate { normalized },
        },
    )
}

fn original_receive(store: &Store, epoch: Uuid, key: &str) -> Receive {
    let state: State = files::json(
        &store.paths.data.join("sync/state.json"),
        MAX_METADATA as u64,
    )
    .unwrap();
    let progress = &state.selected.unwrap().epochs[&epoch].scopes[key];
    assert!(progress.applied.is_none() && progress.chain.is_empty());
    let receive = progress.receive.clone().unwrap();
    assert!(receive.verified && !receive.applied && receive.expected.is_none());
    receive
}

fn interrupt_after_activation(wire: &Wire, receiver: &Arc<Store>, group: Uuid) {
    let mut receiving = fixture_engine(wire, receiver.clone(), group, 32);
    receiver.set_fault(Fault::AfterActivation).unwrap();
    let interrupted = receiving.step().unwrap_err();
    assert!(interrupted
        .to_string()
        .contains("injected storage interruption"));
}

fn assert_unregistered(
    wire: &Wire,
    admission: &SelectedAdmission,
    consumer: &selected_memory::SelectedMemory,
    snapshot: &SelectedSnapshot,
    conversation: Uuid,
    query: &memory::Query,
    key: &memory::Key,
) {
    let view = consumer.query(query).unwrap();
    assert!(matches!(
        view.status,
        memory::Status::Unavailable | memory::Status::CompleteEmpty
    ));
    assert!(view.context.facts.is_empty());
    assert!(consumer.inspect(key).map_or(true, |heads| heads.is_empty()));
    let error = admission
        .with_current(&snapshot.token, |_| -> Result<()> {
            panic!("unregistered admission ran")
        })
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("mixed aggregate is not receive-ready"));
    let document = query.document.unwrap();
    let error = writer_input(admission, snapshot, conversation, document)
        .err()
        .unwrap();
    assert!(error
        .to_string()
        .contains("mixed aggregate is not receive-ready"));
    probe_worker_admission(wire, admission.writer_store(), snapshot, false);
}

struct PublishingWorker(Worker);

impl PublishingWorker {
    fn request_stop(&self) {
        self.0.cancel.store(true, Ordering::Relaxed);
        let _ = self.0.wake.try_send(());
    }

    fn stop_and_join(mut self) {
        self.request_stop();
        self.0.done.recv_timeout(Duration::from_secs(15)).unwrap();
        self.0.handle.take().unwrap().join().unwrap();
    }
}

impl Drop for PublishingWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

#[test]
fn mixed_receiving_products_require_original_registration_before_checkpoint() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let publisher = fixture.store("mixed-publisher");
    let receiver = fixture.store("mixed-receiver");
    let conversation = Uuid::new_v4();
    Ledger::new(publisher.clone())
        .create(conversation, Uuid::new_v4(), 1)
        .unwrap();
    let group = Uuid::new_v4();
    let binding = wire
        .client(publisher.paths.credentials.join("drive.json"), group)
        .binding_digest();
    let publisher = compound(publisher, group, &binding);
    let receiver = compound(receiver, group, &binding);
    assert_ne!(publisher.actor_id, receiver.actor_id);
    choose(&publisher, group, &binding);
    choose(&receiver, group, &binding);
    let identity = catalog::Identity {
        group,
        profile: Uuid::new_v4(),
        binding_sha256: binding.clone(),
    };
    let document = Uuid::new_v4();
    let owner = catalog::Owner::Document { document };
    let scope = identity.aggregate_scope(&owner).unwrap();
    let key = &scope.key_sha256;
    // Reuse the fixture's already-agreed group. The mixed winner itself must
    // come from the actual producer and Worker, with no seeded document slot.
    let root = existing_group(
        &wire,
        group,
        binding,
        identity.profile,
        &scope,
        owner.clone(),
    );
    let (note, sent) = produce_mixed_document(
        publisher.clone(),
        &identity,
        &scope,
        document,
        conversation,
        &root,
    );
    let publishing = PublishingWorker(launch(&wire, publisher.clone(), group, 32));
    until(&publishing.0, || {
        publisher
            .selected_attempt_inventory()
            .unwrap()
            .iter()
            .any(|attempt| {
                attempt.selection.operation == sent.transaction.operation
                    && attempt.selection.scope == scope
                    && attempt.state == SelectedAttemptState::Withdrawn
            })
    });
    publishing.stop_and_join();
    let before_receive = wire.requests().len();

    // Drive the real receiver Engine synchronously and let the same one-shot
    // fault interrupt each successive publication on resumption.
    let epoch = receiver.sync_mode_snapshot().unwrap().epoch.unwrap();
    interrupt_after_activation(&wire, &receiver, group);
    let original = original_receive(&receiver, epoch, key);
    let snapshot = receiver
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.transaction.mutation, SelectionMutation::Initialize);
    assert_eq!(snapshot.transaction.operation, original.operation);
    assert_ne!(snapshot.transaction.operation, sent.transaction.operation);
    assert_eq!(
        snapshot.transaction.accepted_base_sha256,
        original.descriptor.sha256
    );
    assert!(shared::same_membership(
        &snapshot.transaction.selected,
        &sent.transaction.selected
    ));

    let catalog_scope = identity.scope().unwrap();
    assert!(receiver
        .selected_snapshot(&catalog_scope, MAX_ITEMS)
        .unwrap()
        .is_none());
    let admission = SelectedAdmission::new(receiver.clone(), scope.clone()).unwrap();
    let consumer =
        selected_memory::SelectedMemory::new(receiver.clone(), identity.clone()).unwrap();
    let mut query = memory::Query::new("units".into(), 2);
    query.document = Some(document);
    assert_unregistered(
        &wire,
        &admission,
        &consumer,
        &snapshot,
        conversation,
        &query,
        &note.key,
    );

    let checkpoint = receiver.paths.data.join("sync/state.json");
    let before_registration = fs::read(&checkpoint).unwrap();
    interrupt_after_activation(&wire, &receiver, group);
    assert_eq!(fs::read(&checkpoint).unwrap(), before_registration);
    let catalog_initial = receiver
        .selected_snapshot(&catalog_scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_eq!(
        catalog_initial.transaction.mutation,
        SelectionMutation::Initialize
    );
    assert_eq!(
        catalog_initial.transaction.operation,
        Uuid::new_v5(
            &original.operation,
            b"shared-receive:local-catalog-initialize:v1"
        )
    );
    assert_eq!(
        catalog_initial.transaction.accepted_base_sha256,
        original.descriptor.sha256
    );
    assert_eq!(catalog_initial.selected_records.len(), 1);
    assert_unregistered(
        &wire,
        &admission,
        &consumer,
        &snapshot,
        conversation,
        &query,
        &note.key,
    );

    interrupt_after_activation(&wire, &receiver, group);
    assert_eq!(fs::read(&checkpoint).unwrap(), before_registration);
    let still_original = original_receive(&receiver, epoch, key);
    assert_eq!(still_original.operation, original.operation);
    assert_eq!(still_original.descriptor.bytes, original.descriptor.bytes);
    assert_eq!(still_original.manifest, original.manifest);

    let mut budget = SelectedReadBudget::new(64, MAX_ITEMS as u64, MAX_METADATA as u64, MAX_MEDIA);
    let local = receiver
        .selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(local.snapshot.token, snapshot.token);
    let catalog = receiver
        .selected_snapshot_with_budget(&catalog_scope, MAX_ITEMS, &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(
        catalog.snapshot.transaction.mutation,
        SelectionMutation::Commit
    );
    assert_eq!(
        catalog.snapshot.transaction.operation,
        Uuid::new_v5(
            &original.operation,
            b"shared-receive:local-catalog-register:v1"
        )
    );
    assert_eq!(
        catalog
            .accepted_history
            .last()
            .unwrap()
            .transaction
            .operation,
        catalog_initial.transaction.operation
    );
    let catalog =
        catalog::Catalog::project(&identity, &catalog.snapshot, &catalog.accepted_history).unwrap();
    let registration = &catalog.registrations[key];
    let genesis = local.accepted_history.last().unwrap();
    assert_eq!(registration.owner, owner);
    assert_eq!(registration.genesis_operation, original.operation);
    assert_eq!(registration.genesis_sha256, genesis.sha256);
    admission
        .with_current(&snapshot.token, |current| {
            assert_eq!(current.transaction.operation, original.operation);
            Ok(())
        })
        .unwrap();
    let writer = writer_input(&admission, &snapshot, conversation, document).unwrap();
    assert_eq!(writer.source.document.document, document);
    probe_worker_admission(&wire, &receiver, &snapshot, true);
    let view = consumer.query(&query).unwrap();
    assert_eq!(view.status, memory::Status::Complete);
    assert_eq!(view.context.facts.len(), 1);
    assert_eq!(view.context.facts[0].record, note.key.record_id().unwrap());
    let envelope = snapshot
        .selected_records
        .iter()
        .find(|record| {
            record.namespace == Namespace::SubjectMemory
                && record.record_id == view.context.facts[0].record
        })
        .unwrap();
    assert_eq!(view.context.facts[0].revision, envelope.revision_id);
    assert_eq!(view.context.facts[0].fact, note);
    consumer.with_current(&view, &mut || Ok(())).unwrap();
    assert_eq!(fs::read(&checkpoint).unwrap(), before_registration);

    let mut receiving = fixture_engine(&wire, receiver.clone(), group, 32);
    assert_eq!(receiving.step().unwrap(), Status::Current);
    let progress = &receiving.checkpoint.scopes[key];
    assert!(progress.receive.is_none());
    assert_eq!(
        progress.applied.as_ref().unwrap().operation,
        original.operation
    );
    assert_eq!(progress.chain[0].bytes, original.descriptor.bytes);
    assert_eq!(
        receiver
            .selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        snapshot.token
    );
    for reference in &sent.transaction.selected.records {
        assert_eq!(
            receiver.read_object(reference).unwrap(),
            publisher.read_object(reference).unwrap()
        );
    }
    assert!(receiver.selected_attempt_inventory().unwrap().is_empty());
    let requests = wire.requests();
    assert!(!requests[before_receive..].is_empty());
    assert!(requests[before_receive..]
        .iter()
        .all(|request| request.starts_with("GET ")
            && !request.starts_with("GET /drive/v3/files/generateIds?")));
}

fn probe_worker_admission(
    wire: &Wire,
    store: &Arc<Store>,
    snapshot: &SelectedSnapshot,
    registered: bool,
) {
    let scope = snapshot.token.scope();
    let mut worker = fixture_engine(wire, store.clone(), scope.group, 32);
    let view = worker.view().unwrap().unwrap();
    let entry = &view.entries[&scope.key_sha256];
    let (slot, base) = worker.scope_view(&scope.key_sha256, entry, &view).unwrap();
    let before = wire.requests().len();
    let mut budget = 32;
    let error = worker
        .publish_scope(
            entry,
            &view,
            slot,
            base,
            &mut budget,
            Some(store.selected_snapshot(scope, MAX_ITEMS).unwrap().unwrap()),
        )
        .unwrap_err();
    // After registration the real outbound admission succeeds. The independent
    // genesis guard still forbids turning an incoming winner into an upload.
    assert_eq!(
        error.to_string(),
        if registered {
            "initial scope is not admitted at this root genesis"
        } else {
            "mixed aggregate is not receive-ready"
        }
    );
    assert!(worker.checkpoint.scopes[&scope.key_sha256]
        .pending
        .is_none());
    assert_eq!(budget, 32);
    assert_eq!(wire.requests().len(), before);
}
