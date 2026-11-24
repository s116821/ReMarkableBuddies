//! Exercise the public Writer path against the existing bounded HTTP fixture.
use super::*;
use remarkable_reader_buddy::{
    config::Personalization,
    conversation::{
        notebook::NotebookAdmission, ArchivedNormalizedSource, Mode, Role, TextScope, Turn,
    },
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::{digest, Namespace},
    writer, writer_flow, LLMEngine,
};

struct Model(usize);
impl LLMEngine for Model {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, _: &str) {
        panic!("text only");
    }
    fn clear_content(&mut self) {}
    fn execute(&mut self) -> anyhow::Result<String> {
        self.0 += 1;
        Ok("# Study\n\nx = 7 (page 1).".into())
    }
}
fn document() -> NormalizedDocument {
    NormalizedDocument {
        document: Uuid::new_v4(),
        format: SourceFormat::Notebook,
        source_sha256: "a".repeat(64),
        extractor_revision: "capacities-flow-fixture".into(),
        units: vec![SourceUnit {
            page: Some(1),
            section: None,
            text: "x = 7".into(),
        }],
    }
}
fn request(conversation: Uuid, correlation: Uuid) -> writer_flow::Request {
    writer_flow::Request::GenerateAndExport {
        outer_operation: Uuid::new_v4(),
        correlation,
        effect_operation: Uuid::new_v4(),
        generation: writer::Request::Generate {
            conversation,
            operation: Uuid::new_v4(),
            request: "Keep x = 7 and page 1".into(),
            document: document(),
            selection: TextScope::SuppliedNotebook,
            guidance: None,
        },
    }
}
fn destination(api: &Api) -> (CapacitiesDestination, Uuid) {
    let fixture = CapacitiesApi::fixture();
    let space = fixture.space;
    let tag = fixture.tag;
    api.state.lock().unwrap().capacities = Some(fixture);
    (api.capacities_client(space, tag), space)
}
fn run(
    ledger: &Ledger,
    model: &mut Model,
    destination: &CapacitiesDestination,
    request: &writer_flow::Request,
) -> writer_flow::Report {
    let (_tx, rx) = watch::channel(false);
    writer_flow::run_capacities(
        ledger,
        model,
        destination,
        request,
        &Personalization::default().snapshot().unwrap(),
        rx,
    )
}

#[test]
fn writer_capacities_owned_create_reopen_replay_and_space_isolation() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let (client, space) = destination(&api);
    let other = Api::new();
    let (other_client, other_space) = destination(&other);
    let mut model = Model(0);
    let correlation = Uuid::new_v4();
    let request = request(conversation, correlation);
    let first = run(&ledger, &mut model, &client, &request);
    assert_eq!(first.status, writer_flow::Status::Exported);
    assert!(first.observation.destination_polled && first.observation.write_polled);
    assert_eq!(model.0, 1);
    assert_eq!(api.count("POST"), 1);
    let key = CapacitiesAttempt::association_id(conversation, space, correlation);
    assert_ne!(key, correlation);
    let stored = ledger
        .export_association(conversation, key)
        .unwrap()
        .unwrap();
    assert!(stored.attempt.is_none());
    assert_eq!(stored.capacities.as_ref().unwrap().correlation, correlation);
    assert_eq!(
        stored.capacities.as_ref().unwrap().id,
        request.identities().2
    );
    assert_eq!(stored.native_note, first.export.unwrap().native_note);
    let gets = api.count("GET");
    drop(ledger);
    let ledger = Ledger::new(local.open());
    assert_eq!(
        run(&ledger, &mut model, &client, &request).status,
        writer_flow::Status::Historical
    );
    assert_eq!(model.0, 1);
    assert_eq!(api.count("GET"), gets);
    assert_eq!(api.count("POST"), 1);

    // An old outer intent cannot be redirected to another selected space before model admission.
    assert_eq!(
        run(&ledger, &mut model, &other_client, &request).status,
        writer_flow::Status::Refused
    );
    assert_eq!(model.0, 1);
    assert_eq!(other.count("GET") + other.count("POST"), 0);
    let reconcile = writer_flow::Request::ReconcileExport {
        conversation,
        correlation,
    };
    assert_eq!(
        run(&ledger, &mut model, &other_client, &reconcile).status,
        writer_flow::Status::Refused
    );
    assert_eq!(other.count("GET") + other.count("POST"), 0);
    // A genuinely new deliberate intent may use the same portable correlation in a different scope.
    let next = self::request(conversation, correlation);
    assert_eq!(
        run(&ledger, &mut model, &other_client, &next).status,
        writer_flow::Status::Exported
    );
    assert_eq!(model.0, 2);
    assert_eq!(other.count("POST"), 1);
    assert!(ledger
        .export_association(
            conversation,
            CapacitiesAttempt::association_id(conversation, other_space, correlation)
        )
        .unwrap()
        .is_some());
    assert_eq!(
        ledger
            .export_association(conversation, key)
            .unwrap()
            .unwrap(),
        stored
    );
}

