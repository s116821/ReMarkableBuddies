use super::*;
use crate::{
    config::Personalization,
    note_export::{self, NoteDestination},
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::StorePaths,
    LLMEngine,
};
use std::{collections::BTreeMap, sync::Mutex};

struct Fixture {
    root: PathBuf,
    ledger: Ledger,
    request: Request,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("notebook-command-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let store = Store::open(paths.clone()).unwrap();
        let ledger = Ledger::new(Arc::new(store));
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        drop(ledger);
        let document = NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "command-fixture-v1".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "Force = 10 N.".into(),
            }],
        };
        let bytes = serde_json::to_vec(&document).unwrap();
        let path = root.join("source.json");
        std::fs::write(&path, &bytes).unwrap();
        let request = Request::GenerateNotebook(Box::new(Generate {
            conversation,
            operation: Uuid::new_v4(),
            request: "Generate study notes".into(),
            document_file: path,
            document_sha256: digest(&bytes),
            selection: TextScope::SuppliedNotebook,
            guidance: None,
            max_model_calls: 2,
            history_range: None,
        }));
        let preference = Personalization::default().snapshot().unwrap();
        let (store, prepared) =
            Store::prepare_notebook_validated(paths, |l| prepare(l, &request, &preference))
                .unwrap();
        assert!(matches!(prepared, Prepared::Fresh(_)));
        Self {
            root,
            ledger: Ledger::new(Arc::new(store)),
            request,
        }
    }
    fn input(&self, request: &Request) -> runner::Input {
        let preference = Personalization::default().snapshot().unwrap();
        let Prepared::Fresh(input) = prepare(&self.ledger, request, &preference).unwrap() else {
            panic!("fresh input required")
        };
        *input
    }
    fn reopen(&mut self) {
        let paths = self.ledger.store().paths.clone();
        let replacement = Ledger::new(Arc::new(
            Store::open(StorePaths {
                data: self.root.join("other/data"),
                cache: self.root.join("other/cache"),
                credentials: self.root.join("other/credentials"),
            })
            .unwrap(),
        ));
        drop(std::mem::replace(&mut self.ledger, replacement));
        self.ledger = Ledger::new(Arc::new(Store::open(paths).unwrap()));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[derive(Default)]
struct Model {
    calls: usize,
    parts: Vec<String>,
}
impl LLMEngine for Model {
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(text.into());
    }
    fn add_image_content(&mut self, _: &str) {
        panic!("text model only")
    }
    fn clear_content(&mut self) {
        self.parts.clear();
    }
    fn execute(&mut self) -> Result<String> {
        self.calls += 1;
        assert!(self.parts.len() >= 2);
        Ok(if self.calls % 2 == 1 {
            "Force = 10 N. (page 1)."
        } else if self.calls >= 4 {
            "# Force\n\nForce = 10 N. (page 1). Refined."
        } else {
            "# Force\n\nForce = 10 N. (page 1)."
        }
        .into())
    }
}
#[derive(Default)]
struct Destination(Mutex<(BTreeMap<String, note_export::RemoteNote>, usize, usize)>);
impl NoteDestination for Destination {
    fn backend(&self) -> &str {
        "fixture"
    }
    fn correlation_location(&self) -> note_export::CorrelationLocation {
        note_export::CorrelationLocation::Title
    }
    async fn discover(
        &self,
        _: Uuid,
        _: note_export::Budget,
    ) -> note_export::Result<note_export::Discovery> {
        self.0.lock().unwrap().2 += 1;
        Ok(note_export::Discovery::Missing)
    }
    async fn read(
        &self,
        id: &str,
        _: note_export::Budget,
    ) -> note_export::Result<Option<note_export::RemoteNote>> {
        let mut state = self.0.lock().unwrap();
        state.2 += 1;
        Ok(state.0.get(id).cloned())
    }
    async fn create(
        &self,
        id: &str,
        content: &str,
        _: Uuid,
        _: note_export::Budget,
    ) -> note_export::Result<note_export::RemoteNote> {
        self.write(id, content, 1)
    }
    async fn update(
        &self,
        id: &str,
        content: &str,
        version: u64,
        _: Uuid,
        _: note_export::Budget,
    ) -> note_export::Result<note_export::RemoteNote> {
        self.write(id, content, version + 1)
    }
}
impl Destination {
    fn write(
        &self,
        id: &str,
        content: &str,
        version: u64,
    ) -> note_export::Result<note_export::RemoteNote> {
        let note = note_export::RemoteNote {
            id: id.into(),
            title: content
                .lines()
                .next()
                .unwrap()
                .trim_start_matches("# ")
                .into(),
            content: content.into(),
            version,
            trashed: false,
            metadata: BTreeMap::new(),
        };
        let mut state = self.0.lock().unwrap();
        state.0.insert(id.into(), note.clone());
        state.1 += 1;
        state.2 += 1;
        Ok(note)
    }
}
fn identity() -> flow::Identity {
    flow::Identity {
        outer_operation: Uuid::new_v4(),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    }
}

