use super::*;

#[test]
fn installed_writer_claims_and_queries_memory_for_the_current_draft() -> Result<()> {
    use crate::{
        config::Config,
        conversation::TextScope,
        memory::{self, Provider as _},
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
        storage::digest,
        workflow::{conversation_dispatch::Dispatch, resident_output::SelectedConversationRequest},
        writer,
    };

    struct WriterModel {
        reply: &'static str,
        parts: Vec<String>,
        calls: Rc<RefCell<Vec<Vec<String>>>>,
    }
    impl crate::LLMEngine for WriterModel {
        fn add_text_content(&mut self, text: &str) {
            self.parts.push(text.into());
        }

        fn add_image_content(&mut self, _: &str) {
            panic!("installed Writer refinement should use text only");
        }

        fn clear_content(&mut self) {
            self.parts.clear();
        }

        fn execute(&mut self) -> Result<String> {
            self.calls.borrow_mut().push(self.parts.clone());
            Ok(self.reply.into())
        }
    }

    let mode = "reconnect-source-handoff";
    let releases = vec![released(Instant::now()), released(Instant::now())];
    let (mut workflow, store, base, events, _notify) = workflow(mode, releases, Duration::ZERO);
    let fixture = Fixture::new();
    let mut runtime = fixture.start_with_store(store.clone(), mode, Duration::from_secs(5))?;
    let Reply::Observed(facts) = runtime
        .connection()?
        .observe(operation(), Instant::now() + Duration::from_secs(3))?
    else {
        anyhow::bail!("Writer fixture source unavailable");
    };
    let document = ConversationId::from_bytes(*facts.page().document.as_bytes());
    let page = ConversationId::from_bytes(*facts.page().page.as_bytes());
    runtime.retire_connection();

    let ledger = Ledger::new(store.clone());
    let conversation = ConversationId::new_v4();
    ledger.create(conversation, ConversationId::new_v4(), 1)?;
    let source_text = "The force applied to the block was 6 N.";
    let original = "# Original draft\n\nThe force on the block was 6 N.";
    let revised = "# Refined draft\n\nThe block experienced 6 N.";
    let refinement = "Refine the force note.";
    let document_text = NormalizedDocument {
        document,
        format: SourceFormat::Notebook,
        source_sha256: digest(source_text.as_bytes()),
        extractor_revision: "installed-writer-fixture-v1".into(),
        units: vec![SourceUnit {
            page: Some(1),
            section: None,
            text: source_text.into(),
        }],
    };
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut model = WriterModel {
        reply: original,
        parts: Vec::new(),
        calls: calls.clone(),
    };
    let seed = writer::run(
        &ledger,
        &mut model,
        &writer::Request::Generate {
            conversation,
            operation: ConversationId::new_v4(),
            request: "Write a note about the block.".into(),
            document: document_text.clone(),
            selection: TextScope::Page { page: 1 },
            guidance: None,
        },
        &Config::default().personalization_snapshot()?,
        &mut || false,
    );
    assert_eq!(seed.status, writer::Status::Generated);
    let draft = seed.draft.context("initial Writer draft unavailable")?;
    calls.borrow_mut().clear();
    model.reply = revised;

    let operation = ConversationId::new_v4();
    let request = writer::Request::Refine {
        conversation,
        operation,
        request: refinement.into(),
        draft,
    };
    let slot = Rc::new(RefinementFixtureSlot::new(
        document,
        page,
        request.clone(),
        119,
    )?);
    let owner = workflow.resident_source.as_mut().unwrap();
    owner.selection = InstalledReaderSelection::fixture_with_installed_output();
    owner.session = Some(Box::new(OwnedSession {
        runtime,
        _fixture: fixture,
        events: events.clone(),
        delivery_fault: false,
        observe_file_change: None,
        writer_slot_fault: false,
        refinement_slot: Some(slot.clone()),
        writer_slot: None,
    }));
    owner.reconnect = true;

    let profile = ConversationId::new_v4();
    let mut config = Config::default();
    config.memory.enabled = true;
    config.memory.profile = Some(profile);
    let dispatch = Dispatch::local(&config, &ledger)?;
    let memory = dispatch
        .local_memory()
        .context("local memory unavailable")?;
    {
        let profile_provider: Arc<dyn memory::Provider> = memory.clone();
        let queued = SelectedConversationRequest::WriterRefinement {
            request: request.clone(),
            require_current_draft: true,
        };
        let planned = SelectedConversationRequest::WriterRefinement {
            request,
            require_current_draft: false,
        };
        assert!(queued.installed_writer_memory(&store, None)?.is_none());
        assert!(planned
            .installed_writer_memory(&store, Some(&profile_provider))?
            .is_none());
        assert!(SelectedConversationRequest::BlankStart(None)
            .installed_writer_memory(&store, Some(&profile_provider))?
            .is_none());
        let selected: Arc<dyn memory::Provider> = Arc::new(memory::selected::SelectedMemory::new(
            store.clone(),
            memory::catalog::Identity {
                group: ConversationId::new_v4(),
                profile,
                binding_sha256: digest(b"selected Writer memory control"),
            },
        )?);
        assert!(queued
            .installed_writer_memory(&store, Some(&selected))
            .is_err());
    }
    let value = "Use SI units when describing force.";
    memory.put(memory::Mutation {
        operation: ConversationId::new_v4(),
        expected: Default::default(),
        fact: memory::Fact {
            key: memory::Key {
                profile,
                scope: memory::Scope::Document { document },
                fact: ConversationId::new_v4(),
            },
            claim: "force units".into(),
            value: value.into(),
            subject: "force".into(),
            tags: vec![],
            provenance: memory::Provenance {
                basis: memory::Basis::UserConfirmed,
                description: "Synthetic installed Writer memory".into(),
                conversation: None,
                turn: None,
                source: None,
            },
            confidence_basis_points: 1000,
            observed_ms: 1,
            expires_ms: None,
        },
    })?;
    let foreign = Arc::new(memory::LocalMemory::new_local(
        Arc::new(Store::open(StorePaths {
            data: base.join("foreign-data"),
            cache: base.join("foreign-cache"),
            credentials: base.join("foreign-credentials"),
        })?),
        profile,
    )?);
    let mut installed = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
    installed.set_subject_memory(Some(foreign.clone()));
    let expected = ledger.expected(Namespace::Conversation, conversation)?;
    let pending = slot.at_release(120)?.context("Writer request missing")?;
    let error = installed.run_iteration().unwrap_err();
    assert!(
        error.to_string().contains("foreign Writer memory Store"),
        "{error:#}"
    );
    assert_eq!(slot.at_release(120)?, Some(pending));
    assert!(!events.borrow().contains(&"writer claim"));
    assert!(calls.borrow().is_empty());
    assert_eq!(foreign.query_count(), 0);
    assert_eq!(
        ledger.expected(Namespace::Conversation, conversation)?,
        expected
    );

    installed.set_subject_memory(Some(memory.clone()));
    // This fixture stops at placement. The real Writer receiver has already
    // saved the generated draft when the absent output backend is reported.
    let error = installed.run_iteration().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("resident output backend unavailable"),
        "{error:#}"
    );
    assert!(slot.at_release(120)?.is_none());
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|entry| **entry == "writer claim")
            .count(),
        1
    );
    assert_eq!(memory.query_count(), 1);
    let generated = ledger.writer_turn(writer::stage(operation, "draft"), conversation)?;
    assert_eq!(generated.outcome, Outcome::Generated);
    assert_eq!(generated.text.as_deref(), Some(revised));

    {
        let calls = calls.borrow();
        let [parts] = calls.as_slice() else {
            panic!("expected one Writer model call, got {}", calls.len());
        };
        let mut contexts = parts
            .iter()
            .filter(|part| part.starts_with(memory::context::LOCAL_SNAPSHOT_POLICY));
        assert!(contexts
            .next()
            .context("missing labeled memory part")?
            .contains(value));
        assert!(contexts.next().is_none());
        let payload: serde_json::Value =
            serde_json::from_str(parts.last().context("missing Writer payload")?)?;
        assert_eq!(payload["request"], refinement);
        assert_eq!(payload["selected_draft"], original);
        assert_eq!(
            payload["source_provenance"]["document"],
            serde_json::json!(document)
        );
        assert_eq!(
            payload["source_units"],
            serde_json::json!(document_text.units)
        );
    }
    drop((installed, slot, foreign, memory, dispatch, ledger, store));
    std::fs::remove_dir_all(base)?;
    Ok(())
}
