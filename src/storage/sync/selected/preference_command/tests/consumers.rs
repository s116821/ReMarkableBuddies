use super::*;

fn consumer_writer(
    fixture: &Fixture,
) -> (
    SelectedAdmission,
    crate::writer::selected::Selection,
    crate::writer::Request,
) {
    use crate::conversation::{Record, Root as ConversationRoot, SelectedDomainProjection};
    let conversation = Uuid::new_v4();
    let document = Uuid::new_v4();
    let operation = Uuid::new_v4();
    let root = Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: conversation,
        revision_id: Uuid::new_v4(),
        parents: Default::default(),
        operation_id: operation,
        actor_id: fixture.store.actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(Record::Root(ConversationRoot {
            id: conversation,
            next_sequence: 0,
            binding: None,
            created_ms: 1,
            updated_ms: 1,
        }))
        .unwrap(),
        media_descriptors: vec![],
    };
    let bytes = serde_json::to_vec(&root).unwrap();
    let hash = digest(&bytes);
    let scope = SelectionScope {
        group: fixture.group,
        key_sha256: crate::memory::catalog::document_key(document).unwrap(),
        binding_sha256: fixture.binding.clone(),
    };
    let publication = fixture
        .store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation,
                accepted_base_sha256: digest(b"explicit supplied-text genesis"),
                selected: Manifest {
                    format: 1,
                    transaction_id: operation,
                    scope: Scope::SelectedRecords,
                    records: vec![ObjectRef {
                        sha256: hash.clone(),
                        bytes: bytes.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(hash, bytes)]),
        )
        .unwrap();
    let writer = SelectedAdmission::new(fixture.store.clone(), scope).unwrap();
    let read = fixture
        .store
        .selected_snapshot(publication.token.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let projection = SelectedDomainProjection::from_snapshot(&read).unwrap();
    let selection = crate::writer::selected::Selection {
        accepted: publication.token,
        root: projection
            .current_reference(Namespace::Conversation, conversation)
            .unwrap(),
        refine: None,
    };
    let request = crate::writer::Request::Generate {
        conversation,
        operation: Uuid::new_v4(),
        request: "Keep x exact".into(),
        document: crate::retrieval::NormalizedDocument {
            document,
            format: crate::retrieval::SourceFormat::Notebook,
            source_sha256: digest(b"x = 7"),
            extractor_revision: "explicit test input".into(),
            units: vec![crate::retrieval::SourceUnit {
                page: Some(1),
                section: None,
                text: "x = 7".into(),
            }],
        },
        selection: crate::conversation::TextScope::SuppliedNotebook,
        guidance: None,
    };
    (writer, selection, request)
}
fn consumer_preferences(
    fixture: &Fixture,
    wire: &Wire,
) -> super::super::preference_consumer::RequestPreferences {
    super::super::preference_consumer::RequestPreferences::from_binding(
        fixture.store.clone(),
        fixture.config.clone(),
        Binding {
            group: fixture.group,
            root_id: "command-root".into(),
            digest: fixture.binding.clone(),
        },
        |store| Ok(wire.client(store.paths.credentials.join("drive.json"), fixture.group)),
    )
    .unwrap()
}