#[test]
fn strict_commands_reject_nested_variants_duplicate_unknown_and_identity_collisions() {
    let value = serde_json::json!({"command":"generate-notebook","conversation":Uuid::new_v4(),"operation":Uuid::new_v4(),"request":"note","document_file":"/source.json","document_sha256":"a".repeat(64),"selection":{"scope":"supplied-notebook"},"max_model_calls":2});
    // Use the typed serializer so the scope representation stays the production one.
    let request = Request::GenerateNotebook(Box::new(Generate {
        conversation: Uuid::new_v4(),
        operation: Uuid::new_v4(),
        request: "note".into(),
        document_file: PathBuf::from("/source.json"),
        document_sha256: "a".repeat(64),
        selection: TextScope::SuppliedNotebook,
        guidance: None,
        max_model_calls: 2,
        history_range: None,
    }));
    request.preflight().unwrap();
    let mut json = serde_json::to_value(&request).unwrap();
    json["unknown"] = value;
    assert!(serde_json::from_value::<Request>(json).is_err());
    let bytes = serde_json::to_string(&request).unwrap();
    let duplicate = bytes.replacen("{", "{\"command\":\"generate-notebook\",", 1);
    assert!(serde_json::from_str::<Request>(&duplicate).is_err());
    let joined = Request::GenerateNotebookAndExport {
        outer_operation: request.operation(),
        generation: Box::new(request),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    assert!(joined.preflight().is_err());
    let nested = Request::GenerateNotebookAndExport {
        outer_operation: Uuid::new_v4(),
        generation: Box::new(joined),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    assert!(nested.preflight().is_err());
}
#[test]
#[cfg(target_os = "linux")]
fn generation_replay_uses_retained_archive_after_transport_file_disappears_and_refines_after_restart(
) {
    let mut f = Fixture::new();
    let mut model = Model::default();
    let generated = runner::run(&f.ledger, &mut model, f.input(&f.request), &mut || false);
    assert_eq!(generated.report.status, runner::Status::Generated);
    let draft = generated.report.draft.unwrap();
    drop(generated);
    let Request::GenerateNotebook(input) = &f.request else {
        unreachable!()
    };
    std::fs::remove_file(&input.document_file).unwrap();
    f.reopen();
    let preference = Personalization::default().snapshot().unwrap();
    let Prepared::Replay(replay) = prepare(&f.ledger, &f.request, &preference).unwrap() else {
        panic!("historical operation required")
    };
    assert_eq!(replay.draft, Some(draft));
    assert_eq!(model.calls, 2);
    let refine = Request::RefineNotebook(Box::new(Refine {
        conversation: f.request.conversation(),
        operation: Uuid::new_v4(),
        request: "Keep the equation and page reference".into(),
        draft,
        max_model_calls: 2,
        history_range: None,
    }));
    let result = runner::run(&f.ledger, &mut model, f.input(&refine), &mut || false);
    assert_eq!(result.report.status, runner::Status::Generated);
    assert_eq!(model.calls, 4);
}
#[test]
#[cfg(target_os = "linux")]
fn joined_create_refine_update_and_explicit_saved_export_keep_one_correlation() {
    let mut f = Fixture::new();
    let mut model = Model::default();
    let destination = Destination::default();
    let first_id = identity();
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let first = flow::run(
        &f.ledger,
        &mut model,
        &destination,
        f.input(&f.request),
        first_id,
        receiver,
    );
    assert_eq!(first.status, crate::writer_flow::Status::Exported);
    let first_draft = first.draft.unwrap();
    assert_eq!(model.calls, 2);
    assert_eq!(destination.0.lock().unwrap().1, 1);
    f.reopen();
    let refine = Request::RefineNotebook(Box::new(Refine {
        conversation: f.request.conversation(),
        operation: Uuid::new_v4(),
        request: "Make a concise study note".into(),
        draft: first_draft,
        max_model_calls: 2,
        history_range: None,
    }));
    let next = flow::Identity {
        correlation: first_id.correlation,
        ..identity()
    };
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let result = flow::run(
        &f.ledger,
        &mut model,
        &destination,
        f.input(&refine),
        next,
        receiver,
    );
    assert_eq!(result.status, crate::writer_flow::Status::Exported);
    assert_eq!(model.calls, 4);
    assert_eq!(destination.0.lock().unwrap().0.len(), 1);
    assert_eq!(destination.0.lock().unwrap().1, 2);
    let saved = crate::writer_flow::Request::ExportDraft {
        conversation: f.request.conversation(),
        outer_operation: Uuid::new_v4(),
        draft: result.draft.unwrap(),
        correlation: first_id.correlation,
        effect_operation: Uuid::new_v4(),
    };
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let preference = Personalization::default().snapshot().unwrap();
    let result = crate::writer_flow::run(
        &f.ledger,
        &mut model,
        &destination,
        &saved,
        &preference,
        receiver,
    );
    assert_eq!(result.status, crate::writer_flow::Status::Exported);
    assert_eq!(model.calls, 4);
    assert_eq!(destination.0.lock().unwrap().0.len(), 1);
}
#[test]
#[cfg(target_os = "linux")]
fn joined_root_change_before_model_stops_without_provider_or_destination_write() {
    let f = Fixture::new();
    let mut model = Model::default();
    let destination = Destination::default();
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let input = f.input(&f.request);
    let conversation = f.request.conversation();
    let result = flow::run_with_events(
        &f.ledger,
        &mut model,
        &destination,
        input,
        identity(),
        receiver,
        &mut |phase, ledger| {
            if phase == crate::writer_flow::Phase::BeforeModel {
                ledger.create(Uuid::new_v4(), Uuid::new_v4(), 1).unwrap();
                // A concurrent related root revision must invalidate the carried snapshot.
                let root = ledger
                    .expected(Namespace::Conversation, conversation)
                    .unwrap();
                let _ = ledger
                    .admit_writer_flow(crate::conversation::WriterFlowAdmission {
                        conversation,
                        operation: Uuid::new_v4(),
                        fingerprint: "b".repeat(64),
                        correlation: Uuid::new_v4(),
                        effect: Uuid::new_v4(),
                        draft: Uuid::new_v4(),
                        expected: root,
                    })
                    .unwrap();
            }
        },
    );
    assert_ne!(result.status, crate::writer_flow::Status::Exported);
    assert_eq!(model.calls, 0);
    assert_eq!(destination.0.lock().unwrap().1, 0);
}

#[test]
#[cfg(target_os = "linux")]
fn replay_reconstructs_original_subrange_despite_foreign_conversation_domain_and_later_history() {
    let mut f = Fixture::new();
    let conversation = f.request.conversation();
    let append = |ledger: &Ledger, conversation: Uuid, text: &str| {
        ledger
            .append(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, conversation)
                    .unwrap(),
                Turn {
                    id: Uuid::new_v4(),
                    conversation,
                    exchange: Uuid::new_v4(),
                    sequence: 0,
                    role: Role::User,
                    mode: Mode::Reader,
                    outcome: Outcome::Interpreted,
                    text: Some(text.into()),
                    sources: vec![],
                    correction_of: None,
                    created_ms: 1,
                    updated_ms: 1,
                    completion: None,
                },
                vec![],
            )
            .unwrap();
    };
    append(&f.ledger, conversation, "Excluded earlier context");
    append(&f.ledger, conversation, "Selected equation context");
    let foreign = Uuid::new_v4();
    f.ledger.create(foreign, Uuid::new_v4(), 1).unwrap();
    append(&f.ledger, foreign, "Foreign context");
    let store = f.ledger.store();
    store
        .commit(
            vec![crate::storage::Envelope {
                envelope_version: 1,
                namespace: Namespace::SubjectMemory,
                domain_schema_version: 1,
                record_id: Uuid::new_v4(),
                revision_id: Uuid::new_v4(),
                parents: Default::default(),
                operation_id: Uuid::new_v4(),
                actor_id: store.actor_id,
                kind: crate::storage::Kind::Value,
                payload: serde_json::json!({"fixture":"unrelated domain"}),
                media_descriptors: vec![],
            }],
            BTreeMap::new(),
        )
        .unwrap();
    let Request::GenerateNotebook(input) = &mut f.request else {
        unreachable!()
    };
    input.history_range = Some(TurnRange {
        start: 1,
        end_exclusive: 2,
    });
    let mut model = Model::default();
    let generated = runner::run(&f.ledger, &mut model, f.input(&f.request), &mut || false);
    assert_eq!(generated.report.status, runner::Status::Generated);
    drop(generated);
    append(
        &f.ledger,
        conversation,
        "Later mutable history must not replace frozen context",
    );
    f.reopen();
    let preference = Personalization::default().snapshot().unwrap();
    let Prepared::Replay(replay) = prepare(&f.ledger, &f.request, &preference).unwrap() else {
        panic!("unchanged retry must be historical")
    };
    let history = initial_history(
        &f.ledger,
        &replay.plan,
        Some(TurnRange {
            start: 1,
            end_exclusive: 2,
        }),
    )
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].text.as_deref(),
        Some("Selected equation context")
    );
    assert_eq!(model.calls, 2);
    let identity = identity();
    let joined = Request::GenerateNotebookAndExport {
        outer_operation: identity.outer_operation,
        generation: Box::new(f.request.clone()),
        correlation: identity.correlation,
        effect_operation: identity.effect_operation,
    };
    let destination = Destination::default();
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let historical = joined
        .run_joined_prepared(
            Prepared::Replay(replay),
            &f.ledger,
            &mut model,
            &destination,
            receiver,
            &mut |_, _| {},
        )
        .unwrap();
    assert_eq!(historical.status, crate::writer_flow::Status::Historical);
    assert_eq!(historical.generation.unwrap().local_invocations, 0);
    assert_eq!(model.calls, 2);
    assert_eq!(destination.0.lock().unwrap().2, 0);
    let Request::GenerateNotebook(input) = &mut f.request else {
        unreachable!()
    };
    input.history_range = Some(TurnRange {
        start: 0,
        end_exclusive: 1,
    });
    assert!(prepare(&f.ledger, &f.request, &preference).is_err());
    assert_eq!(model.calls, 2);
}

