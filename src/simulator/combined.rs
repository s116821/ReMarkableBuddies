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

pub(super) mod qualification;
mod resident;

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

#[cfg(test)]
struct InterruptedNotebookStream<'a> {
    model: Model,
    streaming: &'a std::cell::Cell<bool>,
}

#[cfg(test)]
impl LLMEngine for InterruptedNotebookStream<'_> {
    fn clear_content(&mut self) {
        self.model.clear_content();
    }

    fn add_text_content(&mut self, text: &str) {
        self.model.add_text_content(text);
    }

    fn add_image_content(&mut self, image: &str) {
        self.model.add_image_content(image);
    }

    fn execute(&mut self) -> Result<String> {
        self.model.execute()
    }

    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        // The map stream loses its visit only while the provider is in flight.
        self.streaming.set(self.model.seen.len() == 1);
        let current = progress();
        self.streaming.set(false);
        current?;
        self.execute()
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
) -> Result<(Uuid, String)> {
    let png = fixture_png(tone)?;
    seed_image_bytes(ledger, conversation, source, png, u32::from(tone))
}

fn seed_image_bytes(
    ledger: &Ledger,
    conversation: Uuid,
    source: &SourceObservation,
    png: Vec<u8>,
    ordinal: u32,
) -> Result<(Uuid, String)> {
    use image::GenericImageView;
    let (width, height) = image::load_from_memory(&png)?.dimensions();
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
            width,
            height,
            purpose: "overview".into(),
            ordinal,
            parent: None,
            parent_dimensions: None,
            crop: None,
            transform: [1., 0., 0., 1., 0., 0.],
            target: None,
            captured_ms: 1,
        }],
    )?;
    Ok((id, STANDARD.encode(png)))
}

fn fixture_png(tone: u8) -> Result<Vec<u8>> {
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 2, image::Luma([tone])))
        .write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