#[test]
fn healthy_guarded_writer_response_beyond_command_deadline_keeps_final_verification() {
    use crate::LLMEngine;
    use std::net::TcpListener;
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let guard = fixture.inspect(&wire, true);
    drop(fixture.command(&wire,Request::Edit(Uuid::new_v4(),guard,patch(json!({"personalization":{"enabled":true,"text":"SHARED MODEL PREFERENCE"}}))),true).unwrap());
    let (writer, selection, request) = consumer_writer(&fixture);
    let preferences = consumer_preferences(&fixture, &wire);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(stream) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => panic!("guarded model was not submitted within the bound"),
            }
        };
        let body = read_fixture_model_request(&mut stream, 131072);
        assert!(body.to_string().contains("SHARED MODEL PREFERENCE"));
        assert!(body.to_string().contains("x = 7"));
        // Deliberate bounded model latency, not a readiness assumption: this
        // crosses the old 64-callback quota and 60s snapshot TTL. Exact model
        // submission, generated receipt and unchanged current pins prove success.
        std::thread::sleep(Duration::from_secs(65));
        let response = br##"{"choices":[{"message":{"content":"# Shared draft\n\nx = 7"}}]}"##;
        let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len());
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(response);
    });
    struct Count {
        inner: crate::OpenAI,
        pulses: usize,
    }
    impl LLMEngine for Count {
        fn guarded_timeout(&self) -> Option<Duration> {
            self.inner.guarded_timeout()
        }
        fn add_text_content(&mut self, text: &str) {
            self.inner.add_text_content(text)
        }
        fn add_image_content(&mut self, _: &str) {
            panic!("text-only Writer")
        }
        fn clear_content(&mut self) {
            self.inner.clear_content()
        }
        fn execute(&mut self) -> Result<String> {
            panic!("guarded route required")
        }
        fn execute_with_handoff(
            &mut self,
            progress: &mut dyn FnMut() -> Result<()>,
            handoff: &mut crate::llm::ModelHandoff<'_>,
        ) -> Result<String> {
            let pulses = &mut self.pulses;
            self.inner.execute_with_handoff(
                &mut || {
                    *pulses += 1;
                    progress()
                },
                handoff,
            )
        }
    }
    let mut model = Count {
        inner: crate::OpenAI::new(
            "test-model".into(),
            "synthetic-loopback-key".into(),
            Some(format!("http://{address}")),
        )
        .with_timeout(Duration::from_secs(75))
        .with_single_request(),
        pulses: 0,
    };
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    server.join().unwrap();
    assert_eq!(result.report.status, crate::writer::Status::Generated);
    assert!(model.pulses > 64);
    assert!(matches!(
        result.publication,
        Some(crate::conversation::SelectedWriterPublication::Published { .. })
    ));
}

struct ConsumerModel {
    calls: usize,
    parts: Vec<String>,
    sent: Vec<String>,
    before: Option<Box<dyn FnOnce()>>,
    after: Option<Box<dyn FnOnce()>>,
    duplicate: bool,
    skip_handoff: bool,
}
impl crate::LLMEngine for ConsumerModel {
    fn guarded_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(text.into());
    }
    fn add_image_content(&mut self, _: &str) {
        panic!("text-only source")
    }
    fn clear_content(&mut self) {
        self.parts.clear()
    }
    fn execute(&mut self) -> Result<String> {
        panic!("no unguarded model fallback")
    }
    fn execute_with_handoff(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut crate::llm::ModelHandoff<'_>,
    ) -> Result<String> {
        progress()?;
        if let Some(before) = self.before.take() {
            before();
        }
        if self.skip_handoff {
            return Ok("# Unsubmitted draft\n\nx = 7".into());
        }
        handoff(&mut || {
            self.calls += 1;
            self.sent = self.parts.clone();
            Ok(())
        })?;
        if self.duplicate {
            handoff(&mut || {
                self.calls += 1;
                Ok(())
            })?;
        }
        if let Some(after) = self.after.take() {
            after();
        }
        progress()?;
        Ok("# Draft\n\nx = 7".into())
    }
}
fn test_model(after: Option<Box<dyn FnOnce()>>) -> ConsumerModel {
    ConsumerModel {
        calls: 0,
        parts: vec![],
        sent: vec![],
        before: None,
        after,
        duplicate: false,
        skip_handoff: false,
    }
}
fn set_preferences(fixture: &Fixture, wire: &Wire, settings: serde_json::Value) {
    let guard = fixture.inspect(wire, true);
    drop(
        fixture
            .command(
                wire,
                Request::Edit(Uuid::new_v4(), guard, patch(settings)),
                true,
            )
            .unwrap(),
    );
}

#[test]
fn consumer_disabled_shared_writer_and_foreign_snapshot_refuse_before_new_intent_or_model() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    set_preferences(
        &fixture,
        &wire,
        json!({"product_settings":{"writer_enabled":false}}),
    );
    let (writer, selection, request) = consumer_writer(&fixture);
    let before = selection.accepted.clone();
    let preferences = consumer_preferences(&fixture, &wire);
    assert!(!preferences.config().admits_writer(false));
    let mut model = test_model(None);
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(result.report.status, crate::writer::Status::Refused);
    assert_eq!(model.calls, 0);
    assert_eq!(
        fixture
            .store
            .selected_snapshot(writer.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        before
    );
    let other_wire = Wire::new();
    let other = Fixture::new(&other_wire);
    other.bootstrap(&other_wire);
    set_preferences(
        &other,
        &other_wire,
        json!({"personalization":{"enabled":true}}),
    );
    let preferences = consumer_preferences(&other, &other_wire);
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(result.report.status, crate::writer::Status::Refused);
    assert_eq!(model.calls, 0);
}

