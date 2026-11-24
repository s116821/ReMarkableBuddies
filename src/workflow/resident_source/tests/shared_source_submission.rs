use super::*;
use crate::conversation::{
    BindingAcquisition, BindingReceipt, ExpectedHeads, Mode, Role, SourceObservation, Turn,
};
use crate::workflow::{conversation_dispatch::Dispatch, selected_backend::RefusingBackend};
use base64::{engine::general_purpose::STANDARD, Engine as _};

const NOTE: &str = "Make a note from this page.";
const SAVE: &str = "Save a note from this page.";
const AMBIGUOUS_SAVE: &str = "Can you save it?";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Save {
    Explicit,
    Ambiguous,
}
const CIRCLE_NOTE: &str = "Make a note about the circled part.";
const REGION_TEXT: &str = "Selected equation: 24 / 6 = 4.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Circled {
    Selected,
    MissingOutline,
}
const QUESTION: &str = "What is this?";
const PAGE_TEXT: &str = "24 / 6 = 4. This is the rest of the page.";
const DRAFT: &str = "# Page note\n\nThe page records an equation and its result.";
const UNPUBLISHED: &str = "A short generated draft with no completed placement.";

struct SubmissionModel {
    writer: bool,
    circled: Option<Circled>,
    save: Option<Save>,
    last_visible_text: Option<String>,
    steps: Rc<Cell<usize>>,
    source: Rc<RefCell<Option<String>>>,
    images: Vec<String>,
    text: Vec<String>,
}

