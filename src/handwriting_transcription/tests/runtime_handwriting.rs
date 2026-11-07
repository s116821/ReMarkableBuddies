//! Real same-Store runtime controls; synthetic replies do not establish learning.
use super::*;
use crate::{
    conversation::{
        ImageInput, ImageUse, Ledger, Mode, Outcome, Record, Role, SourceObservation, Turn,
    },
    handwriting::runtime::Runtime,
    workflow::conversation_dispatch::{
        handwriting::{ConfirmIntent, ImageRef, WriterImageRequest},
        Dispatch, RequestScope,
    },
};

fn mutate(store: &Arc<Store>, config: &Config, value: Value) -> handwriting::Report {
    let input = handwriting::parse(&mut Cursor::new(serde_json::to_vec(&value).unwrap())).unwrap();
    LocalCorrections::new(store.clone(), config)
        .unwrap()
        .execute(input)
}
fn confirmation(correction: Uuid, time: u64, sample: &[u8], heads: Vec<Uuid>) -> Value {
    json!({"handwriting_version":1,"command":if heads.is_empty(){"confirm"}else{"update"},"profile":profile(),"correction":correction,"operation":Uuid::new_v4(),"expected_heads":heads,"confirmation":"user-confirmed","original_text":"original sample","corrected_text":format!("corrected {time}"),"confirmed_ms":time,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic runtime control only","source":null},"sample_png_base64":STANDARD.encode(sample)})
}
fn enable(local: &mut Local) {
    local.config.handwriting = handwriting::Configuration {
        enabled: true,
        profile: Some(profile()),
    };
}
#[test]
fn runtime_recency_updates_forget_target_exclusion_and_real_custody() {
    for ready in [false, true] {
        let mut local = if ready {
            Local::ready_notebook(true)
        } else {
            Local::new()
        };
        enable(&mut local);
        let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
        let ledger = Ledger::new(store.clone());
        let dispatch = Dispatch::local(&local.config, &ledger).unwrap();
        let mut ids = Vec::new();
        let mut revisions = Vec::new();
        for time in 1..=5 {
            let id = Uuid::new_v4();
            let report = mutate(
                &store,
                &local.config,
                confirmation(id, time, &png(), vec![]),
            );
            assert_eq!(report.status, handwriting::Status::Complete);
            ids.push(id);
            revisions.push(report.facts[0].revision);
        }
        let runtime = dispatch.handwriting_runtime(&ledger).unwrap().unwrap();
        let captured = runtime.capture(&store, &BTreeSet::new(), &mut || Ok(()));
        assert!(captured.failure.is_none());
        assert_eq!(
            captured
                .context
                .as_ref()
                .unwrap()
                .identity()
                .examples
                .iter()
                .map(|e| e.correction)
                .collect::<Vec<_>>(),
            vec![ids[4], ids[3], ids[2]]
        );
        assert_eq!(captured.selection.as_ref().unwrap().checked, 3);
        // Local mutation is allowed while captured bytes remain historical; it does not rewrite them.
        let update = mutate(
            &store,
            &local.config,
            confirmation(ids[0], 20, &png(), vec![revisions[0]]),
        );
        assert_eq!(update.status, handwriting::Status::Complete);
        let updated = update.facts[0].revision;
        drop(update);
        let later = runtime.capture(&store, &BTreeSet::new(), &mut || Ok(()));
        assert_eq!(
            later.context.as_ref().unwrap().identity().examples[0].correction,
            ids[0]
        );
        assert_eq!(
            captured.context.as_ref().unwrap().identity().examples[0].correction,
            ids[4]
        );
        drop(mutate(
            &store,
            &local.config,
            json!({"handwriting_version":1,"command":"forget","profile":profile(),"correction":ids[0],"operation":Uuid::new_v4(),"expected_heads":[updated]}),
        ));
        let forgotten = runtime.capture(&store, &BTreeSet::new(), &mut || Ok(()));
        assert!(forgotten
            .context
            .as_ref()
            .unwrap()
            .identity()
            .examples
            .iter()
            .all(|e| e.correction != ids[0]));
        let empty = runtime.capture(&store, &BTreeSet::from([digest(&png())]), &mut || Ok(()));
        assert!(empty.failure.is_none() && empty.context.is_none());
        assert!(empty.selection.as_ref().unwrap().selected.is_empty());
        let mode = store.sync_mode_snapshot().unwrap();
        assert!(store
            .set_sync_mode(
                &mode.token,
                Uuid::new_v4(),
                crate::storage::sync::mode::SyncModeChoice::LocalOnly
            )
            .is_err());
        drop((captured, later, forgotten, empty, runtime, dispatch));
        if ready {
            assert!(store.ordinary_mode_lease(&mode.token).is_ok());
        } else {
            assert!(store
                .set_sync_mode(
                    &mode.token,
                    Uuid::new_v4(),
                    crate::storage::sync::mode::SyncModeChoice::LocalOnly
                )
                .is_ok());
        }
    }
}
#[test]
fn runtime_window_media_skip_ties_and_freeze_to_capture_staleness() {
    let mut local = Local::ready_notebook(false);
    enable(&mut local);
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    let ledger = Ledger::new(store.clone());
    let dispatch = Dispatch::local(&local.config, &ledger).unwrap();
    let good = png();
    let mut bad = good.clone();
    bad.extend_from_slice(b"trailing");
    for time in 1..=17 {
        let report = mutate(
            &store,
            &local.config,
            confirmation(
                Uuid::new_v4(),
                time,
                if time == 1 { &good } else { &bad },
                vec![],
            ),
        );
        assert_eq!(report.status, handwriting::Status::Complete);
    }
    let runtime = dispatch.handwriting_runtime(&ledger).unwrap().unwrap();
    let empty = runtime.capture(&store, &BTreeSet::new(), &mut || Ok(()));
    assert!(empty.failure.is_none() && empty.context.is_none());
    let selection = empty.selection.as_ref().unwrap();
    assert_eq!(
        (
            selection.checked,
            selection.skipped_media,
            selection.unexamined
        ),
        (16, 16, 1)
    );
    drop(empty);
    let id = Uuid::new_v4();
    let created = mutate(
        &store,
        &local.config,
        confirmation(id, u64::MAX, &png(), vec![]),
    );
    let revision = created.facts[0].revision;
    drop(created);
    let mut checkpoints = 0;
    let mut changed = false;
    // One good plus fifteen rejected candidate reads, then the exact capture's first progress.
    let capture = runtime.capture(&store, &BTreeSet::new(), &mut || {
        checkpoints += 1;
        if checkpoints == 20 {
            let report = mutate(
                &store,
                &local.config,
                confirmation(id, u64::MAX, &png(), vec![revision]),
            );
            assert_eq!(report.status, handwriting::Status::Complete);
            changed = true;
        }
        Ok(())
    });
    assert!(changed);
    assert_eq!(capture.failure, Some(handwriting::ContextFailure::Stale));
    assert!(capture.context.is_none());
    assert_eq!(
        capture.selection.as_ref().unwrap().selected[0].revision,
        revision
    );
}
#[test]
fn runtime_foreign_store_disabled_and_cancellation_do_not_capture() {
    let mut local = Local::new();
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    let lifetime = Arc::new(crate::mode_maintenance::admit_local_dispatch(&store).unwrap());
    assert!(Runtime::new(&local.config, store.clone(), lifetime.clone())
        .unwrap()
        .is_none());
    enable(&mut local);
    let runtime = Runtime::new(&local.config, store.clone(), lifetime.clone())
        .unwrap()
        .unwrap();
    let foreign = Local::new();
    let other = Arc::new(Store::open(foreign.config.paths.clone()).unwrap());
    assert_eq!(
        runtime
            .capture(&other, &BTreeSet::new(), &mut || Ok(()))
            .failure,
        Some(handwriting::ContextFailure::Storage)
    );
    assert_eq!(
        runtime
            .capture(&store, &BTreeSet::new(), &mut || Err(
                handwriting::ContextFailure::Canceled
            ))
            .failure,
        Some(handwriting::ContextFailure::Canceled)
    );
}
fn source_fixture(local: &Local, mode: Mode) -> (Ledger, RequestScope, ImageRef) {
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    retained_source_fixture(store, mode, png())
}
fn retained_source_fixture(
    store: Arc<Store>,
    mode: Mode,
    bytes: Vec<u8>,
) -> (Ledger, RequestScope, ImageRef) {
    let ledger = Ledger::new(store);
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let turn = Uuid::new_v4();
    let source = Uuid::new_v4();
    let observation = SourceObservation {
        document: Uuid::new_v4(),
        page: Uuid::new_v4(),
        session: "fixture-session".into(),
        visit: "fixture-visit".into(),
        content_revision: Some("fixture-content".into()),
        order_revision: Some("fixture-order".into()),
        capability_revision: "fixture-capability".into(),
        evidence_procedure: "synthetic receiving fixture, no native authority".into(),
    };
    let decoded = image::load_from_memory(&bytes).unwrap();
    let image = ImageUse {
        id: source,
        conversation,
        turn,
        image: crate::storage::Media {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
            media_type: "image/png".into(),
        },
        observation: observation.clone(),
        width: decoded.width(),
        height: decoded.height(),
        purpose: "inference".into(),
        ordinal: 0,
        parent: None,
        parent_dimensions: None,
        crop: None,
        transform: [1., 0., 0., 1., 0., 0.],
        target: None,
        captured_ms: 1,
    };
    ledger
        .prepare_request(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            Turn {
                id: turn,
                conversation,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::User,
                mode,
                outcome: Outcome::Prepared,
                text: None,
                sources: vec![source],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            },
            vec![ImageInput {
                source: image,
                bytes,
                parent_bytes: None,
            }],
        )
        .unwrap();
    let reference = ImageRef {
        source,
        revision: *ledger
            .expected(Namespace::Source, source)
            .unwrap()
            .0
            .first()
            .unwrap(),
        turn,
        turn_revision: *ledger
            .expected(Namespace::Conversation, turn)
            .unwrap()
            .0
            .first()
            .unwrap(),
        ordinal: 0,
        page: 7,
    };
    let scope = RequestScope {
        conversation,
        operation: Uuid::new_v4(),
        root: ledger
            .expected(Namespace::Conversation, conversation)
            .unwrap(),
        source: observation,
    };
    (ledger, scope, reference)
}
struct Calls {
    parts: Vec<Value>,
    calls: Vec<Vec<Value>>,
    replies: std::collections::VecDeque<String>,
}
mod receiving_driver;
impl LLMEngine for Calls {
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(json!({"type":"text","text":text}));
    }
    fn add_image_content(&mut self, image: &str) {
        self.parts
            .push(json!({"type":"image_url","image_url":image}));
    }
    fn clear_content(&mut self) {
        self.parts.clear();
    }
    fn execute(&mut self) -> anyhow::Result<String> {
        self.calls.push(self.parts.clone());
        self.replies
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("unexpected model call"))
    }
}
#[test]
fn runtime_writer_normalizes_before_receiving_and_replay_never_calls_again() {
    let mut local = Local::ready_notebook(true);
    enable(&mut local);
    // The retained example is distinct from the current target hash.
    let mut example = png();
    example.extend_from_slice(b"x");
    // A valid different static PNG is generated once as synthetic test input.
    example.clear();
    image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut Cursor::new(&mut example), image::ImageFormat::Png)
        .unwrap();
    local.confirm(Uuid::new_v4(), &example, "EXAMPLE_ONLY", "CONFIRMED_ONLY");
    let (ledger, scope, reference) = source_fixture(&local, Mode::Writer);
    let mut dispatch = Dispatch::local(&local.config, &ledger).unwrap();
    let input = WriterImageRequest {
        image: reference.clone(),
        request: "Write a note".into(),
        guidance: None,
        model_profile: "fixture-model".into(),
    };
    let mut engine = Calls {
        parts: vec![],
        calls: vec![],
        replies: std::collections::VecDeque::from([
            good_reply(),
            "# Title\n\nBody from target.".into(),
        ]),
    };
    let report = dispatch.writer_source_image(&ledger, &mut engine, &input, &scope, &mut || {
        Ok(scope.source.clone())
    });
    assert!(report.result.is_ok(), "{:?}", report.result.as_ref().err());
    assert_eq!(engine.calls.len(), 2);
    assert!(engine.calls[0].iter().any(|p| p["text"]
        .as_str()
        .is_some_and(|s| s.contains("CONFIRMED_ONLY"))));
    assert!(!engine.calls[1].iter().any(|p| p["text"]
        .as_str()
        .is_some_and(|s| s.contains("CONFIRMED_ONLY"))));
    let records = ledger.inspect(scope.conversation, false).unwrap();
    assert!(records.iter().any(
        |r| matches!(r,Record::NormalizedText(source) if source.document.units[0].text=="café x²")
    ));
    let mut emitted = Vec::new();
    report.write_to(&mut emitted).unwrap();
    drop(report);
    // Explicit confirmation resolves normalized source text, never the generic Generate request.
    let normalized = records
        .iter()
        .find_map(|r| {
            if let Record::NormalizedText(source) = r {
                Some(source)
            } else {
                None
            }
        })
        .unwrap();
    let source_revision = *ledger
        .expected(Namespace::Source, normalized.id)
        .unwrap()
        .0
        .first()
        .unwrap();
    let confirmation_scope = RequestScope {
        conversation: scope.conversation,
        operation: Uuid::new_v4(),
        root: ledger
            .expected(Namespace::Conversation, scope.conversation)
            .unwrap(),
        source: scope.source.clone(),
    };
    let intent = ConfirmIntent {
        image: reference.clone(),
        corrected_text: "confirmed target reading".into(),
        normalized_source: Some((normalized.id, source_revision)),
    };
    let prepared = dispatch
        .prepare_handwriting_confirmation(&ledger, &intent, &confirmation_scope, &mut || {
            Ok(scope.source.clone())
        })
        .unwrap();
    let confirmed = prepared
        .execute(&ledger, &mut || Ok(scope.source.clone()))
        .unwrap();
    assert_eq!(confirmed.report.status, handwriting::Status::Complete);
    assert_eq!(
        confirmed.report.facts[0]
            .value
            .as_ref()
            .unwrap()
            .original_text,
        "café x²"
    );
    assert_eq!(
        confirmed.report.facts[0]
            .value
            .as_ref()
            .unwrap()
            .provenance
            .source,
        Some([scope.conversation, reference.turn, reference.source])
    );
    let frozen: Value = serde_json::from_slice(confirmed.frozen_input()).unwrap();
    assert_eq!(frozen["operation"], json!(confirmation_scope.operation));
    let mut confirmed_output = Vec::new();
    confirmed.write_to(&mut confirmed_output).unwrap();
    drop(confirmed);
    let replay = dispatch.writer_source_image(&ledger, &mut engine, &input, &scope, &mut || {
        Ok(scope.source.clone())
    });
    assert!(replay.result.is_err());
    assert_eq!(engine.calls.len(), 2);
    assert_eq!(
        ledger
            .store()
            .snapshot_revisions_matching(&[Namespace::Handwriting], 4096, |_| true)
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn runtime_writer_confirmation_refuses_generic_request_without_source_interpretation() {
    let mut local = Local::ready_notebook(false);
    enable(&mut local);
    let (ledger, scope, image) = source_fixture(&local, Mode::Writer);
    let mut dispatch = Dispatch::local(&local.config, &ledger).unwrap();
    let intent = ConfirmIntent {
        image,
        corrected_text: "corrected".into(),
        normalized_source: None,
    };
    assert!(dispatch
        .prepare_handwriting_confirmation(&ledger, &intent, &scope, &mut || Ok(scope
            .source
            .clone()))
        .is_err());
    assert!(ledger
        .store()
        .snapshot_revisions_matching(&[Namespace::Handwriting], 4096, |_| true)
        .unwrap()
        .is_empty());
}

#[test]
fn runtime_writer_unreadable_result_keeps_real_owner_through_output_failure() {
    let mut local = Local::new();
    enable(&mut local);
    let (ledger, scope, image) = source_fixture(&local, Mode::Writer);
    let mut dispatch = Dispatch::local(&local.config, &ledger).unwrap();
    let input = WriterImageRequest {
        image,
        request: "Generate".into(),
        guidance: None,
        model_profile: "fixture".into(),
    };
    let mut engine = Calls {
        parts: vec![],
        calls: vec![],
        replies: std::collections::VecDeque::from([
            json!({"pages":[{"page":7,"status":"unreadable","text":null}]}).to_string(),
        ]),
    };
    let report = dispatch.writer_source_image(&ledger, &mut engine, &input, &scope, &mut || {
        Ok(scope.source.clone())
    });
    assert!(report.result.is_err());
    assert_eq!(engine.calls.len(), 1);
    assert!(engine.parts.is_empty());
    assert!(ledger
        .store()
        .value(
            Namespace::Conversation,
            crate::writer::stage(scope.operation, "source-normalization")
        )
        .unwrap()
        .is_some());
    assert!(ledger
        .store()
        .value(
            Namespace::Conversation,
            crate::writer::stage(scope.operation, "draft")
        )
        .unwrap()
        .is_none());
    drop(dispatch);
    let mode = ledger.store().sync_mode_snapshot().unwrap();
    assert!(ledger
        .store()
        .set_sync_mode(
            &mode.token,
            Uuid::new_v4(),
            crate::storage::sync::mode::SyncModeChoice::LocalOnly
        )
        .is_err());
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("synthetic output failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            panic!("must not flush failed output")
        }
    }
    assert!(report.write_to(&mut Broken).is_err());
    assert!(ledger
        .store()
        .set_sync_mode(
            &mode.token,
            Uuid::new_v4(),
            crate::storage::sync::mode::SyncModeChoice::LocalOnly
        )
        .is_err());
    drop(report);
    assert!(ledger
        .store()
        .set_sync_mode(
            &mode.token,
            Uuid::new_v4(),
            crate::storage::sync::mode::SyncModeChoice::LocalOnly
        )
        .is_ok());
}

#[test]
fn runtime_writer_invalid_intent_and_changed_source_refuse_before_recognition() {
    let mut local = Local::ready_notebook(false);
    enable(&mut local);
    let (ledger, scope, image) = source_fixture(&local, Mode::Writer);
    let mut dispatch = Dispatch::local(&local.config, &ledger).unwrap();
    let mut input = WriterImageRequest {
        image,
        request: String::new(),
        guidance: None,
        model_profile: "fixture".into(),
    };
    let mut engine = Calls {
        parts: vec![],
        calls: vec![],
        replies: std::collections::VecDeque::new(),
    };
    assert!(dispatch
        .writer_source_image(&ledger, &mut engine, &input, &scope, &mut || Ok(scope
            .source
            .clone()))
        .result
        .is_err());
    input.request = "Generate".into();
    input.image.revision = Uuid::new_v4();
    assert!(dispatch
        .writer_source_image(&ledger, &mut engine, &input, &scope, &mut || Ok(scope
            .source
            .clone()))
        .result
        .is_err());
    assert!(engine.calls.is_empty());
    assert_eq!(
        ledger
            .expected(Namespace::Conversation, scope.conversation)
            .unwrap(),
        scope.root
    );
}