#[cfg(test)]
fn fixture_notebook_drawing(with_letter: bool) -> Result<Vec<u8>> {
    let mut canvas = image::GrayImage::from_pixel(40, 32, image::Luma([255]));
    for (start, end) in [
        ((7., 22.), (17., 8.)),
        ((17., 8.), (27., 22.)),
        ((27., 22.), (7., 22.)),
    ] {
        imageproc::drawing::draw_line_segment_mut(&mut canvas, start, end, image::Luma([0]));
    }
    if with_letter {
        imageproc::drawing::draw_line_segment_mut(
            &mut canvas,
            (32., 7.),
            (32., 12.),
            image::Luma([0]),
        );
        imageproc::drawing::draw_line_segment_mut(
            &mut canvas,
            (32., 12.),
            (35., 12.),
            image::Luma([0]),
        );
    }
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(canvas).write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

fn seed_shared_images(
    ledger: &Ledger,
    conversation: Uuid,
    source: &SourceObservation,
) -> Result<(Vec<String>, Value)> {
    use crate::conversation::{LegacyImage, LegacyImageInput, LegacyImageRole};
    let mut turn = Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange: Uuid::new_v4(),
        sequence: 0,
        role: Role::User,
        mode: Mode::Reader,
        outcome: Outcome::Prepared,
        text: None,
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    };
    let capture = Uuid::new_v4();
    let pngs = [fixture_png(0)?, fixture_png(1)?];
    let hashes = pngs.each_ref().map(|bytes| digest(bytes));
    let images = pngs
        .iter()
        .enumerate()
        .map(|(index, bytes)| LegacyImageInput {
            descriptor: LegacyImage {
                media: Media {
                    sha256: hashes[index].clone(),
                    bytes: bytes.len() as u64,
                    media_type: "image/png".into(),
                },
                dimensions: [2, 2],
                role: if index == 0 {
                    LegacyImageRole::Overview
                } else {
                    LegacyImageRole::Detail
                },
                provider_ordinal: Some(index as u32),
            },
            bytes: bytes.clone(),
        })
        .collect();
    ledger.prepare_legacy(
        Uuid::new_v4(),
        ledger.expected(Namespace::Conversation, conversation)?,
        turn.clone(),
        capture,
        None,
        images,
    )?;
    turn.outcome = Outcome::Interpreted;
    turn.text = Some("Earlier image question?".into());
    turn.sources = vec![capture];
    ledger.advance(
        Uuid::new_v4(),
        ledger.expected(Namespace::Conversation, conversation)?,
        ledger.expected(Namespace::Conversation, turn.id)?,
        turn,
    )?;
    let (shared, _) = seed_image(ledger, conversation, source, 0)?;
    Ok((
        pngs.iter().map(|bytes| STANDARD.encode(bytes)).collect(),
        json!([
            {"source":capture,"ordinal":0,"dimensions":[2,2],"attachment_index":0,"sha256":hashes[0]},
            {"source":capture,"ordinal":1,"dimensions":[2,2],"attachment_index":1,"sha256":hashes[1]},
            {"source":shared,"ordinal":0,"dimensions":[2,2],"attachment_index":0,"sha256":hashes[0]},
        ]),
    ))
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
        .then(|| run_notebook(&scenario, true))
        .transpose()?;
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open()?;
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let source = fixture_source(scenario.document.document);
    let mut observe = || Ok(source.clone());
    let (attachments, associations) = seed_shared_images(&ledger, conversation, &source)?;
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
    let mut expected = vec!["Earlier image question?".to_owned(); 2];
    let preference_part =
        crate::config::personalization::receiving_prompt_part(&config.personalization.text);
    for (index, ((text, images), request)) in engine.seen.iter().zip(&scenario.requests).enumerate()
    {
        ensure!(
            text.len() == 3 && text[1] == preference_part && images == &attachments,
            "receiving parts changed"
        );
        let payload: Value = serde_json::from_str(&text[2])?;
        ensure!(
            payload["request"] == *request && payload["historical_images"] == associations,
            "request or image associations changed"
        );
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
            == std::iter::repeat_n("Earlier image question?".to_owned(), 2)
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
fn whole_notebook_writer_uses_complete_ordered_images() -> Result<()> {
    use crate::{
        conversation::notebook::{NotebookStage, NotebookStageKind, NotebookStageOutcome},
        retrieval::notebook::{NotebookSourceReceipt, OrderedNotebookImages, VISUAL_SECTION},
    };
    use writer::notebook::runner::Status;

    let fixture = SimulatorLedger::new();
    let ledger = fixture.open_targets()?;
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let mut source = fixture_source(Uuid::new_v4());
    let no_patterns = std::collections::BTreeMap::new();
    let blank = Uuid::new_v4();
    let buddy = Uuid::new_v4();
    let last = Uuid::new_v4();
    let order = [source.page, blank, buddy, last];
    let buddy_pages = [buddy].into_iter().collect();
    let first_png = fixture_notebook_drawing(false)?;
    let blank_png = fixture_png(255)?;
    let last_png = fixture_notebook_drawing(true)?;
    let originals = [
        (source.page, first_png.as_slice()),
        (blank, blank_png.as_slice()),
        (last, last_png.as_slice()),
    ];
    let images = OrderedNotebookImages {
        document: source.document,
        order: &order,
        originals: &originals,
        buddy_pages: &buddy_pages,
        patterned_paper: &no_patterns,
    };
    source.order_revision = Some(images.order_revision()?);
    // Model the owner's saved-scene sampling independently of the rendered PNGs.
    let saved_revision = |last_scene: &[u8]| -> Result<String> {
        Ok(digest(&serde_json::to_vec(&(
            source.document,
            [
                (source.page, Some(digest(b"first saved scene"))),
                (blank, None),
                (last, Some(digest(last_scene))),
            ],
            digest(b"fixture layout and template assets"),
        ))?))
    };
    let receipt = NotebookSourceReceipt {
        saved_source_revision: saved_revision(b"last saved scene")?,
        ordered_image_sha256: images.image_digest()?,
    };
    let mut revised_source = source.clone();
    revised_source.content_revision = Some(saved_revision(b"edited last saved scene")?);
    source.content_revision = Some(receipt.saved_source_revision.clone());
    assert_ne!(receipt.saved_source_revision, receipt.ordered_image_sha256);
    let config = Config::default();
    let mut dispatch = Dispatch::local(&config, &ledger)?;
    let mut observe = || Ok(source.clone());
    let request = "Make a note from the whole notebook.\nKeep  the examples.";
    let ready = scope(&ledger, conversation, &source)?;
    let mut model = Model::default();

    let invalid_document = OrderedNotebookImages {
        document: Uuid::nil(),
        ..images
    };
    let Err(error) =
        invalid_document.admit_originals(source.page, &invalid_document.image_digest()?)
    else {
        anyhow::bail!("missing document identity was accepted");
    };
    assert_eq!(error.to_string(), "invalid notebook identity");
    assert!(model.seen.is_empty());

    let edited_png = fixture_png(19)?;
    let substituted = [originals[0], originals[1], (last, edited_png.as_slice())];
    for (wrong, expected) in [
        (&originals[..2], "complete ordered notebook source required"),
        (
            &[originals[2], originals[1], originals[0]],
            "original notebook page order changed",
        ),
        (
            &substituted,
            "notebook images differ from acquisition receipt",
        ),
    ] {
        let images = OrderedNotebookImages {
            document: source.document,
            order: &order,
            originals: wrong,
            buddy_pages: &buddy_pages,
            patterned_paper: &no_patterns,
        };
        let Err(error) = dispatch.whole_notebook(
            &ledger,
            &mut model,
            request,
            &images,
            &receipt,
            "fixture",
            &ready,
            &mut observe,
        ) else {
            anyhow::bail!("invalid notebook images were accepted");
        };
        assert_eq!(error.to_string(), expected);
        assert!(model.seen.is_empty());
        assert_eq!(
            ledger.expected(Namespace::Conversation, conversation)?,
            ready.root
        );
    }

    let revised_scope = scope(&ledger, conversation, &revised_source)?;
    let Err(error) = dispatch.whole_notebook(
        &ledger,
        &mut model,
        request,
        &images,
        &receipt,
        "fixture",
        &revised_scope,
        &mut || Ok(revised_source.clone()),
    ) else {
        anyhow::bail!("stale acquisition was accepted");
    };
    assert_eq!(
        error.to_string(),
        "notebook saved source differs from acquisition receipt"
    );
    assert!(model.seen.is_empty());
    assert_eq!(
        ledger.expected(Namespace::Conversation, conversation)?,
        ready.root
    );

    let transcript = json!({"pages": [
        {"page": 1, "status": "readable", "writing": "", "visual_observation": "Three strokes form a triangle."},
        {"page": 2, "status": "blank", "writing": "", "visual_observation": ""},
        {"page": 4, "status": "readable", "writing": "L", "visual_observation": "A triangle sits left of the letter."}
    ]});
    let unavailable = scope(&ledger, conversation, &source)?;
    for (status, expected) in [
        ("unreadable", "notebook page unreadable"),
        ("blank", "marked notebook page reported blank"),
    ] {
        let mut rejected = transcript.clone();
        rejected["pages"][0] =
            json!({"page":1, "status":status, "writing":"", "visual_observation":""});
        model.replies.push_back(rejected.to_string());
        let Err(error) = dispatch.whole_notebook(
            &ledger,
            &mut model,
            request,
            &images,
            &receipt,
            "fixture",
            &unavailable,
            &mut observe,
        ) else {
            anyhow::bail!("invalid page evidence reached planning");
        };
        assert_eq!(error.to_string(), expected);
        assert_eq!(model.seen.len(), 1);
        assert_eq!(
            ledger.expected(Namespace::Conversation, conversation)?,
            unavailable.root
        );
        model.seen.clear();
    }

    let map_text =
        "Page 1 depicts a triangle. Page 4 depicts a triangle beside the written letter L.";
    let draft_text = "# Study note\n\nPage 1 depicts a triangle. Page 4 shows a triangle next to the written label L.";
    let mut navigated_source = source.clone();
    navigated_source.page = last;
    assert_eq!(navigated_source.content_revision, source.content_revision);
    assert_eq!(navigated_source.order_revision, source.order_revision);
    for observation_fails in [false, true] {
        let interrupted_conversation = Uuid::new_v4();
        ledger.create(interrupted_conversation, Uuid::new_v4(), 1)?;
        let interrupted = scope(&ledger, interrupted_conversation, &source)?;
        let streaming = std::cell::Cell::new(false);
        let mut engine = InterruptedNotebookStream {
            model: Model {
                replies: [
                    transcript.to_string(),
                    "Map response".into(),
                    draft_text.into(),
                ]
                .into(),
                ..Model::default()
            },
            streaming: &streaming,
        };
        let (stopped, pending) = dispatch.whole_notebook(
            &ledger,
            &mut engine,
            request,
            &images,
            &receipt,
            "fixture",
            &interrupted,
            &mut || {
                if streaming.get() {
                    ensure!(!observation_fails, "simulated live observation error");
                    return Ok(navigated_source.clone());
                }
                Ok(source.clone())
            },
        )?;
        assert_eq!(stopped.status, Status::Canceled);
        assert_eq!(stopped.local_invocations, 1);
        assert_eq!(engine.model.seen.len(), 1);
        assert!(pending.is_none() && stopped.draft.is_none());
    }

    for current_source in [&revised_source, &navigated_source] {
        let interrupted_conversation = Uuid::new_v4();
        ledger.create(interrupted_conversation, Uuid::new_v4(), 1)?;
        let interrupted = scope(&ledger, interrupted_conversation, &source)?;
        let reducer = NotebookStage::identity(interrupted.operation, NotebookStageKind::Reduce, 0);
        let mut changed = false;
        let mut observe_until_publication = || {
            if let Some(envelope) = ledger.store().value(Namespace::Conversation, reducer)? {
                let Record::NotebookStage(stage) = Ledger::decode(&envelope)? else {
                    anyhow::bail!("expected reducer stage");
                };
                if stage.outcome == Some(NotebookStageOutcome::Generated) {
                    changed = true;
                    return Ok(current_source.clone());
                }
            }
            Ok(source.clone())
        };
        let mut interrupted_model = Model::default();
        interrupted_model.replies.extend([
            transcript.to_string(),
            "Ordered source coverage from pages 1 and 4.".into(),
            draft_text.into(),
        ]);
        let (stopped, pending) = dispatch.whole_notebook(
            &ledger,
            &mut interrupted_model,
            request,
            &images,
            &receipt,
            "fixture",
            &interrupted,
            &mut observe_until_publication,
        )?;
        assert!(changed);
        assert_eq!(interrupted_model.seen.len(), 3);
        assert_eq!(stopped.status, Status::Canceled);
        assert!(pending.is_none() && stopped.draft.is_none());
        assert!(ledger
            .store()
            .value(
                Namespace::Conversation,
                writer::stage(interrupted.operation, "draft"),
            )?
            .is_none());
    }

    model
        .replies
        .extend([transcript.to_string(), map_text.into(), draft_text.into()]);
    let (report, output) = dispatch.whole_notebook(
        &ledger,
        &mut model,
        request,
        &images,
        &receipt,
        "fixture",
        &ready,
        &mut observe,
    )?;
    assert_eq!(report.status, Status::Generated);
    assert_eq!(model.seen.len(), 3);
    let map_parts = &model.seen[1].0;
    assert!(map_parts[0].contains("unverified model observations"));
    let map: Value = serde_json::from_str(map_parts.last().unwrap())?;
    let segments = map["chunk"]["authoritative"].as_array().unwrap();
    let values = segments
        .iter()
        .map(|segment| {
            (
                segment["page"].as_u64(),
                segment["section"].as_str(),
                segment["text"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            (
                Some(1),
                Some(VISUAL_SECTION),
                "Three strokes form a triangle."
            ),
            (Some(2), None, ""),
            (Some(4), None, "L"),
            (
                Some(4),
                Some(VISUAL_SECTION),
                "A triangle sits left of the letter."
            ),
        ]
    );
    let reduce_parts = &model.seen[2].0;
    assert!(reduce_parts[0].contains("separately from the user's transcribed words"));
    assert!(reduce_parts.iter().any(|part| part.contains(map_text)));
    assert_eq!(
        model.seen[0]
            .1
            .iter()
            .map(|s| STANDARD.decode(s))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        [first_png, blank_png, last_png]
    );
    let draft = ledger.writer_turn(
        report
            .draft
            .ok_or_else(|| anyhow::anyhow!("notebook draft absent"))?,
        conversation,
    )?;
    let user = ledger.writer_turn(writer::stage(ready.operation, "user"), conversation)?;
    assert_eq!(user.text.as_deref(), Some(request));
    let envelope = ledger
        .store()
        .value(Namespace::Source, draft.sources[0])?
        .ok_or_else(|| anyhow::anyhow!("notebook archive absent"))?;
    let Record::ArchivedNormalizedSource(archive) = Ledger::decode(&envelope)? else {
        anyhow::bail!("notebook archive absent");
    };
    let normalized = archive.load(ledger.store())?;
    assert_eq!(
        normalized.document().source_sha256,
        receipt.ordered_image_sha256
    );
    assert_eq!(
        normalized
            .document()
            .units
            .iter()
            .map(|unit| (unit.page, unit.section.as_deref(), unit.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                Some(1),
                Some(VISUAL_SECTION),
                "Three strokes form a triangle."
            ),
            (Some(2), None, ""),
            (Some(4), None, "L"),
            (
                Some(4),
                Some(VISUAL_SECTION),
                "A triangle sits left of the letter."
            )
        ]
    );
    let visible = scope(&ledger, conversation, &source)?;
    dispatch.record_output(
        &ledger,
        &visible,
        &output.ok_or_else(|| anyhow::anyhow!("notebook output witness absent"))?,
        crate::conversation::CompletionEvidence {
            native_operation: visible.operation,
            exact_text: draft_text.into(),
            observation: source.clone(),
            procedure: "simulated-notebook-writer-readback".into(),
        },
        &mut observe,
    )?;
    let visible_turn =
        ledger.writer_turn(writer::stage(draft.id, "visible-output"), conversation)?;
    assert_eq!(visible_turn.outcome, Outcome::Completed);
    assert_eq!(visible_turn.text.as_deref(), Some(draft_text));
    assert_eq!(
        ledger.writer_turn(draft.id, conversation)?.outcome,
        Outcome::Generated
    );
    Ok(())
}

#[test]
fn whole_notebook_writer_batches_ocr_before_planning_one_complete_draft() -> Result<()> {
    use crate::{
        conversation::notebook::NotebookPlan,
        retrieval::notebook::{NotebookSourceReceipt, OrderedNotebookImages, PatternedPaper},
        writer::notebook::runner::Status,
    };

    let fixture = SimulatorLedger::new();
    let ledger = fixture.open_targets()?;
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let mut source = fixture_source(Uuid::new_v4());
    let originals = [
        source.page,
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let [first, blank, third, fourth, fifth] = originals;
    let buddy = Uuid::new_v4();
    let order = [first, blank, buddy, third, fourth, fifth];
    let pngs = [5, 255, 12, 18, 25].map(fixture_png);
    let pngs = pngs.into_iter().collect::<Result<Vec<_>>>()?;
    let original_images: Vec<_> = originals
        .into_iter()
        .zip(pngs.iter().map(Vec::as_slice))
        .collect();
    let images = OrderedNotebookImages {
        document: source.document,
        order: &order,
        originals: &original_images,
        buddy_pages: &std::collections::BTreeSet::from([buddy]),
        patterned_paper: &std::collections::BTreeMap::from([
            (blank, PatternedPaper::PaperOnly),
            (fifth, PatternedPaper::Ink),
        ]),
    };
    let receipt = NotebookSourceReceipt {
        saved_source_revision: digest(b"independently observed fixture notebook"),
        ordered_image_sha256: images.image_digest()?,
    };
    source.order_revision = Some(images.order_revision()?);
    source.content_revision = Some(receipt.saved_source_revision.clone());
    let config = Config::default();
    let dispatch = Dispatch::local(&config, &ledger)?;
    let request = "Make one study note from this notebook.";

    let leading_notes = "First-page example. ".repeat(1_000);
    let trailing_notes = "Last-page example. ".repeat(1_000);
    let first_batch = json!({"pages": [
        {"page":1, "status":"readable", "writing":leading_notes, "visual_observation":""},
        {"page":2, "status":"blank", "writing":"", "visual_observation":""},
        {"page":4, "status":"readable", "writing":"Third original.", "visual_observation":""},
        {"page":5, "status":"readable", "writing":"Fourth original.", "visual_observation":""}
    ]});
    let later_unreadable = json!({"pages": [{"page":6, "status":"unreadable", "writing":"", "visual_observation":""}]});
    let refused = scope(&ledger, conversation, &source)?;
    let mut blocked = Model {
        replies: [first_batch.to_string(), later_unreadable.to_string()].into(),
        ..Model::default()
    };
    assert!(dispatch
        .whole_notebook(
            &ledger,
            &mut blocked,
            request,
            &images,
            &receipt,
            "fixture",
            &refused,
            &mut || Ok(source.clone()),
        )
        .is_err());
    assert_eq!(blocked.seen.len(), 2);
    assert_eq!(
        ledger.expected(Namespace::Conversation, conversation)?,
        refused.root
    );
    assert!(ledger
        .store()
        .value(
            Namespace::Conversation,
            NotebookPlan::identity(refused.operation)
        )?
        .is_none());

    let final_batch = json!({"pages": [{"page":6, "status":"readable", "writing":trailing_notes, "visual_observation":""}]});
    let mut model = Model {
        replies: [
            first_batch.to_string(),
            final_batch.to_string(),
            "First map summary".into(),
            "Second map summary".into(),
            "# One notebook note\n\nExamples from the entire notebook.".into(),
        ]
        .into(),
        ..Model::default()
    };
    let ready = scope(&ledger, conversation, &source)?;
    let (report, output) = dispatch.whole_notebook(
        &ledger,
        &mut model,
        request,
        &images,
        &receipt,
        "fixture",
        &ready,
        &mut || Ok(source.clone()),
    )?;
    assert_eq!(report.status, Status::Generated);
    assert_eq!(report.local_invocations, 3);
    assert!(output.is_some());
    assert_eq!(
        model
            .seen
            .iter()
            .map(|(_, attachments)| attachments.len())
            .collect::<Vec<_>>(),
        [4, 1, 0, 0, 0]
    );
    for (batch, pattern) in [json!({"2":"paper-only"}), json!({"6":"ink"})]
        .into_iter()
        .enumerate()
    {
        let manifest: Value = serde_json::from_str(&model.seen[batch].0[1])?;
        assert_eq!(manifest["patterned_paper"], pattern);
    }
    let encoded: Vec<_> = model
        .seen
        .iter()
        .take(2)
        .flat_map(|(_, images)| images)
        .collect();
    for (attachment, expected) in encoded.into_iter().zip(&pngs) {
        assert_eq!(STANDARD.decode(attachment)?, *expected);
    }
    let draft = ledger.writer_turn(report.draft.unwrap(), conversation)?;
    let archive = ledger
        .store()
        .value(Namespace::Source, draft.sources[0])?
        .unwrap();
    let Record::ArchivedNormalizedSource(archive) = Ledger::decode(&archive)? else {
        anyhow::bail!("notebook archive absent");
    };
    let normalized = archive.load(ledger.store())?;
    let document = normalized.document();
    assert_eq!(document.source_sha256, receipt.ordered_image_sha256);
    assert_eq!(
        document
            .units
            .iter()
            .map(|unit| (unit.page, unit.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (Some(1), leading_notes.as_str()),
            (Some(2), ""),
            (Some(4), "Third original."),
            (Some(5), "Fourth original."),
            (Some(6), trailing_notes.as_str()),
        ]
    );
    Ok(())
}

#[test]
fn whole_notebook_writer_uses_confirmed_handwriting_only_in_ocr_batches() -> Result<()> {
    use crate::{
        handwriting::{self, LocalCorrections, Provider},
        retrieval::notebook::{
            NotebookSourceReceipt, OrderedNotebookImages, PatternedPaper, WRITER_VISUAL_REVISION,
        },
    };

    let fixture = SimulatorLedger::new();
    let ledger = fixture.open_targets()?;
    let source = fixture_source(Uuid::new_v4());
    let order = [
        source.page,
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let pngs = [12, 255, 36, 48, 60]
        .map(fixture_png)
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let originals: Vec<_> = order
        .iter()
        .copied()
        .zip(pngs.iter().map(Vec::as_slice))
        .collect();
    let run = |config: &Config,
               patterned_paper: &std::collections::BTreeMap<_, _>|
     -> Result<(Model, String)> {
        let mut source = source.clone();
        let images = OrderedNotebookImages {
            document: source.document,
            order: &order,
            originals: &originals,
            buddy_pages: &Default::default(),
            patterned_paper,
        };
        let receipt = NotebookSourceReceipt {
            saved_source_revision: digest(b"sealed notebook"),
            ordered_image_sha256: images.image_digest()?,
        };
        source.content_revision = Some(receipt.saved_source_revision.clone());
        source.order_revision = Some(images.order_revision()?);
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let request = scope(&ledger, conversation, &source)?;
        let mut pages: Vec<_> = (1..=5).map(|page| json!({
            "page":page, "status":"readable", "writing":format!("Target {page}"), "visual_observation":""
        })).collect();
        pages[1] = json!({"page":2, "status":"blank", "writing":"", "visual_observation":""});
        let mut model = Model {
            replies: [
                json!({"pages":&pages[..4]}).to_string(),
                json!({"pages":&pages[4..]}).to_string(),
                "Map of the five originals".into(),
                "# Notes\n\nFive original pages.".into(),
            ]
            .into(),
            ..Model::default()
        };
        let (report, _) = Dispatch::local(config, &ledger)?.whole_notebook(
            &ledger,
            &mut model,
            "Make a note from the whole notebook.",
            &images,
            &receipt,
            "fixture",
            &request,
            &mut || Ok(source.clone()),
        )?;
        let draft =
            ledger.writer_turn(report.draft.expect("Writer produced a draft"), conversation)?;
        let record = ledger
            .store()
            .value(Namespace::Source, draft.sources[0])?
            .unwrap();
        let Record::ArchivedNormalizedSource(archive) = Ledger::decode(&record)? else {
            anyhow::bail!("notebook archive absent");
        };
        assert_eq!(archive.source_sha256, receipt.ordered_image_sha256);
        let notebook = archive.load(ledger.store())?;
        let blank = &notebook.document().units[1];
        assert_eq!((blank.page, blank.text.as_str()), (Some(2), ""));
        Ok((model, archive.extractor_revision))
    };

    let mut config = Config {
        paths: ledger.store().paths.clone(),
        ..Config::default()
    };
    let plain_paper = Default::default();
    let (plain, plain_revision) = run(&config, &plain_paper)?;
    assert_eq!(
        plain_revision,
        format!("{WRITER_VISUAL_REVISION}{}", digest(b"fixture"))
    );
    let profile = Uuid::new_v4();
    config.handwriting = handwriting::Configuration {
        enabled: true,
        profile: Some(profile),
    };
    let confirm = |image: &[u8], correction: &str| {
        let command = json!({
            "handwriting_version":1, "command":"confirm", "profile":profile,
            "correction":Uuid::new_v4(), "operation":Uuid::new_v4(), "expected_heads":[],
            "confirmation":"user-confirmed", "original_text":"Earlier reading",
            "corrected_text":correction, "confirmed_ms":1,
            "provenance":{"kind":"caller-confirmed-sample","description":"Synthetic notebook handwriting","source":null},
            "sample_png_base64":STANDARD.encode(image),
        });
        let input = handwriting::parse(&mut Cursor::new(command.to_string().into_bytes())).unwrap();
        let result = LocalCorrections::new(ledger.store().clone(), &config)
            .unwrap()
            .execute(input);
        assert_eq!(result.status, handwriting::Status::Complete);
    };
    // The last original is outside the first batch. Neither batch may use it as an example.
    confirm(&pngs[4], "Target must stay excluded");
    let (empty, empty_revision) = run(&config, &plain_paper)?;
    assert_eq!(empty.seen[..2], plain.seen[..2]);
    assert_eq!(empty_revision, plain_revision);

    let reference = fixture_png(196)?;
    let correction = "Reference handwriting only";
    confirm(&reference, correction);
    let (personalized, revision) = run(&config, &plain_paper)?;
    assert_ne!(revision, plain_revision);
    assert!(revision.starts_with(WRITER_VISUAL_REVISION));
    assert_eq!(personalized.seen.len(), 4);
    for (i, (texts, attachments)) in personalized.seen[..2].iter().enumerate() {
        assert!(texts.iter().any(|text| text.contains(correction)));
        assert!(texts
            .iter()
            .all(|text| !text.contains("Target must stay excluded")));
        assert_eq!(attachments[0], STANDARD.encode(&reference));
        assert_eq!(attachments[1..], plain.seen[i].1);
    }
    for (texts, attachments) in &personalized.seen[2..] {
        assert!(attachments.is_empty());
        assert!(texts.iter().all(|text| !text.contains(correction)));
    }
    let paper = std::collections::BTreeMap::from([
        (order[1], PatternedPaper::PaperOnly),
        (order[4], PatternedPaper::Ink),
    ]);
    let (patterned, patterned_revision) = run(&config, &paper)?;
    let profile_digest = revision.strip_prefix(WRITER_VISUAL_REVISION).unwrap();
    assert_eq!(
        patterned_revision,
        format!("{WRITER_VISUAL_REVISION}stock-paper-ink-only-v2:{profile_digest}")
    );
    for (batch, expected) in [json!({"2":"paper-only"}), json!({"5":"ink"})]
        .into_iter()
        .enumerate()
    {
        let (texts, attachments) = &patterned.seen[batch];
        let manifest = texts
            .iter()
            .filter_map(|text| serde_json::from_str::<Value>(text).ok())
            .find(|value| value["protocol"] == "writer-notebook-page-evidence-v1")
            .expect("missing original-page manifest");
        assert_eq!(manifest["patterned_paper"], expected);
        assert!(texts.iter().any(|text| text.contains(correction)));
        assert!(texts
            .iter()
            .all(|text| !text.contains("Target must stay excluded")));
        assert_eq!(attachments, &personalized.seen[batch].1);
    }
    for (texts, attachments) in &patterned.seen[2..] {
        assert!(attachments.is_empty());
        assert!(texts.iter().all(|text| !text.contains(correction)));
    }
    Ok(())
}

#[test]
fn maintained_combined_fixture_runs() {
    let scenario = serde_json::from_str(include_str!(
        "../../docs/simulator/scenarios/combined-reader-writer-reader.json"
    ))
    .unwrap();
    run_notebook(&scenario, false).unwrap();
    run(scenario).unwrap();
}

fn run_notebook(scenario: &Scenario, preference_enabled: bool) -> Result<usize> {
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
    let (attachments, associations) = seed_shared_images(&ledger, conversation, &source)?;
    let config: Config = serde_json::from_value(
        json!({"personalization":{"enabled":preference_enabled,"text":scenario.preference}}),
    )?;
    let mut dispatch = Dispatch::local(&config, &ledger)?;
    let admitted = scope(&ledger, conversation, &source)?;
    let raw = serde_json::to_vec(&scenario.document)?;
    let sha256 = digest(&raw);
    let frozen = ValidatedNotebook::from_bytes(&raw, &sha256)?;
    let guidance = "Keep each equation exactly as written.";
    let request = Request::GenerateNotebook(Box::new(Generate {
        conversation,
        operation: admitted.operation,
        request: scenario.requests[1].clone(),
        document_file: fixture.0.join("unused-owner-frozen-input.json"),
        document_sha256: sha256,
        selection: crate::conversation::TextScope::SuppliedNotebook,
        guidance: Some(guidance.into()),
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
        model.seen[0].1.is_empty() && model.seen[1].1 == attachments,
        "notebook stage attachments changed"
    );
    let preference_part =
        crate::config::personalization::receiving_prompt_part(&scenario.preference);
    for (parts, _) in &model.seen {
        if preference_enabled {
            ensure!(
                parts.len() == 3
                    && parts[0]
                        .contains("explicit_guidance take precedence over the saved preference")
                    && parts[1] == preference_part,
                "notebook stage lost the precedence of the current guidance"
            );
        } else {
            ensure!(
                parts.len() == 2 && !parts[0].contains("over the saved preference"),
                "disabled notebook preference changed the stage instructions"
            );
        }
    }
    let reduce: Value = serde_json::from_str(
        model.seen[1]
            .0
            .last()
            .ok_or_else(|| anyhow::anyhow!("reducer payload absent"))?,
    )?;
    let map: Value = serde_json::from_str(
        model.seen[0]
            .0
            .last()
            .ok_or_else(|| anyhow::anyhow!("map payload absent"))?,
    )?;
    for frame in [&map, &reduce] {
        ensure!(
            frame["request"] == scenario.requests[1] && frame["explicit_guidance"] == guidance,
            "notebook stage changed the current request or its guidance"
        );
    }
    ensure!(
        map.get("historical_images").is_none(),
        "map received image associations"
    );
    use crate::conversation::notebook::{notebook_input_digest, NotebookStageKind, REDUCE_PROMPT};
    let parts = &model.seen[1].0;
    let input_digest = notebook_input_digest(REDUCE_PROMPT, parts)?;
    ensure!(ledger.inspect(conversation, false)?.iter().any(|record| matches!(record,
        Record::NotebookStage(stage) if stage.kind == NotebookStageKind::Reduce && stage.input_digest == input_digest
    )), "reducer did not persist its exact input digest");
    let mut reordered = reduce.clone();
    reordered["historical_images"]
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("image associations absent"))?
        .swap(0, 1);
    let mut changed_parts = parts.clone();
    *changed_parts
        .last_mut()
        .ok_or_else(|| anyhow::anyhow!("reducer payload absent"))? =
        serde_json::to_string(&reordered)?;
    ensure!(
        notebook_input_digest(REDUCE_PROMPT, &changed_parts)? != input_digest,
        "reducer digest did not bind association order"
    );
    let mut reassigned = reduce.clone();
    reassigned["historical_images"][2]["attachment_index"] = json!(1);
    *changed_parts
        .last_mut()
        .ok_or_else(|| anyhow::anyhow!("reducer payload absent"))? =
        serde_json::to_string(&reassigned)?;
    ensure!(
        notebook_input_digest(REDUCE_PROMPT, &changed_parts)? != input_digest,
        "reducer digest did not bind attachment assignment"
    );
    ensure!(
        reduce["history"][0]["text"] == "Earlier image question?"
            && reduce["historical_images"] == associations
            && reduce["historical_png_base64_sha256"]
                == json!(attachments
                    .iter()
                    .map(|image| digest(image.as_bytes()))
                    .collect::<Vec<_>>()),
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
        reader["history"][3]["text"] == response
            && reader["historical_images"] == associations
            && model.seen[2].1 == attachments,
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
            let (_, image) = seed_image(&ledger, conversation, &source, tone)?;
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
