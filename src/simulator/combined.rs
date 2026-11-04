use super::{writer::Destination, SimulatorLedger};
use crate::{
    config::Config,
    conversation::{ImageUse, Ledger, Mode, Outcome, Record, Role, SourceObservation, Turn},
    retrieval::NormalizedDocument,
    storage::{digest, Media, Namespace, Uuid},
    workflow::conversation_dispatch::{Dispatch, ExportDecision, RequestScope},
    writer, writer_flow, LLMEngine,
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::VecDeque, io::Cursor};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    mode: String,
    #[serde(default)]
    notebook: bool,
    requests: [String; 3],
    replies: [String; 3],
    preference: String,
    document: NormalizedDocument,
}

#[derive(Default)]
struct Model {
    text: Vec<String>,
    images: Vec<String>,
    seen: Vec<(Vec<String>, Vec<String>)>,
    replies: VecDeque<String>,
    after_execute: Option<Box<dyn FnOnce() -> Result<()>>>,
}

impl LLMEngine for Model {
    fn clear_content(&mut self) {
        self.text.clear();
        self.images.clear();
    }
    fn add_text_content(&mut self, text: &str) {
        self.text.push(text.into());
    }
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.into());
    }
    fn execute(&mut self) -> Result<String> {
        self.seen.push((self.text.clone(), self.images.clone()));
        if let Some(action) = self.after_execute.take() {
            action()?;
        }
        self.replies
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("scripted reply exhausted"))
    }
}

fn scope(ledger: &Ledger, conversation: Uuid, source: &SourceObservation) -> Result<RequestScope> {
    Ok(RequestScope {
        conversation,
        operation: Uuid::new_v4(),
        root: ledger.expected(Namespace::Conversation, conversation)?,
        source: source.clone(),
    })
}

fn seed_image(
    ledger: &Ledger,
    conversation: Uuid,
    source: &SourceObservation,
    tone: u8,
) -> Result<String> {
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 2, image::Luma([tone])))
        .write_to(&mut png, image::ImageFormat::Png)?;
    let png = png.into_inner();
    let image = Media {
        sha256: digest(&png),
        bytes: png.len() as u64,
        media_type: "image/png".into(),
    };
    ledger.stage_image(&image, png.as_slice())?;
    let turn = Uuid::new_v4();
    let id = Uuid::new_v4();
    ledger.append(
        Uuid::new_v4(),
        ledger.expected(Namespace::Conversation, conversation)?,
        Turn {
            id: turn,
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some("Earlier image question?".into()),
            sources: vec![id],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        },
        vec![ImageUse {
            id,
            conversation,
            turn,
            image,
            observation: source.clone(),
            width: 2,
            height: 2,
            purpose: "overview".into(),
            ordinal: 0,
            parent: None,
            parent_dimensions: None,
            crop: None,
            transform: [1., 0., 0., 1., 0., 0.],
            target: None,
            captured_ms: 1,
        }],
    )?;
    Ok(STANDARD.encode(png))
}

fn fixture_source(document: Uuid) -> SourceObservation {
    SourceObservation {
        document,
        page: Uuid::new_v4(),
        session: "simulated-session".into(),
        visit: "simulated-visit".into(),
        content_revision: Some("simulated-content".into()),
        order_revision: None,
        capability_revision: "simulated-v1".into(),
        evidence_procedure: "combined-fixture".into(),
    }
}