#[test]
fn consumer_changed_preferences_after_owned_submission_refuse_the_old_result_without_replay() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    set_preferences(
        &fixture,
        &wire,
        json!({"personalization":{"enabled":true,"text":"OLD SHARED PREFERENCE"}}),
    );
    let (writer, selection, request) = consumer_writer(&fixture);
    let preferences = consumer_preferences(&fixture, &wire);
    let key_scope = SelectionScope {
        group: fixture.group,
        key_sha256: record::key_sha256(),
        binding_sha256: fixture.binding.clone(),
    };
    let current = fixture
        .store
        .selected_snapshot(&key_scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    let old = preferences::project(
        &fixture.store,
        &key_scope,
        &current.transaction.selected,
        &mut budget(),
    )
    .unwrap();
    let config_hash = digest(&files::read(&fixture.config, MAX_RECORD as u64).unwrap());
    let guard = Condition {
        group: fixture.group,
        mode_epoch: fixture.store.sync_mode_snapshot().unwrap().epoch.unwrap(),
        binding_sha256: fixture.binding.clone(),
        store_generation: current.token.store_generation(),
        local_config_sha256: config_hash,
        selection: super::selection(&current, &old),
    };
    let operation = Uuid::new_v4();
    let envelope = record::prepare(
        fixture.store.actor_id,
        operation,
        &guard,
        Some(&old),
        &patch(json!({"personalization":{"text":"NEW SHARED PREFERENCE"}})),
    )
    .unwrap()
    .unwrap();
    let store = fixture.store.clone();
    let mut model = test_model(Some(Box::new(move || {
        store
            .commit_selected(&current.token, operation, vec![envelope], BTreeMap::new())
            .unwrap();
    })));
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(model.calls, 1);
    assert_eq!(result.report.status, crate::writer::Status::Stale);
    assert!(result.publication.is_none());
    assert!(model
        .sent
        .iter()
        .any(|p| p.contains("OLD SHARED PREFERENCE")));
    assert!(!model
        .sent
        .iter()
        .any(|p| p.contains("NEW SHARED PREFERENCE")));
}

#[test]
fn consumer_lightweight_progress_never_authorizes_a_result_after_original_object_corruption() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    set_preferences(&fixture, &wire, json!({"research_policy":null}));
    let (writer, selection, request) = consumer_writer(&fixture);
    let preferences = consumer_preferences(&fixture, &wire);
    let current = fixture
        .store
        .selected_snapshot(
            &SelectionScope {
                group: fixture.group,
                key_sha256: record::key_sha256(),
                binding_sha256: fixture.binding.clone(),
            },
            MAX_ITEMS,
        )
        .unwrap()
        .unwrap();
    let reference = &current.transaction.selected.records[0];
    let object = fixture
        .store
        .paths
        .data
        .join("generations")
        .join(current.token.store_generation().to_string())
        .join("objects")
        .join(&reference.sha256);
    let mut model = test_model(Some(Box::new(move || {
        std::fs::write(object, b"corrupt original after admitted handoff").unwrap();
    })));
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(model.calls, 1);
    assert_eq!(result.report.status, crate::writer::Status::Stale);
    assert!(result.publication.is_none());
}

#[test]
fn consumer_one_request_snapshot_cannot_authorize_two_owned_model_submissions() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    set_preferences(&fixture, &wire, json!({"research_policy":null}));
    let (writer, selection, request) = consumer_writer(&fixture);
    let preferences = consumer_preferences(&fixture, &wire);
    let mut model = test_model(None);
    model.duplicate = true;
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(model.calls, 1);
    assert_ne!(result.report.status, crate::writer::Status::Generated);
}

#[test]
fn consumer_generated_text_without_owned_handoff_has_no_publication() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    set_preferences(&fixture, &wire, json!({"research_policy":null}));
    let (writer, selection, request) = consumer_writer(&fixture);
    let preferences = consumer_preferences(&fixture, &wire);
    let mut model = test_model(None);
    model.skip_handoff = true;
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(model.calls, 0);
    assert_eq!(result.report.status, crate::writer::Status::Stale);
    assert!(result.publication.is_none());
}

#[test]
fn consumer_raw_config_change_before_handoff_refuses_without_model_submission() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    set_preferences(&fixture, &wire, json!({"research_policy":null}));
    let (writer, selection, request) = consumer_writer(&fixture);
    let preferences = consumer_preferences(&fixture, &wire);
    let path = fixture.config.clone();
    let mut model = test_model(None);
    model.before = Some(Box::new(move || {
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(b' ');
        std::fs::write(path, bytes).unwrap();
    }));
    let result = crate::writer::selected::run_with_preferences(
        &writer,
        &selection,
        &mut model,
        &request,
        preferences,
        None,
        &mut || false,
    );
    assert_eq!(model.calls, 0);
    assert_eq!(result.report.status, crate::writer::Status::Stale);
    assert!(result.publication.is_none());
}