#[test]
fn writer_capacities_unknown_create_blocks_model_and_never_resends_while_candidate_reconciles() {
    for lost_id in [true, false] {
        let local = LocalStore::new();
        let (ledger, conversation) = local.ledger();
        let api = Api::new();
        let (client, space) = destination(&api);
        {
            let mut state = api.state.lock().unwrap();
            state.lose_ack = lost_id;
            state.capacities.as_mut().unwrap().fail_readback = !lost_id;
        }
        let mut model = Model(0);
        let correlation = Uuid::new_v4();
        let request = request(conversation, correlation);
        let first = run(&ledger, &mut model, &client, &request);
        assert_eq!(first.status, writer_flow::Status::ReconcileRequired);
        assert!(first.observation.write_polled);
        assert_eq!(api.count("POST"), 1);
        drop(ledger);
        let ledger = Ledger::new(local.open());
        let key = CapacitiesAttempt::association_id(conversation, space, correlation);
        let stored = ledger
            .export_association(conversation, key)
            .unwrap()
            .unwrap();
        assert_eq!(stored.capacities.unwrap().candidate.is_none(), lost_id);
        assert_eq!(
            run(&ledger, &mut model, &client, &request).status,
            writer_flow::Status::ReconcileRequired
        );
        let next = self::request(conversation, correlation);
        assert_eq!(
            run(&ledger, &mut model, &client, &next).status,
            writer_flow::Status::ReconcileRequired
        );
        assert_eq!(model.0, 1);
        assert_eq!(api.count("POST"), 1);
        api.state
            .lock()
            .unwrap()
            .capacities
            .as_mut()
            .unwrap()
            .fail_readback = false;
        let gets = api.count("GET");
        let reconcile = writer_flow::Request::ReconcileExport {
            conversation,
            correlation,
        };
        let result = run(&ledger, &mut model, &client, &reconcile);
        assert_eq!(
            result.status,
            if lost_id {
                writer_flow::Status::ReconcileRequired
            } else {
                writer_flow::Status::Exported
            }
        );
        assert_eq!(result.observation.destination_polled, !lost_id);
        assert!(!result.observation.write_polled);
        assert_eq!(api.count("GET"), gets + if lost_id { 0 } else { 2 });
        assert_eq!(api.count("POST"), 1);
        assert_eq!(model.0, 1);
    }
}

#[test]
fn writer_capacities_cancel_after_create_poll_keeps_uncertainty() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let (client, _) = destination(&api);
    let (tx, rx) = watch::channel(false);
    api.state.lock().unwrap().cancel_post = Some(tx);
    let mut model = Model(0);
    let request = request(conversation, Uuid::new_v4());
    let first = writer_flow::run_capacities(
        &ledger,
        &mut model,
        &client,
        &request,
        &Personalization::default().snapshot().unwrap(),
        rx,
    );
    assert_eq!(first.status, writer_flow::Status::ReconcileRequired);
    assert!(first.observation.write_polled);
    assert_eq!(first.observation.no_send_recorded, None);
    assert_eq!(api.count("POST"), 1);
    assert_eq!(
        run(&ledger, &mut model, &client, &request).status,
        writer_flow::Status::ReconcileRequired
    );
    assert_eq!(model.0, 1);
    assert_eq!(api.count("POST"), 1);
}

#[test]
fn notebook_capacities_flow_uses_existing_published_selection_and_replays_without_model() {
    use writer::notebook::{self, FrozenNotebookPayloads, PayloadContext, ValidatedNotebook};
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let paths = ledger.store().paths.clone();
    drop(ledger);
    let ledger = Ledger::new(Arc::new(Store::prepare_notebook(paths).unwrap()));
    let raw = serde_json::to_vec(&document()).unwrap();
    let source = ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap();
    let operation = Uuid::new_v4();
    let user_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:writer:v1:{operation}:user").as_bytes(),
    );
    let archive = ArchivedNormalizedSource::from_notebook(
        conversation,
        user_id,
        &source,
        TextScope::SuppliedNotebook,
        None,
    )
    .unwrap();
    let preference = Personalization::default().snapshot().unwrap();
    let payloads = FrozenNotebookPayloads::new(
        &source,
        &archive,
        PayloadContext {
            operation,
            request: "Keep x = 7 and page 1",
            personalization: &preference,
            history: &[],
            history_range: None,
            prior: None,
            max_model_calls: 2,
        },
    )
    .unwrap();
    let input = notebook::runner::Input {
        admission: NotebookAdmission {
            plan: payloads.plan().clone(),
            canonical: source.canonical_bytes().to_vec(),
            user: Turn {
                id: user_id,
                conversation,
                exchange: operation,
                sequence: 0,
                role: Role::User,
                mode: Mode::Writer,
                outcome: Outcome::Interpreted,
                text: Some(payloads.plan().request.clone()),
                sources: vec![archive.id],
                correction_of: None,
                created_ms: 0,
                updated_ms: 0,
                completion: None,
            },
            archive,
            expected: ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            guards: vec![],
        },
        payloads,
    };
    let replay = input.clone();
    let api = Api::new();
    let (client, space) = destination(&api);
    let mut model = Model(0);
    let identity = notebook::flow::Identity {
        outer_operation: Uuid::new_v4(),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    let (_tx, rx) = watch::channel(false);
    let result =
        notebook::flow::run_capacities(&ledger, &mut model, &client, input, identity, rx.clone());
    assert_eq!(result.status, writer_flow::Status::Exported);
    assert_eq!(model.0, 2);
    assert!(result.observation.write_polled);
    assert_eq!(api.count("POST"), 1);
    let stored = ledger
        .export_association(
            conversation,
            CapacitiesAttempt::association_id(conversation, space, identity.correlation),
        )
        .unwrap()
        .unwrap();
    assert_eq!(stored.outcome, Outcome::Completed);
    assert!(stored.source_scope.starts_with("normalized-document:"));
    let gets = api.count("GET");
    assert_eq!(
        notebook::flow::run_capacities(&ledger, &mut model, &client, replay, identity, rx).status,
        writer_flow::Status::Historical
    );
    assert_eq!(model.0, 2);
    assert_eq!(api.count("POST"), 1);
    assert_eq!(api.count("GET"), gets);
}