pub(super) fn run(scenario: Scenario) -> Result<Value> {
    ensure!(scenario.mode == "combined", "invalid combined mode");
    ensure!(
        scenario
            .requests
            .iter()
            .all(|request| !request.trim().is_empty() && request.len() <= 4096),
        "invalid scripted requests"
    );
    let notebook_model_calls = scenario
        .notebook
        .then(|| run_notebook(&scenario))
        .transpose()?;
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open()?;
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let source = fixture_source(scenario.document.document);
    let mut observe = || Ok(source.clone());
    let image = seed_image(&ledger, conversation, &source, 0)?;
    let config: Config = serde_json::from_value(
        json!({"personalization":{"enabled":true,"text":scenario.preference}}),
    )?;
    let mut dispatch = Dispatch::local(&config, &ledger)?;
    use crate::device::interaction::Interaction;
    ensure!(
        dispatch.reader_trigger(&[Interaction::Reader])
            && !dispatch.reader_trigger(&[Interaction::Reader, Interaction::Undo])
            && !dispatch.reader_trigger(&[Interaction::Reader, Interaction::Reader]),
        "idle interaction arbitration changed"
    );
    let first_reply = scenario.replies[0].clone();
    let writer_reply = scenario.replies[1].clone();
    let mut engine = Model {
        replies: scenario.replies.into(),
        ..Model::default()
    };
    let destination = Destination::default();

    let initial = scope(&ledger, conversation, &source)?;
    let mut opened = 0;
    dispatch.blank_start(&ledger, &initial, &mut observe, || {
        opened += 1;
        Ok(())
    })?;
    ensure!(
        dispatch
            .blank_start(&ledger, &initial, &mut observe, || anyhow::bail!(
                "duplicate opened"
            ))
            .is_err(),
        "duplicate blank start"
    );
    ensure!(
        opened == 1
            && engine.seen.is_empty()
            && initial.root == ledger.expected(Namespace::Conversation, conversation)?,
        "blank start changed history or called model"
    );
    ensure!(
        matches!(dispatch.export_draft(&ledger, &mut engine, &destination, None, &initial, &mut observe)?, ExportDecision::Clarify(text) if !text.is_empty())
            && destination.writes() == 0,
        "ambiguous export dispatched"
    );

    let reader_draft = dispatch.reader(
        &ledger,
        &mut engine,
        &scenario.requests[0],
        None,
        &initial,
        &mut observe,
    )?;
    let output = scope(&ledger, conversation, &source)?;
    dispatch.record_output(
        &ledger,
        &output,
        &reader_draft,
        crate::conversation::CompletionEvidence {
            native_operation: output.operation,
            exact_text: first_reply.clone(),
            observation: source.clone(),
            procedure: "combined-simulator-readback".into(),
        },
        &mut observe,
    )?;
    ensure!(
        dispatch
            .reader(
                &ledger,
                &mut engine,
                &scenario.requests[0],
                None,
                &initial,
                &mut observe
            )
            .is_err(),
        "stale Reader request accepted"
    );
    let current = scope(&ledger, conversation, &source)?;
    let generation = writer::Request::Generate {
        conversation,
        operation: current.operation,
        request: scenario.requests[1].clone(),
        document: scenario.document,
        selection: crate::conversation::TextScope::SuppliedNotebook,
        guidance: None,
    };
    let (draft, output) =
        dispatch.writer(&ledger, &mut engine, &generation, &current, &mut observe)?;
    ensure!(
        draft.status == writer::Status::Generated,
        "combined Writer failed: {:?}",
        draft.failure
    );
    let output = output.ok_or_else(|| anyhow::anyhow!("missing Writer output witness"))?;
    let visible = scope(&ledger, conversation, &source)?;
    let receipt = crate::conversation::CompletionEvidence {
        native_operation: visible.operation,
        exact_text: writer_reply.clone(),
        observation: source.clone(),
        procedure: "combined-simulator-writer-readback".into(),
    };
    let mut wrong = receipt.clone();
    wrong.exact_text.push('!');
    let mut departed = source.clone();
    departed.visit = "different-visit".into();
    ensure!(
        dispatch
            .record_output(&ledger, &visible, &output, wrong, &mut observe)
            .is_err(),
        "mismatched Writer output recorded"
    );
    ensure!(
        dispatch
            .record_output(&ledger, &visible, &output, receipt.clone(), &mut || Ok(
                departed.clone()
            ))
            .is_err(),
        "late Writer output recorded"
    );
    dispatch.record_output(&ledger, &visible, &output, receipt.clone(), &mut observe)?;
    ensure!(
        dispatch
            .record_output(&ledger, &visible, &output, receipt, &mut observe)
            .is_err(),
        "stale Writer output recorded"
    );
    let current = scope(&ledger, conversation, &source)?;
    let export = writer_flow::Request::ExportDraft {
        outer_operation: current.operation,
        conversation,
        draft: draft
            .draft
            .ok_or_else(|| anyhow::anyhow!("draft missing"))?,
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    ensure!(
        matches!(dispatch.export_draft(&ledger, &mut engine, &destination, Some(&export), &current, &mut observe)?, ExportDecision::Attempt(report) if report.status == writer_flow::Status::Exported),
        "combined Writer export failed"
    );
    let current = scope(&ledger, conversation, &source)?;
    dispatch.reader(
        &ledger,
        &mut engine,
        &scenario.requests[2],
        None,
        &current,
        &mut observe,
    )?;

    ensure!(
        engine.seen.len() == 3 && destination.writes() == 1,
        "unexpected operation count"
    );
    let mut expected = vec!["Earlier image question?".to_owned()];
    for (index, ((text, images), request)) in engine.seen.iter().zip(&scenario.requests).enumerate()
    {
        ensure!(
            text.len() == 3
                && text[1].ends_with(&config.personalization.text)
                && images == &[image.clone()],
            "receiving parts changed"
        );
        let payload: Value = serde_json::from_str(&text[2])?;
        ensure!(payload["request"] == *request, "request changed");
        let history: Vec<Turn> = serde_json::from_value(payload["history"].clone())?;
        ensure!(
            history
                .iter()
                .filter_map(|turn| turn.text.clone())
                .collect::<Vec<_>>()
                == expected,
            "history changed"
        );
        expected.push(request.clone());
        if index == 0 {
            expected.push(first_reply.clone());
        } else if index == 1 {
            expected.push(writer_reply.clone());
        }
    }
    ensure!(
        engine.seen[0].0[0] == engine.seen[2].0[0] && engine.seen[0].0[0] != engine.seen[1].0[0],
        "receiver presets changed"
    );
    let users: Vec<_> = ledger
        .inspect(conversation, false)?
        .into_iter()
        .filter_map(|record| match record {
            Record::Turn(turn) if turn.role == Role::User => turn.text,
            _ => None,
        })
        .collect();
    ensure!(
        users
            == std::iter::once("Earlier image question?".to_owned())
                .chain(scenario.requests.iter().cloned())
                .collect::<Vec<_>>(),
        "visible requests changed"
    );
    for (reader, writer) in [(true, false), (false, true), (false, false)] {
        let config: Config = serde_json::from_value(
            json!({"product_settings":{"reader_enabled":reader,"writer_enabled":writer}}),
        )?;
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        ensure!(
            dispatch.reader_trigger(&[Interaction::Reader]) == reader
                && dispatch.admits(Mode::Writer, false) == writer,
            "saved module admission changed"
        );
        let current = scope(&ledger, conversation, &source)?;
        if !reader {
            ensure!(
                dispatch
                    .reader(
                        &ledger,
                        &mut engine,
                        "Disabled Reader",
                        None,
                        &current,
                        &mut observe
                    )
                    .is_err(),
                "disabled Reader ran"
            );
        }
        if !writer {
            ensure!(
                dispatch
                    .writer(&ledger, &mut engine, &generation, &current, &mut observe)
                    .is_err(),
                "disabled Writer ran"
            );
        }
    }
    let mut nonlocal = config.clone();
    nonlocal.sync.enabled = true;
    ensure!(
        Dispatch::local(&nonlocal, &ledger).is_err(),
        "nonlocal configuration entered unified dispatch"
    );

    let cancel = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = cancel.clone();
    let mut late = Model {
        replies: ["Late reply".to_owned()].into(),
        after_execute: Some(Box::new(move || {
            flag.set(true);
            Ok(())
        })),
        ..Model::default()
    };
    let current = scope(&ledger, conversation, &source)?;
    let mut cancel_observation = || {
        ensure!(!cancel.get(), "simulated cancel");
        Ok(source.clone())
    };
    ensure!(
        dispatch
            .reader(
                &ledger,
                &mut late,
                "Cancel after model dispatch",
                None,
                &current,
                &mut cancel_observation
            )
            .is_err()
            && late.seen.len() == 1,
        "late canceled reply admitted"
    );
    let current = scope(&ledger, conversation, &source)?;
    let replacement = Ledger::new(ledger.store().clone());
    let mut replaced = Model {
        replies: ["Stale reply".to_owned()].into(),
        after_execute: Some(Box::new(move || {
            replacement.append(
                Uuid::new_v4(),
                replacement.expected(Namespace::Conversation, conversation)?,
                Turn {
                    id: Uuid::new_v4(),
                    conversation,
                    exchange: Uuid::new_v4(),
                    sequence: 0,
                    role: Role::User,
                    mode: Mode::Reader,
                    outcome: Outcome::Interpreted,
                    text: Some("Synthetic changed revision".into()),
                    sources: vec![],
                    correction_of: None,
                    created_ms: 2,
                    updated_ms: 2,
                    completion: None,
                },
                vec![],
            )?;
            Ok(())
        })),
        ..Model::default()
    };
    ensure!(
        dispatch
            .reader(
                &ledger,
                &mut replaced,
                "Replace revision after model dispatch",
                None,
                &current,
                &mut observe
            )
            .is_err()
            && replaced.seen.len() == 1,
        "stale reply admitted"
    );
    ensure!(!ledger.inspect(conversation, false)?.iter().any(|record| matches!(record, Record::Turn(turn) if matches!(turn.text.as_deref(), Some("Late reply" | "Stale reply")))), "unowned output persisted");
    Ok(
        json!({"mode":"combined","model_calls":engine.seen.len(),"notebook_model_calls":notebook_model_calls,"destination_writes":destination.writes(),"blank_starts":opened}),
    )
}

#[test]
fn maintained_combined_fixture_runs() {
    let scenario = serde_json::from_str(include_str!(
        "../../docs/simulator/scenarios/combined-reader-writer-reader.json"
    ))
    .unwrap();
    run(scenario).unwrap();
}

fn run_notebook(scenario: &Scenario) -> Result<usize> {
    use writer::notebook::{
        command::{Generate, Refine, Request},
        runner::Status,
        ValidatedNotebook,
    };
    let fixture = SimulatorLedger::new();
    let ordinary = fixture.open()?;
    let paths = ordinary.store().paths.clone();
    drop(ordinary);
    let ledger = Ledger::new(std::sync::Arc::new(
        crate::storage::Store::prepare_notebook(paths)?,
    ));
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let source = fixture_source(scenario.document.document);
    let mut observe = || Ok(source.clone());
    let image = seed_image(&ledger, conversation, &source, 0)?;
    let config = Config::default();
    let mut dispatch = Dispatch::local(&config, &ledger)?;
    let admitted = scope(&ledger, conversation, &source)?;
    let raw = serde_json::to_vec(&scenario.document)?;
    let sha256 = digest(&raw);
    let frozen = ValidatedNotebook::from_bytes(&raw, &sha256)?;
    let request = Request::GenerateNotebook(Box::new(Generate {
        conversation,
        operation: admitted.operation,
        request: scenario.requests[1].clone(),
        document_file: fixture.0.join("unused-owner-frozen-input.json"),
        document_sha256: sha256,
        selection: crate::conversation::TextScope::SuppliedNotebook,
        guidance: None,
        max_model_calls: 2,
        history_range: None,
    }));
    let response = scenario.replies[1].clone();
    let mut model = Model {
        replies: ["x = 7".to_owned(), response.clone()].into(),
        ..Model::default()
    };
    let (report, output) = dispatch.notebook(
        &ledger,
        &mut model,
        &request,
        Some(frozen.clone()),
        &admitted,
        &mut observe,
    )?;
    ensure!(
        report.status == Status::Generated && model.seen.len() == 2,
        "routed notebook failed: {:?}",
        report.failure
    );
    ensure!(
        model.seen[0].1.is_empty() && model.seen[1].1 == [image.clone()],
        "notebook stage attachments changed"
    );
    let reduce: Value = serde_json::from_str(
        model.seen[1]
            .0
            .last()
            .ok_or_else(|| anyhow::anyhow!("reducer payload absent"))?,
    )?;
    ensure!(
        reduce["request"] == scenario.requests[1]
            && reduce["history"][0]["text"] == "Earlier image question?"
            && reduce["historical_png_base64_sha256"] == json!([digest(image.as_bytes())]),
        "reducer receiving context changed"
    );
    let visible = scope(&ledger, conversation, &source)?;
    let draft = report
        .draft
        .ok_or_else(|| anyhow::anyhow!("notebook draft absent"))?;
    dispatch.record_output(
        &ledger,
        &visible,
        &output.ok_or_else(|| anyhow::anyhow!("notebook output witness absent"))?,
        crate::conversation::CompletionEvidence {
            native_operation: visible.operation,
            exact_text: response.clone(),
            observation: source.clone(),
            procedure: "simulated-notebook-writer-readback".into(),
        },
        &mut observe,
    )?;
    let current = scope(&ledger, conversation, &source)?;
    let mut repeated = scope(&ledger, conversation, &source)?;
    repeated.operation = admitted.operation;
    ensure!(
        dispatch
            .notebook(
                &ledger,
                &mut model,
                &request,
                Some(frozen),
                &repeated,
                &mut observe
            )
            .is_err(),
        "routed notebook operation replayed"
    );
    model.replies.push_back(scenario.replies[2].clone());
    dispatch.reader(
        &ledger,
        &mut model,
        &scenario.requests[2],
        None,
        &current,
        &mut observe,
    )?;
    ensure!(model.seen.len() == 3, "notebook route model count changed");
    let reader: Value = serde_json::from_str(
        model.seen[2]
            .0
            .last()
            .ok_or_else(|| anyhow::anyhow!("Reader payload absent"))?,
    )?;
    ensure!(
        reader["history"][2]["text"] == response && model.seen[2].1 == [image],
        "notebook handback changed"
    );
    let refinement = scope(&ledger, conversation, &source)?;
    let request = Request::RefineNotebook(Box::new(Refine {
        conversation,
        operation: refinement.operation,
        request: "Keep the original units.".into(),
        draft,
        max_model_calls: 2,
        history_range: None,
    }));
    let cancel = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = cancel.clone();
    let mut late = Model {
        replies: ["Late map result".to_owned()].into(),
        after_execute: Some(Box::new(move || {
            flag.set(true);
            Ok(())
        })),
        ..Model::default()
    };
    let (refused, output) =
        dispatch.notebook(&ledger, &mut late, &request, None, &refinement, &mut || {
            ensure!(!cancel.get(), "simulated notebook cancellation");
            Ok(source.clone())
        })?;
    ensure!(
        refused.status == Status::Canceled && output.is_none() && late.seen.len() == 1,
        "canceled notebook refinement continued"
    );
    Ok(model.seen.len())
}

#[test]
fn unavailable_or_over_budget_history_never_reaches_the_reader_model() -> Result<()> {
    for missing in [true, false] {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open()?;
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let source = fixture_source(Uuid::new_v4());
        for tone in 0..if missing { 1 } else { 13 } {
            let image = seed_image(&ledger, conversation, &source, tone)?;
            if missing {
                let data = &ledger.store().paths.data;
                let generation: Uuid =
                    serde_json::from_slice(&std::fs::read(data.join("CURRENT"))?)?;
                let object = data
                    .join("generations")
                    .join(generation.to_string())
                    .join("objects")
                    .join(digest(&STANDARD.decode(image)?));
                std::fs::remove_file(object)?;
            }
        }
        let config = Config::default();
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let admitted = scope(&ledger, conversation, &source)?;
        let mut model = Model::default();
        assert!(dispatch
            .reader(
                &ledger,
                &mut model,
                "Use the images",
                None,
                &admitted,
                &mut || Ok(source.clone())
            )
            .is_err());
        assert!(model.seen.is_empty());
    }
    Ok(())
}