#[test]
#[cfg(target_os = "linux")]
fn joined_export_rechecks_archive_bytes_at_both_destination_poll_boundaries() {
    for boundary in [
        crate::writer_flow::Phase::BeforeDestinationPoll,
        crate::writer_flow::Phase::BeforeWritePoll,
    ] {
        let f = Fixture::new();
        let mut model = Model::default();
        let destination = Destination::default();
        let input = f.input(&f.request);
        let hash = input.admission.archive.media.sha256.clone();
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let mut fired = false;
        let result = flow::run_with_events(
            &f.ledger,
            &mut model,
            &destination,
            input,
            identity(),
            receiver,
            &mut |phase, ledger| {
                if phase == boundary && !fired {
                    fired = true;
                    assert!(
                        crate::writer::Owner::acquire(ledger.store()).is_none(),
                        "Writer owner must span destination transport"
                    );
                    assert!(
                        note_export::ExportOwner::acquire(ledger).is_none(),
                        "export owner must span destination transport"
                    );
                    let generation: Uuid = serde_json::from_slice(
                        &std::fs::read(ledger.store().paths.data.join("CURRENT")).unwrap(),
                    )
                    .unwrap();
                    std::fs::write(
                        ledger
                            .store()
                            .paths
                            .data
                            .join("generations")
                            .join(generation.to_string())
                            .join("objects")
                            .join(&hash),
                        b"corrupt archive",
                    )
                    .unwrap();
                }
            },
        );
        assert!(fired);
        assert_eq!(model.calls, 2);
        assert_eq!(destination.0.lock().unwrap().1, 0);
        assert!(!result.observation.write_polled);
        assert_ne!(result.status, crate::writer_flow::Status::Exported);
    }
}
