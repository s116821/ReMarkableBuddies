use super::*;
use crate::conversation::{
    BindingAcquisition, BindingReceipt, ContextBudget, ExpectedHeads, Mode, Role,
    SourceObservation, Turn,
};
use crate::workflow::{conversation_dispatch::Dispatch, selected_backend::RefusingBackend};
use base64::{engine::general_purpose::STANDARD, Engine as _};

const NOTE: &str = "Make a note from this page.";
const SAVE: &str = "Save a note from this page.";
const AMBIGUOUS_SAVE: &str = "Can you save it?";
const UPDATED_SAVE: &str = "Save an updated note from this page.";
const SEPARATE_SAVE: &str = "Save a separate note from this page.";
const CLARIFICATION: &str = "Should I save a note from this page to your configured destination?";
const SAVED: &str = "Saved to your configured note destination.";
const UNCERTAIN: &str =
    "The destination may have received the note. Check the destination before retrying.";
const SAVE_NOTEBOOK: &str = "Save a note from this entire notebook.";
const SEPARATE_NOTEBOOK: &str = "Save a separate note from this entire notebook.";

fn complete_notebook_draft(revision: &str) -> String {
    format!("# Notebook note\n\nThe first and last pages each give an example. {revision}")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Save {
    Explicit,
    Update,
    Separate,
    Compact,
    CompactUncertain,
    Ambiguous,
}
impl Save {
    fn compact(self) -> bool {
        matches!(self, Self::Compact | Self::CompactUncertain)
    }

    fn wording(self) -> &'static str {
        match self {
            Self::Explicit | Self::Compact | Self::CompactUncertain => SAVE,
            Self::Update => UPDATED_SAVE,
            Self::Separate => SEPARATE_SAVE,
            Self::Ambiguous => AMBIGUOUS_SAVE,
        }
    }
}
const CIRCLE_NOTE: &str = "Make a note about the circled part.";
const REGION_TEXT: &str = "Selected equation: 24 / 6 = 4.";
const INITIAL_CIRCLED_SAVE: usize = 3;
const EDITED_CIRCLED_SAVE: usize = 4;
const EDITED_REGION_TEXT: &str = "Corrected equation: 30 / 6 = 5.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Circled {
    Selected,
    MissingOutline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegionAction {
    Save,
    Update,
    Separate,
    Review,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegionShape {
    Left,
    LeftSubpixel,
    Right,
    Overlap,
    Missing,
    Competing,
}
impl RegionShape {
    fn geometry(self) -> serde_json::Value {
        let polygon = |left, right| {
            serde_json::json!([
                [left, 100000],
                [right, 100000],
                [right, 900000],
                [left, 900000]
            ])
        };
        let polygons = match self {
            Self::Left => vec![polygon(100000, 500000)],
            Self::LeftSubpixel => vec![polygon(100001, 500000)],
            Self::Right => vec![polygon(650000, 950000)],
            Self::Overlap => vec![polygon(400000, 550000)],
            Self::Missing => return serde_json::Value::Null,
            Self::Competing => vec![polygon(100000, 500000), polygon(650000, 950000)],
        };
        serde_json::json!({"components":polygons})
    }
}

impl RegionAction {
    fn request(self) -> &'static str {
        match self {
            Self::Save => "Save a note about only the circled part.",
            Self::Update => "Save an updated note about the circled part.",
            Self::Separate => "Save a separate note about the circled part.",
            Self::Review => "Make a note about the circled part for me to review before saving.",
        }
    }

    fn subject(self) -> &'static str {
        match self {
            Self::Update => "equation in the selection",
            _ => "selected equation",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Submission {
    Reader,
    CurrentPage,
    BoundHistory,
    Circled(Circled),
    WholeNotebook,
    SaveNotebook(Save),
    SaveRegion(RegionAction, RegionShape, usize),
    Save(Save),
}

impl Submission {
    fn expects_writer_output(self) -> bool {
        matches!(
            self,
            Self::CurrentPage | Self::Circled(Circled::Selected) | Self::Save(_)
        )
    }

    fn save(self) -> Option<Save> {
        match self {
            Self::Save(save) => Some(save),
            _ => None,
        }
    }

    fn circled(self) -> Option<Circled> {
        match self {
            Self::Circled(region) => Some(region),
            _ => None,
        }
    }
}
const QUESTION: &str = "What is this?";
const PAGE_TEXT: &str = "24 / 6 = 4. This is the rest of the page.";
const DRAFT: &str = "# Page note\n\nThe page records an equation and its result.";
const UNPUBLISHED: &str = "A short generated draft with no completed placement.";

fn draft_at_export_display_boundary() -> String {
    use crate::{
        conversation_layout::CONVERSATION_TITLE, workflow::resident_output::format_exchange,
    };

    let prefix = format_exchange(SAVE, format_args!("{SAVED}\n\nPage note\n\n"));
    let body = "x".repeat(513 - (CONVERSATION_TITLE.len() + 2) - prefix.len());
    format!("# Page note\n\n{body}")
}

struct SubmissionModel {
    submission: Submission,
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
        if let Submission::SaveRegion(action, shape, revision) = self.submission {
            let edited = revision == EDITED_CIRCLED_SAVE;
            let transcription = if edited {
                EDITED_REGION_TEXT
            } else {
                REGION_TEXT
            };
            return match step {
                0 => {
                    assert_eq!(self.images.len(), 1);
                    assert!(self.text[0].contains("writer-export-circled"));
                    assert!(self.text[0].contains("before it is saved"));
                    *self.source.borrow_mut() = Some(self.images[0].clone());
                    let target = serde_json::json!({
                        "subject": action.subject(),
                        "geometry": shape.geometry(),
                        "understanding": {"status":"interpreted"},
                    });
                    let response = if action == RegionAction::Review {
                        serde_json::json!({
                            "intent":"writer-circled", "request":action.request(), "target":target
                        })
                    } else {
                        serde_json::json!({
                            "intent":"writer-export-circled", "request":action.request(), "target":target,
                            "separate_note":action == RegionAction::Separate
                        })
                    };
                    Ok(response.to_string())
                }
                1 => {
                    let original = image::load_from_memory(
                        &STANDARD.decode(self.source.borrow().as_deref().unwrap())?,
                    )?
                    .to_rgb8();
                    let [masked] = self.images.as_slice() else {
                        panic!("expected only the masked region")
                    };
                    let masked = image::load_from_memory(&STANDARD.decode(masked)?)?.to_rgb8();
                    let (inside, outside) = match shape {
                        RegionShape::Left | RegionShape::LeftSubpixel => (32, 150),
                        RegionShape::Right => (150, 32),
                        RegionShape::Overlap => (87, 150),
                        _ => panic!("ambiguous export reached recognition"),
                    };
                    assert_eq!(masked.get_pixel(inside, 32), original.get_pixel(inside, 32));
                    assert_eq!(masked.get_pixel(outside, 32).0, [255, 255, 255]);
                    assert_eq!(original.get_pixel(32, 32).0 == [255, 0, 240], edited);
                    Ok(serde_json::json!({"pages":[{"page":1,"status":"transcribed","text":transcription}]}).to_string())
                }
                2 => {
                    assert!(self.images.is_empty());
                    let input: serde_json::Value = serde_json::from_str(self.text.last().unwrap())?;
                    assert_eq!(input["request"], action.request());
                    assert_eq!(input["source_units"][0]["text"], transcription);
                    assert_eq!(
                        input["source_provenance"]["region"]["about"],
                        format!("About: {}", action.subject())
                    );
                    let body = if edited || revision == INITIAL_CIRCLED_SAVE {
                        transcription.to_owned()
                    } else {
                        format!("Regional draft revision {revision}.")
                    };
                    Ok(format!("# Circled note\n\n{body}"))
                }
                _ => panic!("unexpected circled Writer stage"),
            };
        }
        if let Submission::SaveNotebook(save) = self.submission {
            let request = if save == Save::Separate {
                SEPARATE_NOTEBOOK
            } else {
                SAVE_NOTEBOOK
            };
            return match step {
                0 => {
                    assert_eq!(self.images.len(), 1);
                    assert!(self.text[0].contains("writer-export-whole-notebook"));
                    Ok(serde_json::json!({
                        "intent": "writer-export-whole-notebook",
                        "request": request,
                        "separate_note": save == Save::Separate,
                    })
                    .to_string())
                }
                1 => {
                    assert_eq!(self.images.len(), 2);
                    assert!(self
                        .text
                        .iter()
                        .any(|text| text.contains("writer-notebook-page-evidence-v1")));
                    Ok(serde_json::json!({
                        "pages": [
                            {"page":1,"status":"readable","writing":"First page.","visual_observation":""},
                            {"page":2,"status":"readable","writing":"Last page.","visual_observation":""}
                        ]
                    })
                    .to_string())
                }
                2 => {
                    assert!(self.images.is_empty());
                    let prompt = self.text.join("\n");
                    assert!(prompt.contains("First page."));
                    assert!(prompt.contains("Last page."));
                    Ok("The notebook records examples on both original pages.".into())
                }
                3 => {
                    assert!(self.images.is_empty());
                    Ok(complete_notebook_draft(
                        self.last_visible_text.as_deref().unwrap(),
                    ))
                }
                _ => panic!("unexpected whole-notebook Writer stage"),
            };
        }
        match (step, self.submission.expects_writer_output()) {
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
                if self.submission == Submission::WholeNotebook {
                    return Ok(serde_json::json!({
                        "intent": "writer-whole-notebook",
                        "request": "Make a note from my whole notebook.",
                    })
                    .to_string());
                }
                if let Some(circled) = self.submission.circled() {
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
                if let Some(save) = self.submission.save() {
                    let intent = if save == Save::Ambiguous {
                        serde_json::json!({"intent":"clarify-export","request":save.wording()})
                    } else {
                        serde_json::json!({"intent":"writer-export-current-page","request":save.wording(),"separate_note":save == Save::Separate})
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
            (1, true) if self.submission == Submission::Circled(Circled::Selected) => {
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
                if self.submission == Submission::Circled(Circled::Selected) {
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
                        self.submission.save().map_or(NOTE, Save::wording)
                    );
                    assert_eq!(input["source_units"][0]["text"], PAGE_TEXT);
                    match self.submission.save() {
                        Some(Save::Update) => Ok("# Page note\n\nThe page computes four from twenty-four divided by six.".into()),
                        Some(save) if save.compact() => Ok(draft_at_export_display_boundary()),
                        _ => Ok(DRAFT.into()),
                    }
                }
            }
            (1, false) => {
                assert!(!self.images.is_empty());
                let intent = serde_json::json!({"version":1,"intent":"no-restriction"});
                Ok(format!("QUESTION: {QUESTION}\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: 24 / 6\nREQUEST_WEB_JSON: {intent}\n---\nANSWER: Deferred"))
            }
            (2, false) => Ok(serde_json::json!({
                "version": 1,
                "transcription": QUESTION,
                "request_web": {"version":1,"intent":"no-restriction"}
            })
            .to_string()),
            (3, false) => Ok(serde_json::json!({
                "status":"answered","text":"Four.","citations":[]
            })
            .to_string()),
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
fn installed_circled_save_keeps_an_exact_region_association() -> Result<()> {
    use RegionAction::{Review, Save, Separate, Update};
    use RegionShape::{Competing, Left, Missing, Overlap, Right};

    let base = std::env::temp_dir().join(format!("region-save-{}", ConversationId::new_v4()));
    let config: crate::config::Config = serde_json::from_value(serde_json::json!({
        "paths": StorePaths {
            data: base.join("data"),
            cache: base.join("cache"),
            credentials: base.join("credentials"),
        },
        "product_settings": {
            "reader_enabled": true,
            "writer_enabled": true,
            "export_destination": "mem",
        }
    }))?;
    let store = crate::reader_runtime::ordinary_store(&config, None)
        .map_err(|report| report.into_ordinary_startup_error())?;
    let ledger = Ledger::new(store.clone());
    let mut dispatch =
        Dispatch::local(&config, &ledger)?.with_regional_writer_model("fixture".into());
    let destination = Arc::new(crate::writer_flow::tests::Destination::default());
    dispatch.test_note_destination = Some(destination.clone());
    let document = ConversationId::new_v4();
    let page = ConversationId::new_v4();
    let mut primary = None;
    let mut initial_proof = None;

    let attempts = [
        (Save, Missing, 0, 0),
        (Save, Competing, 0, 0),
        (Review, Left, 0, 0),
        (Save, Left, 1, 1),
        (Update, RegionShape::LeftSubpixel, 2, 1),
        (Save, Right, 3, 2),
        (Save, Overlap, 3, 2),
        (Separate, Left, 4, 3),
        (Update, Left, 5, 3),
        (Save, Left, 5, 3),
        (Update, Right, 6, 3),
    ];
    for (index, (action, shape, writes, notes)) in attempts.into_iter().enumerate() {
        let displayed = index + 1 == attempts.len();
        let mode = if displayed {
            "reconnect-source-handoff-writer-insert"
        } else {
            "reconnect-source-handoff-text-refusal"
        };
        let (mut workflow, events, _notify) =
            workflow_with_store(mode, vec![], Duration::ZERO, &store);
        let native = Fixture::new();
        native.declare_source(document, page);
        if index == EDITED_CIRCLED_SAVE {
            native.declare_edited_selection();
        }
        let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(10))?;
        let owner = workflow.resident_source.as_mut().unwrap();
        owner.selection = InstalledReaderSelection::fixture_for_installed_output(
            &base.join(format!("profile-{index}")),
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
        let written = Rc::new(RefCell::new(Vec::new()));
        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                .with_test_output(Box::new(RecognitionProbe {
                    confirmed_source: displayed,
                    refuse_writer_text: !displayed,
                    insertion_source: displayed.then_some(page),
                    rendered_text: Some(written.clone()),
                    ..RecognitionProbe::default()
                })),
        );
        let steps = Rc::new(Cell::new(0));
        let model = SubmissionModel {
            submission: Submission::SaveRegion(action, shape, index),
            last_visible_text: None,
            steps: steps.clone(),
            source: Rc::new(RefCell::new(None)),
            images: vec![],
            text: vec![],
        };
        if index == 8 {
            destination.lose_next_ack();
        }
        let before = destination.counts();
        assert!(workflow.wait_for_idle_batch(None, Some(&config))?);
        let mut service = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        let result = service.run_iteration_with_trigger(false, Some(&dispatch), Some(&config));
        if matches!(shape, Missing | Competing) {
            result?;
            assert_eq!(steps.get(), 1);
            assert_eq!(destination.counts(), before);
        } else if displayed {
            result?;
            assert_eq!(
                written.borrow().as_slice(),
                &[crate::workflow::resident_output::format_exchange(
                    action.request(),
                    format!(
                        "{SAVED}\n\nCircled note\n\nAbout: {}\n\nRegional draft revision {index}.",
                        action.subject()
                    ),
                )]
            );
        } else {
            let refusal = result.unwrap_err();
            assert!(
                refusal.to_string().contains("Writer draft cannot be typed"),
                "{refusal:#}"
            );
            assert_eq!(steps.get(), 3);
            assert!(written.borrow().is_empty());
        }
        assert_eq!(events.borrow().contains(&"insert"), displayed);
        assert_eq!(destination.counts().2, writes);
        assert_eq!(destination.remote_note_count(), notes);
        if matches!(index, 6 | 9) {
            assert_eq!(destination.counts(), before);
        }
        let exports =
            store.snapshot_heads_matching(&[Namespace::ExportAssociation], 8, |_| true)?;
        assert_eq!(exports.len(), notes);
        for export in exports {
            let Record::Export(export) = Ledger::decode(&export)? else {
                anyhow::bail!("expected circled association");
            };
            assert!(export.source_scope.starts_with(&format!(
                "normalized-document:{document}:installed-region:{page}:",
            )));
            if index == INITIAL_CIRCLED_SAVE {
                primary = Some((export.id, export.native_note.clone()));
            }
            if primary.as_ref().is_some_and(|(id, _)| *id == export.id) {
                assert_eq!(primary.as_ref().unwrap().1, export.native_note);
                if matches!(index, INITIAL_CIRCLED_SAVE | EDITED_CIRCLED_SAVE) {
                    let content = destination
                        .remote_note_content(export.native_note.as_deref().context("no remote ID")?)
                        .context("remote note absent")?;
                    if index == INITIAL_CIRCLED_SAVE {
                        assert!(content.contains(REGION_TEXT), "{content}");
                    } else {
                        assert!(content.contains(EDITED_REGION_TEXT), "{content}");
                        assert!(!content.contains(REGION_TEXT), "{content}");
                    }
                    let records = ledger.inspect(export.conversation, false)?;
                    let latest = records
                        .iter()
                        .filter_map(|record| match record {
                            Record::Turn(turn)
                                if turn.role == Role::Assistant
                                    && turn.outcome == Outcome::Generated =>
                            {
                                Some(turn)
                            }
                            _ => None,
                        })
                        .max_by_key(|turn| turn.sequence)
                        .context("no generated regional draft")?;
                    let source = store
                        .value(Namespace::Source, latest.sources[0])?
                        .context("no retained region")?;
                    let Record::RegionSource(source) = Ledger::decode(&source)? else {
                        anyhow::bail!("unexpected retained source");
                    };
                    let proof = source
                        .installed_region
                        .clone()
                        .context("no installed proof")?;
                    if index == INITIAL_CIRCLED_SAVE {
                        assert!(serde_json::to_value(&proof)?
                            .get("association_mask_sha256")
                            .is_none());
                        initial_proof = Some((proof, source.original.sha256, source.target));
                    } else {
                        let (prior, original, target) = initial_proof.as_ref().unwrap();
                        assert!(proof.same_footprint(prior));
                        assert_eq!(proof.id, prior.id);
                        assert_ne!(proof.mask_sha256, prior.mask_sha256);
                        assert_ne!(&source.original.sha256, original);
                        assert_ne!(&source.target, target);
                    }
                }
                assert_eq!(
                    export.outcome,
                    if index >= 8 {
                        Outcome::ReconcileRequired
                    } else {
                        Outcome::Completed
                    }
                );
            }
        }
    }
    drop((dispatch, ledger, store));
    std::fs::remove_dir_all(base)?;
    Ok(())
}

#[test]
fn installed_notebook_save_reuses_one_external_note_across_original_pages() -> Result<()> {
    use std::{
        fs,
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };

    let base = std::env::temp_dir().join(format!("notebook-save-{}", ConversationId::new_v4()));
    let root = base.join("xochitl");
    let templates = base.join("templates");
    let document = ConversationId::new_v4();
    let pages = [ConversationId::new_v4(), ConversationId::new_v4()];
    let saved_pages = root.join(document.to_string());
    for path in [&base, &root, &templates, &saved_pages] {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let private_write = |path: PathBuf, bytes: &[u8]| -> Result<()> {
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)?
            .write_all(bytes)?;
        Ok(())
    };
    let rows: Vec<_> = pages
        .iter()
        .zip(["a", "b"])
        .map(|(page, idx)| {
            serde_json::json!({
                "id": page, "idx": {"value": idx}, "template": {"value": "Blank"},
            })
        })
        .collect();
    private_write(
        root.join(format!("{document}.content")),
        &serde_json::to_vec(&serde_json::json!({
            "fileType":"notebook", "formatVersion":2, "pageCount":2,
            "orientation":"portrait",
            "cPages":{"pages":rows,"lastOpened":{"value":pages[0]}},
        }))?,
    )?;
    private_write(
        templates.join("templates.json"),
        br#"{"templates":[{"filename":"Blank","name":"Blank"}]}"#,
    )?;
    private_write(
        templates.join("Blank.svg"),
        br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1404 1872"><rect width="100%" height="100%" fill="#fff"/></svg>"##,
    )?;
    let original_scene = include_bytes!("../../../../tests/fixtures/notebook-ink/pens-small.rm");
    let changed_scene = include_bytes!("../../../../tests/fixtures/native-history/applied.rm");
    for page in pages {
        private_write(saved_pages.join(format!("{page}.rm")), original_scene)?;
    }

    let config: crate::config::Config = serde_json::from_value(serde_json::json!({
        "paths": StorePaths {
            data: base.join("data"),
            cache: base.join("cache"),
            credentials: base.join("credentials"),
        },
        "product_settings": {
            "reader_enabled": true,
            "writer_enabled": true,
            "export_destination": "mem"
        }
    }))?;
    drop(Store::open(config.paths.clone())?);
    let store = Arc::new(Store::prepare_notebook(config.paths.clone())?);
    let ledger = Ledger::new(store.clone());
    let mut dispatch =
        Dispatch::local(&config, &ledger)?.with_regional_writer_model("fixture".into());
    let destination = Arc::new(crate::writer_flow::tests::Destination::default());
    dispatch.test_note_destination = Some(destination.clone());

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ResultPath {
        RefusedDisplay,
        LostAcknowledgment,
        ChangedOriginal,
        Displayed,
    }
    use ResultPath::*;

    for (index, (save, path, writes, notes)) in [
        (Save::Explicit, RefusedDisplay, 1, 1),
        (Save::Update, RefusedDisplay, 2, 1),
        (Save::Separate, RefusedDisplay, 3, 2),
        (Save::Explicit, LostAcknowledgment, 4, 2),
        (Save::Update, RefusedDisplay, 4, 2),
        (Save::Separate, ChangedOriginal, 4, 2),
        (Save::Separate, Displayed, 5, 3),
    ]
    .into_iter()
    .enumerate()
    {
        let mode = if path == Displayed {
            "reconnect-source-handoff-writer-insert"
        } else {
            "reconnect-source-handoff-text-refusal"
        };
        let (mut workflow, events, _notify) =
            workflow_with_store(mode, vec![], Duration::ZERO, &store);
        let native = Fixture::new();
        // The fixture can insert directly after the last original without navigating.
        let current = if path == Displayed {
            pages[1]
        } else {
            pages[index % 2]
        };
        native.declare_source(document, current);
        native.declare_source_order(&pages);
        let position = native.successor_position();
        let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(10))?;
        let owner = workflow.resident_source.as_mut().unwrap();
        owner.selection = InstalledReaderSelection::fixture_for_installed_output(
            &base.join(format!("profile-{index}")),
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
        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                .with_test_output(Box::new(RecognitionProbe {
                    conversation: conversation.clone(),
                    confirmed_source: path == Displayed,
                    refuse_writer_text: matches!(path, RefusedDisplay | LostAcknowledgment),
                    insertion_source: (path == Displayed).then_some(current),
                    rendered_text: Some(written.clone()),
                    ..RecognitionProbe::default()
                })),
        );
        let steps = Rc::new(Cell::new(0));
        let revision = format!("Draft revision {}.", index + 1);
        let draft = complete_notebook_draft(&revision);
        let model = SubmissionModel {
            submission: Submission::SaveNotebook(save),
            last_visible_text: Some(revision),
            steps: steps.clone(),
            source: Rc::new(RefCell::new(None)),
            images: vec![],
            text: vec![],
        };
        let before = destination.counts();
        let off_screen = saved_pages.join(format!("{}.rm", pages[(index + 1) % 2]));
        if path == LostAcknowledgment {
            destination.lose_next_ack();
        }
        if path == ChangedOriginal {
            let off_screen = off_screen.clone();
            destination.on_next_discovery(move || {
                // Change a different original after the draft is saved, leaving
                // its identity and the native page order intact.
                fs::write(off_screen, changed_scene).unwrap();
            });
        }
        assert!(workflow.wait_for_idle_batch(None, Some(&config))?);
        let mut service = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        let result =
            super::original_pdf::saved_notebook::with_fixture_paths(&root, &templates, || {
                service.run_iteration_with_trigger(false, Some(&dispatch), Some(&config))
            });
        if path == Displayed {
            result?;
            let response = format!("{SAVED}\n\n{}", draft.strip_prefix("# ").unwrap());
            assert_eq!(
                written.borrow().as_slice(),
                &[crate::workflow::resident_output::format_exchange(
                    SEPARATE_NOTEBOOK,
                    response,
                )]
            );
        } else {
            let refusal = result.unwrap_err();
            if path == ChangedOriginal {
                assert_eq!(
                    refusal.to_string(),
                    "saved notebook changed before placement"
                );
                assert_eq!(destination.counts().1, before.1 + 1);
                assert_eq!(fs::read(&off_screen)?, changed_scene);
                fs::write(&off_screen, original_scene)?;
            } else {
                assert!(
                    refusal.to_string().contains("Writer draft cannot be typed"),
                    "{refusal:#}"
                );
            }
            assert!(written.borrow().is_empty());
        }
        let drafts = store.snapshot_heads_matching(&[Namespace::Conversation], 8, |entry| {
            entry.payload["record"]["text"].as_str() == Some(&draft)
        })?;
        let [head] = drafts.as_slice() else {
            anyhow::bail!("expected exactly one retained draft");
        };
        let Record::Turn(turn) = Ledger::decode(head)? else {
            anyhow::bail!("expected a retained Writer draft");
        };
        assert_eq!(turn.mode, Mode::Writer);
        assert_eq!(turn.outcome, Outcome::Generated);
        if matches!(path, ChangedOriginal | Displayed) {
            let correlation = crate::writer::stage(turn.exchange, "separate-whole-notebook-note");
            let export = ledger
                .export_association(turn.conversation, correlation)?
                .context("expected a journal entry for this draft's export")?;
            if path == Displayed {
                assert_eq!(conversation.get(), Some(turn.conversation));
                assert_eq!(export.outcome, Outcome::Completed);
                assert!(export.native_note.is_some());
            } else {
                assert_eq!(export.outcome, Outcome::Canceled);
                assert!(export.attempt.is_some_and(|attempt| attempt.no_send));
            }
        }
        assert_eq!(steps.get(), 4);
        assert!(!position.exists());
        assert_eq!(events.borrow().contains(&"insert"), path == Displayed);
        assert_eq!(destination.counts().2, writes);
        assert_eq!(destination.remote_note_count(), notes);
    }
    fs::remove_dir_all(base)?;
    Ok(())
}

#[test]
fn installed_source_page_saves_reuse_the_note_after_placement_refusal() -> Result<()> {
    let base = std::env::temp_dir().join(format!("page-save-{}", ConversationId::new_v4()));
    let config: crate::config::Config = serde_json::from_value(serde_json::json!({
        "paths": StorePaths {
            data: base.join("data"),
            cache: base.join("cache"),
            credentials: base.join("credentials"),
        },
        "product_settings": {
            "reader_enabled": true,
            "writer_enabled": true,
            "export_destination": "mem"
        }
    }))?;
    let store = Arc::new(Store::open(config.paths.clone())?);
    let ledger = Ledger::new(store.clone());
    let mut dispatch =
        Dispatch::local(&config, &ledger)?.with_regional_writer_model("fixture".into());
    let destination = Arc::new(crate::writer_flow::tests::Destination::default());
    dispatch.test_note_destination = Some(destination.clone());
    let document = ConversationId::new_v4();
    let page = ConversationId::new_v4();
    let mut primary = None;
    let mut conversation = None;

    for (index, (save, writes, notes)) in [
        (Save::Explicit, 1, 1),
        (Save::Update, 2, 1),
        (Save::Separate, 3, 2),
        (Save::Explicit, 4, 2),
        (Save::Update, 4, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let mode = "reconnect-source-handoff-text-refusal";
        let (mut workflow, events, _notify) =
            workflow_with_store(mode, vec![], Duration::ZERO, &store);
        let native = Fixture::new();
        native.declare_source(document, page);
        let position = native.successor_position();
        let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(5))?;
        let owner = workflow.resident_source.as_mut().unwrap();
        owner.selection = InstalledReaderSelection::fixture_for_installed_output(
            &base.join(format!("profile-{index}")),
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
        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                .with_test_output(Box::new(RecognitionProbe {
                    refuse_writer_text: true,
                    ..RecognitionProbe::default()
                })),
        );
        let steps = Rc::new(Cell::new(0));
        let model = SubmissionModel {
            submission: Submission::Save(save),
            last_visible_text: None,
            steps: steps.clone(),
            source: Rc::new(RefCell::new(None)),
            images: vec![],
            text: vec![],
        };
        if index == 3 {
            destination.lose_next_ack();
        }
        let before = destination.counts();
        assert!(workflow.wait_for_idle_batch(None, Some(&config))?);
        let mut service = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        let result = service.run_iteration_with_trigger(false, Some(&dispatch), Some(&config));
        let refusal = result.unwrap_err();
        assert!(
            refusal.to_string().contains("Writer draft cannot be typed"),
            "{refusal:#}"
        );
        assert_eq!(steps.get(), 3);
        assert!(!position.exists());
        assert!(!events.borrow().contains(&"insert"));
        assert_eq!(destination.counts().2, writes);
        assert_eq!(destination.remote_note_count(), notes);
        if index == 4 {
            assert_eq!(
                destination.counts(),
                before,
                "uncertain write belongs to reconciliation"
            );
        }

        let heads = store.snapshot_heads_matching(&[Namespace::ExportAssociation], 8, |_| true)?;
        assert_eq!(heads.len(), notes);
        for head in heads {
            let Record::Export(association) = Ledger::decode(&head)? else {
                anyhow::bail!("expected export association");
            };
            assert_eq!(
                *conversation.get_or_insert(association.conversation),
                association.conversation
            );
            let (id, note) =
                primary.get_or_insert_with(|| (association.id, association.native_note.clone()));
            if *id == association.id {
                assert_eq!(*note, association.native_note);
                assert_eq!(
                    association.outcome,
                    if index >= 3 {
                        Outcome::ReconcileRequired
                    } else {
                        Outcome::Completed
                    }
                );
            } else {
                assert_ne!(*note, association.native_note);
                assert_eq!(association.outcome, Outcome::Completed);
            }
        }
        let root = store
            .value(Namespace::Conversation, conversation.unwrap())?
            .context("save conversation missing")?;
        let Record::Root(root) = Ledger::decode(&root)? else {
            anyhow::bail!("expected conversation root");
        };
        assert!(
            root.binding.is_none(),
            "test requires an unbound source for each save"
        );
    }

    let records = ledger.inspect(conversation.unwrap(), false)?;
    let captures: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::ResidentCapture(evidence) => Some(evidence),
            _ => None,
        })
        .collect();
    assert_eq!(captures.len(), 5);
    assert!(captures
        .iter()
        .all(|evidence| evidence.before.document == document && evidence.before.page == page));
    assert_eq!(
        captures
            .iter()
            .map(|evidence| evidence.correspondence.capture_operation)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        5
    );
    drop((dispatch, ledger, store));
    std::fs::remove_dir_all(base)?;
    Ok(())
}

#[test]
fn shared_lower_right_submission_routes_reader_and_unqueued_writer() -> Result<()> {
    for (reader_enabled, submission) in [
        (true, Submission::Reader),
        (true, Submission::CurrentPage),
        (false, Submission::CurrentPage),
        (true, Submission::BoundHistory),
        (true, Submission::Circled(Circled::Selected)),
        (false, Submission::Circled(Circled::Selected)),
        (true, Submission::Circled(Circled::MissingOutline)),
        (true, Submission::WholeNotebook),
        (false, Submission::WholeNotebook),
        (true, Submission::Save(Save::Explicit)),
        (false, Submission::Save(Save::Ambiguous)),
        (true, Submission::Save(Save::Compact)),
        (false, Submission::Save(Save::CompactUncertain)),
    ] {
        let writer_request = submission.expects_writer_output();
        let circled = submission.circled();
        let bound_history = submission == Submission::BoundHistory;
        let save = submission.save();
        let compact = save.is_some_and(Save::compact);
        let inserts = compact || circled == Some(Circled::Selected);
        let mode = match (inserts, writer_request) {
            (true, _) => "reconnect-source-handoff-writer-insert",
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
            "research_policy": "disabled",
            "product_settings": {
                "reader_enabled": reader_enabled,
                "writer_enabled": true,
                "export_destination": if compact || save == Some(Save::Ambiguous) { "mem" } else { "disabled" }
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
            insertion_source: inserts.then_some(page),
            successor: (writer_request && !inserts).then(|| SuccessorProbe {
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
        let mut dispatch =
            Dispatch::local(&config, &ledger)?.with_regional_writer_model("fixture".into());
        let destination = Arc::new(crate::writer_flow::tests::Destination::default());
        dispatch.test_note_destination = Some(destination.clone());
        if save == Some(Save::CompactUncertain) {
            destination.lose_next_ack();
        }
        let steps = Rc::new(Cell::new(0));
        let source = Rc::new(RefCell::new(None));
        let model = SubmissionModel {
            submission,
            last_visible_text,
            steps: steps.clone(),
            source: source.clone(),
            images: vec![],
            text: vec![],
        };

        assert!(workflow.wait_for_idle_batch(None, Some(&config))?);
        let mut service = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        let result = service.run_iteration_with_trigger(false, Some(&dispatch), Some(&config));
        let notebook_unavailable = submission == Submission::WholeNotebook;
        if notebook_unavailable {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Whole-notebook Writer needs complete ordered coverage of the notebook's source pages"
            );
            assert!(!events.borrow().contains(&"insert"));
            assert!(!position.exists());
            assert_eq!(destination.counts(), (0, 0, 0));
        } else {
            result?;
        }
        if bound_history || circled == Some(Circled::MissingOutline) || notebook_unavailable {
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
                &[format!("Q: {AMBIGUOUS_SAVE}\n\nA: {CLARIFICATION}\n")]
            );
            assert_eq!(destination.counts(), (0, 0, 0));
            let id = conversation
                .get()
                .context("Placement conversation absent")?;
            drop((service, dispatch, ledger, store));
            let reopened = Ledger::new(Arc::new(Store::open(config.paths.clone())?));
            let history = reopened.context(
                id,
                &ContextBudget {
                    max_text_bytes: 4096,
                    max_turns: 4,
                    provider_token_limit: Some(4096),
                },
                |_| Ok(40),
            )?;
            let [user, answer] = history.turns.as_slice() else {
                anyhow::bail!("expected one saved clarification exchange");
            };
            assert_eq!(user.role, Role::User);
            assert_eq!(user.text.as_deref(), Some(AMBIGUOUS_SAVE));
            assert_eq!(answer.role, Role::Assistant);
            assert_eq!(answer.outcome, Outcome::Clarification);
            assert_eq!(answer.text.as_deref(), Some(CLARIFICATION));
            assert_eq!(user.exchange, answer.exchange);
            assert!(!reopened
                .inspect(id, false)?
                .iter()
                .any(|record| matches!(record, Record::Export(_))));
            drop(reopened);
            std::fs::remove_dir_all(base)?;
            continue;
        }
        assert_eq!(steps.get(), if writer_request { 3 } else { 4 });
        assert!(!events.borrow().contains(&"current-target-writer-received"));
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == "insert")
                .count(),
            usize::from(inserts)
        );
        let records = ledger.inspect(
            conversation
                .get()
                .context("Placement conversation absent")?,
            false,
        )?;

        if compact {
            use crate::workflow::resident_output::{
                development::fits_test_keyboard, format_exchange,
                ResidentOutputText::WriterSourceDraft,
            };

            let draft = draft_at_export_display_boundary();
            let draft_body = draft.strip_prefix("# ").unwrap();
            let draft_only = format_exchange(SAVE, draft_body);
            let complete_save = format_exchange(SAVE, format_args!("{SAVED}\n\n{draft_body}"));
            assert!(fits_test_keyboard(WriterSourceDraft(&draft_only), true));
            assert!(fits_test_keyboard(WriterSourceDraft(&complete_save), false));
            assert!(!fits_test_keyboard(WriterSourceDraft(&complete_save), true));
            let (status, outcome) = if save == Some(Save::CompactUncertain) {
                (UNCERTAIN, Outcome::ReconcileRequired)
            } else {
                (SAVED, Outcome::Completed)
            };
            assert_eq!(
                written.borrow().as_slice(),
                &[format!("{status}\n\nFull draft saved in Buddy history.\n")]
            );
            assert!(records.iter().any(|record| matches!(record, Record::Turn(turn)
                if turn.outcome == Outcome::Generated && turn.text.as_deref() == Some(draft.as_str()))));
            assert!(records
                .iter()
                .any(|record| matches!(record, Record::Export(export)
                if export.outcome == outcome)));
            assert_eq!(
                destination.counts().2,
                1,
                "presentation must not resend the write"
            );
        } else if writer_request {
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

struct PageFirstModel {
    inner: SubmissionModel,
    observed_open_page: std::path::PathBuf,
    switch_to_source: Option<std::path::PathBuf>,
    source_digest: Rc<RefCell<Option<String>>>,
    first: bool,
    refuse_at: Option<usize>,
}

impl crate::LLMEngine for PageFirstModel {
    fn guarded_timeout(&self) -> Option<Duration> {
        self.inner.guarded_timeout()
    }

    fn add_text_content(&mut self, text: &str) {
        self.inner.add_text_content(text);
    }

    fn add_image_content(&mut self, image: &str) {
        self.inner.add_image_content(image);
    }

    fn clear_content(&mut self) {
        self.inner.clear_content();
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
        result.context("fixture provider did not receive a request")?
    }

    fn execute(&mut self) -> Result<String> {
        if self.first {
            self.first = false;
            // Keep the response gated on an independent resident Observe.
            // The old router-first order reaches this gate before the marker.
            let opened = std::fs::read_to_string(&self.observed_open_page)
                .context("provider reached before a Buddy page was observed")?;
            assert!(ConversationId::parse_str(&opened).is_ok());
            if self.inner.steps.get() == 0 {
                let digest = crate::storage::digest(&STANDARD.decode(&self.inner.images[0])?);
                assert_eq!(
                    self.source_digest.borrow().as_deref(),
                    Some(digest.as_str())
                );
            }
        }
        if self.inner.steps.get() == 3 {
            if let Some(switch) = &self.switch_to_source {
                std::fs::write(switch, [])?;
            }
        }
        if self.refuse_at == Some(self.inner.steps.get()) {
            return Err(crate::llm::ProviderRefused.into());
        }
        self.inner.execute()
    }
}

#[derive(Clone, Copy, Debug)]
enum OversizedObservation {
    NativeRefusal,
    FullOrder,
}

#[test]
fn ordinary_submission_opens_before_provider_and_writes_only_to_the_observed_page() -> Result<()> {
    for (only_mode, submission, reuse, switch_page, refuse_at, after_oversized) in [
        (
            Some(Mode::Reader),
            Submission::Reader,
            false,
            false,
            None,
            None,
        ),
        (
            Some(Mode::Writer),
            Submission::Reader,
            false,
            false,
            None,
            None,
        ),
        (None, Submission::Reader, true, false, None, None),
        (None, Submission::CurrentPage, false, false, None, None),
        (None, Submission::Reader, false, true, None, None),
        (None, Submission::Reader, false, false, Some(0), None),
        (None, Submission::Reader, false, false, Some(3), None),
        (None, Submission::CurrentPage, false, false, Some(1), None),
        (
            None,
            Submission::Reader,
            false,
            false,
            None,
            Some(OversizedObservation::NativeRefusal),
        ),
        (
            None,
            Submission::Reader,
            false,
            false,
            None,
            Some(OversizedObservation::FullOrder),
        ),
    ] {
        let reader_enabled = only_mode != Some(Mode::Writer);
        let writer_enabled = only_mode != Some(Mode::Reader);
        let mode = "reconnect-source-handoff-open-first";
        let base =
            std::env::temp_dir().join(format!("open-before-model-{}", ConversationId::new_v4()));
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({
            "paths": StorePaths {
                data: base.join("data"),
                cache: base.join("cache"),
                credentials: base.join("credentials"),
            },
            "research_policy": "disabled",
            "product_settings": { "reader_enabled": reader_enabled, "writer_enabled": writer_enabled }
        }))?;
        let store = Arc::new(Store::open(config.paths.clone())?);
        let ledger = Ledger::new(store.clone());
        let (mut workflow, events, _notify) =
            workflow_with_store(mode, vec![], Duration::ZERO, &store);
        let native = Fixture::new();
        let document = ConversationId::new_v4();
        let page = ConversationId::new_v4();
        let buddy = ConversationId::new_v4();
        native.declare_source(document, page);
        let oversized_source = after_oversized.map(|observation| {
            let order: Vec<_> = (0..18).map(|_| ConversationId::new_v4()).collect();
            let reject = matches!(observation, OversizedObservation::NativeRefusal);
            (
                observation,
                native.temporary_source(ConversationId::new_v4(), &order, reject),
            )
        });
        let document_key = Uuid::parse(&document.to_string()).unwrap();
        let source_key = Uuid::parse(&page.to_string()).unwrap();
        let buddy_key = Uuid::parse(&buddy.to_string()).unwrap();
        let source_facts = ReaderFacts::new(
            remarkable_open_sdk::PageKey {
                document: document_key,
                page: source_key,
            },
            vec![source_key],
        )
        .unwrap();
        let existing_facts = ReaderFacts::new(
            remarkable_open_sdk::PageKey {
                document: document_key,
                page: buddy_key,
            },
            vec![source_key, buddy_key],
        )
        .unwrap();
        let (observed_open_page, switch) = native.opened_page_probe();
        let position = native.successor_position();
        if reuse {
            native.declare_source_order(&[page, buddy]);
        }
        let case = format!(
            "only mode={only_mode:?}, reuse={reuse}, switch={switch_page}, fail at {refuse_at:?}, after oversized={after_oversized:?}"
        );
        let runtime = native
            .start_with_store(store.clone(), mode, Duration::from_secs(10))
            .with_context(|| case.clone())?;
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
        let was_classified = Rc::new(Cell::new(None));
        let written = Rc::new(RefCell::new(Vec::new()));
        let source_digest = Rc::new(RefCell::new(None));
        let mut backend = RecognitionProbe {
            conversation: was_classified.clone(),
            confirmed_source: true,
            early_source: Some((source_facts.clone(), source_digest.clone())),
            successor: reuse.then(|| SuccessorProbe {
                position: position.clone(),
                interrupt_restore: false,
            }),
            rendered_text: Some(written.clone()),
            ..RecognitionProbe::default()
        };
        if reuse {
            let conversation = ConversationId::new_v4();
            ledger.create(conversation, ConversationId::new_v4(), 1)?;
            backend.bind_opened_page(&ledger, conversation, &existing_facts)?;
            backend.early_source.as_mut().unwrap().0 =
                ReaderFacts::new(source_facts.page(), existing_facts.order().to_vec()).unwrap();
        }
        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(
                (0..1 + usize::from(after_oversized.is_some()))
                    .map(|_| released(Instant::now()))
                    .collect(),
            )
            .with_test_output(Box::new(backend)),
        );
        let dispatch =
            Dispatch::local(&config, &ledger)?.with_regional_writer_model("fixture".into());
        let steps = Rc::new(Cell::new(usize::from(!writer_enabled)));
        let model = PageFirstModel {
            inner: SubmissionModel {
                submission,
                last_visible_text: None,
                steps: steps.clone(),
                source: Rc::new(RefCell::new(None)),
                images: vec![],
                text: vec![],
            },
            observed_open_page: observed_open_page.clone(),
            switch_to_source: switch_page.then_some(switch),
            source_digest: source_digest.clone(),
            first: true,
            refuse_at,
        };
        assert!(workflow.wait_for_idle_batch(None, Some(&config))?);
        let mut service = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        if let Some((observation, path)) = oversized_source {
            let error = service
                .run_iteration_with_trigger(false, Some(&dispatch), Some(&config))
                .expect_err("the SDK cannot capture an 18-page order");
            let expected = match observation {
                OversizedObservation::NativeRefusal => "Resident source observation refused",
                OversizedObservation::FullOrder => "supports at most 17 pages (observed 18)",
            };
            assert!(error.to_string().contains(expected), "{error:#}");
            assert_eq!(events.borrow().as_slice(), &["observe"]);
            assert_eq!(steps.get(), 0);
            assert!(!observed_open_page.exists());
            assert!(written.borrow().is_empty());
            assert!(super::super::super::resident_client::attempts(&store)?.is_empty());
            std::fs::remove_file(path)?;
        }
        let result = service.run_iteration_with_trigger(
            after_oversized.is_some(),
            Some(&dispatch),
            Some(&config),
        );
        let opened = ConversationId::parse_str(&std::fs::read_to_string(observed_open_page)?)?;
        assert_ne!(opened, page);
        if reuse {
            assert_eq!(opened, buddy);
            assert!(position.exists());
        }
        let conversation = was_classified
            .get()
            .context("source classification absent")?;
        let records = ledger.inspect(conversation, false)?;
        let write_count = written.borrow().len();
        let disabled_reader = submission == Submission::Reader && !reader_enabled;
        let refuse = refuse_at.is_some() || disabled_reader;
        if switch_page || refuse {
            assert!(result.is_err(), "{result:?}");
            let (request, message) = match (switch_page, refuse_at) {
                (false, None) if disabled_reader => {
                    assert_eq!(steps.get(), 1, "only the shared router may call the model");
                    (
                        None,
                        "Reader is turned off. Enable it in Manager to ask a reading question.",
                    )
                }
                (true, None) => (Some(QUESTION), "This request was canceled."),
                (_, Some(0)) => (
                    None,
                    "The response service could not complete this request.",
                ),
                (_, Some(3)) => (
                    Some(QUESTION),
                    "The response service could not complete this request.",
                ),
                (_, Some(1)) => (Some(NOTE), "This request could not be completed."),
                _ => unreachable!(),
            };
            let budget = ContextBudget {
                max_text_bytes: 4096,
                max_turns: 12,
                provider_token_limit: Some(4096),
            };
            // Reconstruct the ordinary persisted conversation view after the
            // service returned, including the exact request and one safe reply.
            use crate::conversation_layout::{plan, Geometry, PresentationIdentity};
            let view = ledger.presentation_context(conversation, &budget, |_| Ok(1))?;
            let display = plan(
                PresentationIdentity::Persisted {
                    conversation,
                    document,
                    page: opened,
                },
                &view,
                "",
                Geometry::default(),
            )?;
            let matching: Vec<_> = display
                .exchanges
                .iter()
                .flat_map(|item| &item.turns)
                .filter(|turn| {
                    turn.role == Role::Assistant && turn.text.as_deref() == Some(message)
                })
                .collect();
            let [reply] = matching.as_slice() else {
                anyhow::bail!("expected one failure reply, got {:?}", view.turns);
            };
            assert_eq!(
                reply.mode,
                if submission == Submission::CurrentPage {
                    Mode::Writer
                } else {
                    Mode::Reader
                }
            );
            assert!(view.turns.iter().any(|turn| turn.exchange == reply.exchange
                && turn.role == Role::User
                && turn.text.as_deref() == request));
            assert!(ledger
                .context(conversation, &budget, |_| Ok(1))?
                .turns
                .iter()
                .all(|turn| turn.id != reply.id));
            let projection = crate::workflow::exchange_history::project(
                &ledger,
                conversation,
                &crate::workflow::exchange_history::PageOwner {
                    document,
                    page: opened,
                    visit: "fixture-redisplay".into(),
                    session: "fixture-redisplay".into(),
                },
                "",
            )?;
            let reopened = projection
                .exchanges
                .iter()
                .find(|exchange| exchange.id == reply.exchange)
                .context("original exchange absent from redisplay")?;
            assert_eq!(
                reopened
                    .turns
                    .iter()
                    .filter(|turn| turn.text == message)
                    .count(),
                1
            );
            assert!(reopened
                .turns
                .iter()
                .any(|turn| turn.text == request.unwrap_or_default()));
            if switch_page {
                assert!(records
                    .iter()
                    .any(|record| matches!(record, Record::Turn(turn)
                    if turn.role == Role::Assistant && turn.text.as_deref() == Some("Four."))));
            }
            assert_eq!(write_count, usize::from(refuse));
            if refuse {
                let expected = request.map_or_else(
                    || format!("A: {message}\n"),
                    |request| crate::workflow::resident_output::format_exchange(request, message),
                );
                assert_eq!(written.borrow().as_slice(), &[expected]);
            }
        } else {
            result.with_context(|| case.clone())?;
            assert_eq!(write_count, 1);
            assert!(
                written.borrow()[0].contains(if submission == Submission::Reader {
                    "Four."
                } else {
                    "Page note"
                })
            );
            assert!(records
                .iter()
                .any(|record| matches!(record, Record::ResidentCapture(capture)
                if capture.before.document == document && capture.before.page == page
                    && source_digest.borrow().as_deref() == Some(capture.original_png.sha256.as_str()))));
        }
        let attempts = super::super::super::resident_client::attempts(&store)?;
        assert_eq!(attempts.len(), usize::from(!reuse));
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == "insert")
                .count(),
            usize::from(!reuse)
        );
        if let Some(attempt) = attempts.first() {
            assert_eq!(attempt.conversation, conversation);
            assert!(
                matches!(attempt.reply, Some(Reply::InsertionObserved { target, .. })
                if target.page.as_bytes() == opened.as_bytes())
            );
        }
        drop((service, dispatch, ledger, store));
        std::fs::remove_dir_all(base)?;
    }
    Ok(())
}