impl crate::LLMEngine for SubmissionModel {
    fn guarded_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(2))
    }

    fn execute_with_handoff(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut crate::llm::ModelHandoff<'_>,
    ) -> Result<String> {
        progress()?;
        let mut result = None;
        handoff(&mut || {
            result = Some(self.execute());
            Ok(())
        })?;
        result.context("Shared submission never reached the model")?
    }

    fn add_text_content(&mut self, text: &str) {
        self.text.push(text.to_owned());
    }

    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.to_owned());
    }

    fn clear_content(&mut self) {
        self.images.clear();
        self.text.clear();
    }

    fn execute(&mut self) -> Result<String> {
        let step = self.steps.get();
        self.steps.set(step + 1);
        match (step, self.writer) {
            (0, writer) => {
                let [image] = self.images.as_slice() else {
                    panic!("expected exactly one original page at intent intake");
                };
                assert!(self.text[0].contains("circled or highlighted"));
                assert!(STANDARD.decode(image)?.starts_with(b"\x89PNG"));
                *self.source.borrow_mut() = Some(image.clone());
                if let Some(latest) = &self.last_visible_text {
                    assert_eq!(self.text.len(), 2);
                    let history: Vec<(u64, Role, Mode, String)> =
                        serde_json::from_str(&self.text[1])?;
                    assert_eq!(history.len(), 6);
                    assert_eq!(history[0].3, "Prior question 2");
                    assert_eq!(&history[5].3, latest);
                    assert!(!history.iter().any(|turn| turn.3 == UNPUBLISHED));
                    return Ok(r#"{"intent":"unresolved"}"#.into());
                }
                assert_eq!(self.text.len(), 1, "fresh notebook has no history part");
                if let Some(circled) = self.circled {
                    let target = match circled {
                        Circled::Selected => serde_json::json!({
                            "subject": "selected equation",
                            "geometry": {"components": [[[100000,100000],[500000,100000],[500000,900000],[100000,900000]]]},
                            "understanding": {"status": "interpreted"}
                        }),
                        Circled::MissingOutline => serde_json::json!({
                            "subject": "selected equation", "geometry": null,
                            "understanding": {"status": "uncertain", "reason": "No closed outline"}
                        }),
                    };
                    return Ok(serde_json::json!({
                        "intent": "writer-circled", "request": CIRCLE_NOTE, "target": target,
                    })
                    .to_string());
                }
                if let Some(save) = self.save {
                    let intent = if save == Save::Explicit {
                        serde_json::json!({"intent":"writer-export-current-page","request":SAVE,"separate_note":false})
                    } else {
                        serde_json::json!({"intent":"clarify-export","request":AMBIGUOUS_SAVE})
                    };
                    return Ok(intent.to_string());
                }
                if writer {
                    Ok(
                        serde_json::json!({"intent":"writer-current-page","request":NOTE})
                            .to_string(),
                    )
                } else {
                    Ok(r#"{"intent":"reader"}"#.into())
                }
            }
            (1, true) if self.circled == Some(Circled::Selected) => {
                let original = image::load_from_memory(
                    &STANDARD.decode(self.source.borrow().as_deref().unwrap())?,
                )?
                .to_rgb8();
                let [masked] = self.images.as_slice() else {
                    panic!("expected only the masked source")
                };
                let masked = image::load_from_memory(&STANDARD.decode(masked)?)?.to_rgb8();
                assert_eq!(masked.dimensions(), original.dimensions());
                assert_eq!(masked.get_pixel(32, 32), original.get_pixel(32, 32));
                assert_ne!(original.get_pixel(150, 30).0, [255, 255, 255]);
                assert_eq!(masked.get_pixel(150, 30).0, [255, 255, 255]);
                Ok(serde_json::json!({"pages":[{"page":1,"status":"transcribed","text":REGION_TEXT}]}).to_string())
            }
            (1, true) => {
                assert_eq!(
                    self.images.as_slice(),
                    &[self.source.borrow().clone().unwrap()]
                );
                Ok(serde_json::json!({"pages":[{"page":1,"status":"transcribed","text":PAGE_TEXT}]}).to_string())
            }
            (2, true) => {
                assert!(self.images.is_empty());
                let input = serde_json::from_str::<serde_json::Value>(self.text.last().unwrap())?;
                if self.circled == Some(Circled::Selected) {
                    assert_eq!(input["request"], CIRCLE_NOTE);
                    assert_eq!(input["history"], serde_json::json!([]));
                    assert_eq!(input["source_units"][0]["text"], REGION_TEXT);
                    assert_eq!(
                        input["source_provenance"]["region"]["about"],
                        "About: selected equation"
                    );
                    Ok("# Circled note\n\nThe selected equation gives four.".into())
                } else {
                    assert_eq!(
                        input["request"],
                        if self.save == Some(Save::Explicit) {
                            SAVE
                        } else {
                            NOTE
                        }
                    );
                    assert_eq!(input["source_units"][0]["text"], PAGE_TEXT);
                    Ok(DRAFT.into())
                }
            }
            (1, false) => {
                assert!(!self.images.is_empty());
                Ok(format!("QUESTION: {QUESTION}\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: 24 / 6\n---\nANSWER: Four."))
            }
            (2, false) => Ok(format!("TRANSCRIPTION: {QUESTION}")),
            _ => panic!("unexpected extra submission or export"),
        }
    }
}

fn seed_history(
    ledger: &Ledger,
    runtime: &RuntimeIdentity,
    document: ConversationId,
    page: ConversationId,
) -> Result<String> {
    let conversation = ConversationId::new_v4();
    ledger.create(conversation, ConversationId::new_v4(), 1)?;
    let latest = "Prior context: two  spaces, exact words, and a paragraph.\n\n".repeat(50);
    assert!(latest.len() > 2048);
    let earlier = (0..7).map(|index| {
        (
            Role::User,
            Mode::Reader,
            Outcome::Interpreted,
            format!("Prior question {index}"),
        )
    });
    let recent = [
        (
            Role::User,
            Mode::Reader,
            Outcome::Interpreted,
            latest.clone(),
        ),
        (
            Role::Assistant,
            Mode::Writer,
            Outcome::Generated,
            UNPUBLISHED.to_owned(),
        ),
    ];
    for (role, mode, outcome, text) in earlier.chain(recent) {
        ledger.append(
            ConversationId::new_v4(),
            ledger.expected(Namespace::Conversation, conversation)?,
            Turn {
                id: ConversationId::new_v4(),
                conversation,
                exchange: ConversationId::new_v4(),
                sequence: 0,
                role,
                mode,
                outcome,
                text: Some(text),
                sources: vec![],
                correction_of: None,
                created_ms: 2,
                updated_ms: 2,
                completion: None,
            },
            vec![],
        )?;
    }
    let hex = |bytes: [u8; 32]| {
        bytes
            .into_iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let operation = ConversationId::new_v4();
    ledger.bind(
        operation,
        ledger.expected(Namespace::Conversation, conversation)?,
        ExpectedHeads::default(),
        BindingReceipt {
            operation,
            conversation,
            request_fingerprint: "shared-submission".into(),
            adapter_fingerprint: hex(runtime.adapter_sha256),
            source: SourceObservation {
                document,
                page,
                session: "shared-submission-session".into(),
                visit: "shared-submission-visit".into(),
                content_revision: None,
                order_revision: None,
                capability_revision: hex(runtime.profile_sha256),
                evidence_procedure: "shared-submission-fixture".into(),
            },
            intended_target: Some(page),
            observed_document: document,
            observed_page: page,
            expected_order: vec![page],
            observed_order: vec![page],
            persisted_revision: "shared-submission-binding".into(),
            observed_ms: 3,
            evidence_origin: "installed-native-guarded-v1".into(),
            acquisition: BindingAcquisition::ExistingCurrent,
        },
    )?;
    Ok(latest)
}

#[test]
fn shared_lower_right_submission_routes_reader_and_unqueued_writer() -> Result<()> {
    for (reader_enabled, writer_request, bound_history, circled, save) in [
        (true, false, false, None, None),
        (true, true, false, None, None),
        (false, true, false, None, None),
        (true, false, true, None, None),
        (true, true, false, Some(Circled::Selected), None),
        (false, true, false, Some(Circled::Selected), None),
        (true, false, false, Some(Circled::MissingOutline), None),
        (true, true, false, None, Some(Save::Explicit)),
        (false, true, false, None, Some(Save::Ambiguous)),
    ] {
        let mode = match (circled, writer_request) {
            (Some(Circled::Selected), _) => "reconnect-source-handoff-writer-insert",
            (_, true) => "reconnect-source-handoff-text-refusal",
            _ => "reconnect-source-handoff",
        };
        let base = std::env::temp_dir().join(format!("shared-source-{}", ConversationId::new_v4()));
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({
            "paths": StorePaths {
                data: base.join("data"),
                cache: base.join("cache"),
                credentials: base.join("credentials"),
            },
            "product_settings": {
                "reader_enabled": reader_enabled,
                "writer_enabled": true,
                "export_destination": "disabled"
            }
        }))?;
        let store = if circled.is_some() {
            crate::reader_runtime::ordinary_store(&config, None)
                .map_err(|report| report.into_ordinary_startup_error())?
        } else {
            Arc::new(Store::open(config.paths.clone())?)
        };
        let (mut workflow, events, _notify) =
            workflow_with_store(mode, vec![], Duration::ZERO, &store);
        let native = Fixture::new();
        let document = ConversationId::new_v4();
        let page = ConversationId::new_v4();
        native.declare_source(document, page);
        let position = native.successor_position();
        let runtime = native
            .start_with_store(store.clone(), mode, Duration::from_secs(5))
            .unwrap();
        let ledger = Ledger::new(store.clone());
        let last_visible_text = if bound_history {
            Some(seed_history(&ledger, runtime.identity(), document, page)?)
        } else {
            None
        };
        let owner = workflow.resident_source.as_mut().unwrap();
        owner.selection = InstalledReaderSelection::fixture_for_installed_output(
            &base.join("output-profile"),
            runtime.identity(),
        )?;
        owner.session = Some(Box::new(OwnedSession {
            runtime,
            _fixture: native,
            events: events.clone(),
            delivery_fault: false,
            observe_file_change: None,
            writer_slot: None,
            writer_slot_fault: false,
            refinement_slot: None,
        }));
        let conversation = Rc::new(Cell::new(None));
        let written = Rc::new(RefCell::new(Vec::new()));
        let output = RecognitionProbe {
            conversation: conversation.clone(),
            confirmed_source: writer_request,
            insertion_source: (circled == Some(Circled::Selected)).then_some(page),
            successor: (writer_request && circled.is_none()).then(|| SuccessorProbe {
                position: position.clone(),
                interrupt_restore: false,
            }),
            rendered_text: writer_request.then(|| written.clone()),
            ..RecognitionProbe::default()
        };
        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                .with_test_output(Box::new(output)),
        );
        let dispatch =
            Dispatch::local(&config, &ledger)?.with_regional_writer_model("fixture".into());
        let steps = Rc::new(Cell::new(0));
        let source = Rc::new(RefCell::new(None));
        let model = SubmissionModel {
            writer: writer_request,
            circled,
            save,
            last_visible_text,
            steps: steps.clone(),
            source: source.clone(),
            images: vec![],
            text: vec![],
        };

        assert!(workflow.wait_for_idle_batch(None, Some(&config))?);
        let mut service = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        service.run_iteration_with_trigger(false, Some(&dispatch), Some(&config))?;
        if bound_history || circled == Some(Circled::MissingOutline) {
            assert_eq!(steps.get(), 1);
            assert!(written.borrow().is_empty());
            assert!(conversation.get().is_none());
            drop((service, dispatch, ledger, store));
            std::fs::remove_dir_all(base)?;
            continue;
        }
        if save == Some(Save::Ambiguous) {
            assert_eq!(steps.get(), 1);
            assert!(position.exists());
            assert_eq!(
                written.borrow().as_slice(),
                &["Q: Can you save it?\n\nA: Should I save a note from this page to your configured destination?\n".to_owned()]
            );
            assert!(!ledger
                .inspect(
                    conversation
                        .get()
                        .context("Placement conversation absent")?,
                    false
                )?
                .iter()
                .any(|record| matches!(
                    record,
                    Record::Export(_)
                        | Record::Turn(Turn {
                            outcome: Outcome::Generated,
                            ..
                        })
                )));
            drop((service, dispatch, ledger, store));
            std::fs::remove_dir_all(base)?;
            continue;
        }
        assert_eq!(steps.get(), 3);
        assert!(!events.borrow().contains(&"current-target-writer-received"));
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == "insert")
                .count(),
            usize::from(circled == Some(Circled::Selected))
        );
        let records = ledger.inspect(
            conversation
                .get()
                .context("Placement conversation absent")?,
            false,
        )?;

        if writer_request {
            let (exact_request, saved_draft) = if circled == Some(Circled::Selected) {
                assert!(!position.exists(), "native insertion selected the new page");
                assert!(records.iter().any(|record| matches!(record, Record::RegionSource(region)
                    if region.normalized.document.document == document && region.about == "About: selected equation")));
                (CIRCLE_NOTE, "# Circled note\n\nAbout: selected equation\n\nThe selected equation gives four.")
            } else {
                assert!(position.exists(), "draft must go onto the adjacent page");
                (
                    if save == Some(Save::Explicit) {
                        SAVE
                    } else {
                        NOTE
                    },
                    DRAFT,
                )
            };
            let draft_body = saved_draft.strip_prefix("# ").unwrap();
            let response = if save == Some(Save::Explicit) {
                format!("Draft saved. Note export is disabled.\n\n{draft_body}")
            } else {
                draft_body.to_owned()
            };
            assert_eq!(
                written.borrow().as_slice(),
                &[format!("Q: {exact_request}\n\nA: {response}\n")]
            );
            assert_eq!(records.iter().filter(|record| matches!(record, Record::Turn(turn)
                if turn.role == Role::User && turn.mode == Mode::Writer && turn.text.as_deref() == Some(exact_request))).count(), 1);
            assert!(records.iter().any(|record| matches!(record, Record::Turn(turn)
                if turn.role == Role::Assistant && turn.mode == Mode::Writer
                    && turn.outcome == Outcome::Generated && turn.text.as_deref() == Some(saved_draft))));
            let evidence = records
                .iter()
                .find_map(|record| match record {
                    Record::ResidentCapture(capture) => Some(capture.id),
                    _ => None,
                })
                .context("Writer source evidence absent")?;
            let captured = ledger.stored_resident_images(evidence)?;
            assert_eq!(captured.evidence.before.document, document);
            assert_eq!(captured.evidence.before.page, page);
            assert_eq!(
                source.borrow().as_deref(),
                Some(STANDARD.encode(captured.original_png).as_str())
            );
        } else {
            assert!(!position.exists());
            assert!(written.borrow().is_empty());
            assert!(records.iter().any(|record| matches!(record, Record::Turn(turn)
                if turn.role == Role::Assistant && turn.mode == Mode::Reader && turn.text.as_deref() == Some("Four."))));
        }
        drop((service, dispatch, ledger, store));
        std::fs::remove_dir_all(base)?;
    }
    Ok(())
}
