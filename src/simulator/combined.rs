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

fn seed_image(ledger: &Ledger, conversation: Uuid, source: &SourceObservation) -> Result<String> {
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_luma8(2, 2).write_to(&mut png, image::ImageFormat::Png)?;
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

pub(super) fn run(scenario: Scenario) -> Result<Value> {
    ensure!(scenario.mode == "combined", "invalid combined mode");
    ensure!(
        scenario
            .requests
            .iter()
            .all(|request| !request.trim().is_empty() && request.len() <= 4096),
        "invalid scripted requests"
    );
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open()?;
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let source = SourceObservation {
        document: scenario.document.document,
        page: Uuid::new_v4(),
        session: "simulated-session".into(),
        visit: "simulated-visit".into(),
        content_revision: Some("simulated-content".into()),
        order_revision: None,
        capability_revision: "simulated-v1".into(),
        evidence_procedure: "combined-fixture".into(),
    };
    let mut observe = || Ok(source.clone());
    let image = seed_image(&ledger, conversation, &source)?;
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
    dispatch.record_reader_output(
        &ledger,
        &output,
        reader_draft,
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
    let draft = dispatch.writer(&ledger, &mut engine, &generation, &current, &mut observe)?;
    ensure!(
        draft.status == writer::Status::Generated,
        "combined Writer failed: {:?}",
        draft.failure
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
        json!({"mode":"combined","model_calls":engine.seen.len(),"destination_writes":destination.writes(),"blank_starts":opened}),
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