#[test]
fn consumer_optional_memory_joins_at_owned_submission_and_result_without_nested_gates() {
    use crate::memory::selected::{InitializeAggregate, InitializeSelection, SelectedMemory};
    use crate::memory::{self, Provider};
    for change in 0..3 {
        let wire = Wire::new();
        let fixture = Fixture::new(&wire);
        fixture.bootstrap(&wire);
        set_preferences(
            &fixture,
            &wire,
            json!({"personalization":{"enabled":true,"text":"PREFERENCE HINT"}}),
        );
        let current = fixture.store.sync_mode_snapshot().unwrap();
        let cp = checkpoint(
            &fixture.store,
            &Binding {
                group: fixture.group,
                root_id: "command-root".into(),
                digest: fixture.binding.clone(),
            },
            current.epoch.unwrap(),
            &mut budget(),
        )
        .unwrap()
        .unwrap();
        let root: Root = serde_json::from_slice(&cp.root.as_ref().unwrap().bytes).unwrap();
        let identity = memory::catalog::Identity {
            group: fixture.group,
            profile: root.profile,
            binding_sha256: fixture.binding.clone(),
        };
        let memory =
            Arc::new(SelectedMemory::new(fixture.store.clone(), identity.clone()).unwrap());
        let mut fact = memory::Fact {
            key: memory::Key {
                profile: identity.profile,
                scope: memory::Scope::Global,
                fact: Uuid::new_v4(),
            },
            claim: "keep x exact".into(),
            value: "MEMORY HINT EXACT".into(),
            subject: "exact source arithmetic".into(),
            tags: vec!["exact".into()],
            provenance: memory::Provenance {
                basis: memory::Basis::UserConfirmed,
                description: "synthetic consumer fixture".into(),
                conversation: None,
                turn: None,
                source: None,
            },
            confidence_basis_points: 9000,
            observed_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            expires_ms: None,
        };
        let owner = memory::catalog::Owner::GlobalFact {
            key: fact.key.clone(),
        };
        let fact_scope = identity.aggregate_scope(&owner).unwrap();
        let initial = memory
            .initialize_aggregate(InitializeAggregate {
                operation: Uuid::new_v4(),
                accepted_base_sha256: genesis_base(
                    &cp.root.as_ref().unwrap().sha256,
                    &fact_scope.key_sha256,
                ),
                fact: fact.clone(),
            })
            .unwrap();
        memory
            .initialize_selection(InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: cp.root.unwrap().sha256,
                registrations: vec![initial.registration.unwrap()],
            })
            .unwrap();
        let mut config: serde_json::Value =
            serde_json::from_slice(&files::read(&fixture.config, MAX_RECORD as u64).unwrap())
                .unwrap();
        config["memory"] = json!({"enabled":true,"profile":identity.profile,"selection":identity});
        files::atomic_json(&fixture.config, &config).unwrap();
        let (writer, selection, request) = consumer_writer(&fixture);
        let preferences = consumer_preferences(&fixture, &wire);
        let mut model = test_model(None);
        if change != 0 {
            let expected = memory
                .inspect(&fact.key)
                .unwrap()
                .into_iter()
                .map(|r| r.revision_id)
                .collect();
            fact.value = "NEW MEMORY HINT".into();
            let memory = memory.clone();
            let mutation = memory::Mutation {
                operation: Uuid::new_v4(),
                expected,
                fact,
            };
            let mutate = Box::new(move || {
                memory.put(mutation).unwrap();
            });
            if change == 1 {
                model.before = Some(mutate);
            } else {
                model.after = Some(mutate);
            }
        }
        let result = crate::writer::selected::run_with_preferences(
            &writer,
            &selection,
            &mut model,
            &request,
            preferences,
            Some(memory.as_ref()),
            &mut || false,
        );
        if change == 0 {
            assert_eq!(result.report.status, crate::writer::Status::Generated);
            assert_eq!(model.calls, 1);
            assert!(model.sent.iter().any(|p| p.contains("MEMORY HINT EXACT")));
        } else {
            assert_eq!(result.report.status, crate::writer::Status::Stale);
            assert_eq!(model.calls, usize::from(change == 2));
            assert!(result.publication.is_none());
        }
    }
}
