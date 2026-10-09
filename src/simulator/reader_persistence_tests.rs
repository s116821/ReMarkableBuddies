use super::*;
use crate::{
    conversation::*,
    storage::{Fault as StorageFault, Namespace, Store},
};
use std::{collections::BTreeMap, sync::Arc};

type Calls = Rc<RefCell<Vec<Vec<Vec<u8>>>>>;
struct RecordingModel {
    images: Vec<String>,
    calls: Calls,
    store: Arc<Store>,
    replies: VecDeque<String>,
    fail: bool,
    fault_after_second: bool,
    cancel_after_first: Option<Shared>,
}
impl LLMEngine for RecordingModel {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.into());
    }
    fn clear_content(&mut self) {
        self.images.clear();
    }
    fn execute(&mut self) -> Result<String> {
        let ledger = Ledger::new(self.store.clone());
        let records = records(&ledger);
        let stored = records
            .iter()
            .find_map(|r| match r {
                Record::LegacyCapture(e) => {
                    Some(ledger.stored_legacy_images(e.id).map(|v| v.images))
                }
                Record::DevelopmentCapture(e) => {
                    Some(ledger.stored_development_images(e.id).map(|v| v.images))
                }
                Record::Capture(e) => Some(
                    ledger
                        .stored_sdk_images(e.id)
                        .map(|v| v.images.into_iter().map(|i| i.bytes).collect()),
                ),
                _ => None,
            })
            .context("no prepared evidence before provider")??;
        let actual = self
            .images
            .iter()
            .map(|i| STANDARD.decode(i).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;
        ensure!(actual == stored, "provider bytes differ from storage");
        self.calls.borrow_mut().push(actual);
        if self.fault_after_second && self.calls.borrow().len() == 2 {
            self.store.set_fault(StorageFault::BeforeCommit)?;
        }
        ensure!(!self.fail, "provider failed");
        if self.calls.borrow().len() == 1 {
            if let Some(state) = &self.cancel_after_first {
                state.borrow_mut().request_lost = true;
            }
        }
        self.replies.pop_front().context("model replies exhausted")
    }
}
fn records(ledger: &Ledger) -> Vec<Record> {
    ledger
        .store()
        .snapshot_heads_matching(&[Namespace::Conversation, Namespace::Source], 100, |_| true)
        .unwrap()
        .into_iter()
        .filter_map(|e| serde_json::from_value(e.payload).ok())
        .collect()
}
fn setup() -> Shared {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    let scenario: Scenario =
        serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap()).unwrap();
    let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
    state.borrow_mut().refuse_repeated_details = true;
    state
}
fn model(store: Arc<Store>, calls: Calls) -> RecordingModel {
    RecordingModel {images:vec![],calls,store,replies:VecDeque::from([
        "QUESTION: Why?\nQUESTION_BOX: 20,20,30,30\nSELECTION_CENTER: 300,400\n---\nANSWER: Because.".into(),
        "TRANSCRIPTION: Why?".into()]),fail:false,fault_after_second:false,cancel_after_first:None}
}
#[test]
fn no_output_disagreement_cancellation_and_device_uncertainty_remain_explicit() {
    for case in 0..5 {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let state = setup();
        let calls = Rc::new(RefCell::new(vec![]));
        let mut model = model(store.clone(), calls.clone());
        let expected = match case {
            0 => {
                state.borrow_mut().pages.truncate(1);
                AttemptReason::NoSuccessor
            }
            1 => {
                state.borrow_mut().pages[1].background.fill(0);
                AttemptReason::InvalidSuccessor
            }
            2 => {
                state.borrow_mut().faults.push(scenario::Fault {
                    operation: Operation::Text,
                    call: 1,
                    effect: scenario::Effect::Error,
                });
                AttemptReason::DeviceUncertain
            }
            3 => {
                model.replies[1] = "TRANSCRIPTION: Different".into();
                AttemptReason::TranscriptionDisagreement
            }
            _ => {
                model.cancel_after_first = Some(state.clone());
                AttemptReason::Canceled
            }
        };
        let mut orchestrator = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model,
            ledger,
        );
        orchestrator.set_trigger_enabled(false);
        let result = orchestrator.run_iteration();
        assert_eq!(
            result.is_err(),
            matches!(case, 2 | 4),
            "case {case}: {result:?}"
        );
        let recorded = records(&Ledger::new(store.clone()));
        assert!(
            recorded
                .iter()
                .any(|r| matches!(r,Record::OutcomeFact(f) if f.reason==expected)),
            "case{case}"
        );
        assert!(!recorded
            .iter()
            .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Completed)));
        assert_eq!(state.borrow().detail_queries, 1);
        if case >= 3 {
            assert_eq!(
                state
                    .borrow()
                    .counts
                    .get(&Operation::Next)
                    .copied()
                    .unwrap_or(0),
                0
            );
            assert_eq!(
                state
                    .borrow()
                    .counts
                    .get(&Operation::Text)
                    .copied()
                    .unwrap_or(0),
                0
            );
        }
    }
}
#[test]
fn unsupported_or_failed_sdk_capture_never_downgrades_or_dispatches() {
    use crate::device::backend::AcquisitionKind;
    for kind in [AcquisitionKind::Unsupported, AcquisitionKind::Sdk] {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let state = setup();
        state.borrow_mut().acquisition_kind = kind;
        let calls = Rc::new(RefCell::new(vec![]));
        let mut orchestrator = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model(store.clone(), calls.clone()),
            ledger,
        );
        orchestrator.set_trigger_enabled(false);
        assert!(orchestrator.run_iteration().is_err());
        assert!(calls.borrow().is_empty());
        assert!(records(&Ledger::new(store)).is_empty());
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Capture)
                .copied()
                .unwrap_or(0),
            0
        );
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Text)
                .copied()
                .unwrap_or(0),
            0
        );
    }
}
#[test]
fn actual_orchestrator_uses_stored_batch_twice_and_records_unverified_output() {
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open().unwrap();
    let store = ledger.store().clone();
    let state = setup();
    let initial = state.borrow().pages[0].image();
    let calls = Rc::new(RefCell::new(vec![]));
    let mut orchestrator = Orchestrator::new(
        Workflow::with_device(Box::new(SimDevice(state.clone())), false),
        model(store.clone(), calls.clone()),
        ledger,
    );
    orchestrator.set_trigger_enabled(false);
    orchestrator.run_iteration().unwrap();
    assert_eq!(calls.borrow().len(), 2);
    assert_eq!(calls.borrow()[0], calls.borrow()[1]);
    assert_eq!(state.borrow().detail_queries, 1);
    assert_eq!(state.borrow().pages[0].image(), initial);
    let recorded = records(&Ledger::new(store.clone()));
    assert!(recorded.iter().any(|r|matches!(r,Record::Turn(t) if t.role==Role::Assistant && t.outcome==Outcome::ReconcileRequired)));
    assert!(!recorded
        .iter()
        .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Completed)));
    drop(orchestrator);
    drop(store);
    let ledger = fixture.open().unwrap();
    let recorded = records(&ledger);
    assert!(recorded.iter().any(
        |r| matches!(r,Record::OutcomeFact(f) if f.reason==AttemptReason::SubmittedUnverified)
    ));
    // Opening/reconstructing the ledger makes no device/model effects.
    assert_eq!(calls.borrow().len(), 2);
}
#[test]
fn storage_failure_prevents_provider_or_output_and_provider_failure_is_durable() {
    for case in 0..3 {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let state = setup();
        let calls = Rc::new(RefCell::new(vec![]));
        let mut model = model(store.clone(), calls.clone());
        if case == 0 {
            store.set_fault(StorageFault::BeforeCommit).unwrap();
        }
        if case == 1 {
            model.fault_after_second = true;
        }
        if case == 2 {
            model.fail = true;
        }
        let mut orchestrator = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model,
            ledger,
        );
        orchestrator.set_trigger_enabled(false);
        assert!(orchestrator.run_iteration().is_err());
        assert_eq!(calls.borrow().len(), [0, 2, 1][case]);
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Next)
                .copied()
                .unwrap_or(0),
            0
        );
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Text)
                .copied()
                .unwrap_or(0),
            0
        );
        if case == 2 {
            assert!(records(&Ledger::new(store.clone()))
                .iter()
                .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Failed)));
        }
    }
}

fn development_capture_fixture() -> (
    remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture,
    Vec<u8>,
    Vec<u8>,
) {
    use remarkable_open_sdk::development_capture::*;
    let expected = ExpectedCaptureBinding {
        nonce: "0123456789abcdef0123456789abcdef".into(),
        attempt_pid: "42".into(),
        attempt_start: "100".into(),
        root_device: "19".into(),
        root_inode: "200".into(),
        document_id: "00000000-0000-4000-8000-000000000001".into(),
        expected_order: vec!["00000000-0000-4000-8000-000000000002".into()],
    };
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 3)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let png = encoded.into_inner();
    let raw = serde_json::to_vec_pretty(&serde_json::json!({
        "kind":"development-capture-observation", "version":2,
        "nonce":expected.nonce, "attempt_pid":expected.attempt_pid, "attempt_start":expected.attempt_start,
        "root_device":expected.root_device, "root_inode":expected.root_inode, "setup_profile":"main-dev-facts-120s",
        "setup_budget_ms":120000, "capture_budget_ms":5000,
        "accepted_ms":100, "baseline_ms":101, "grab_start_ms":102, "grab_end_ms":103, "post_read_ms":104, "completed_ms":105,
        "document_id":expected.document_id, "page_id":expected.expected_order[0], "page_index":0,
        "begin_epoch":"1", "end_epoch":"1", "width":2, "height":3, "dpr":1,
        "image_width":2, "image_height":3, "png_bytes":png.len(), "png_sha256":crate::storage::digest(&png),
        "image_status":"available", "gui_callback_completed":true, "scope_current":true,
        "atomic_snapshot":false, "native_authority":false, "render_authority":false, "ui_acknowledged":false,
        "observed_order":false, "discovery_scope":DISCOVERY_SCOPE
    })).unwrap();
    let capture =
        ReadOnlyDevelopmentCapture::from_collected_v11(&expected, raw.clone(), png.clone())
            .unwrap();
    (capture, raw, png)
}
fn development_workflow(
    store: Arc<Store>,
    state: Shared,
    capture: remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture,
) -> Workflow {
    use crate::workflow::selected_backend::SelectedBackendFacade;
    let admission = Arc::new(
        SelectedAdmission::new(
            store,
            crate::storage::selection::SelectionScope {
                group: crate::storage::Uuid::new_v4(),
                key_sha256: "a".repeat(64),
                binding_sha256: "b".repeat(64),
            },
        )
        .unwrap(),
    );
    let facade = SelectedBackendFacade::new(admission, Box::new(SimDevice(state)));
    Workflow::with_selected_development_capture(facade, capture, false)
}
#[test]
fn development_owned_transport_persists_before_provider_and_reopens_exact_bytes_without_output() {
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open().unwrap();
    let store = ledger.store().clone();
    let state = setup();
    let source_before = state.borrow().pages[0].image();
    let (capture, completion, png) = development_capture_fixture();
    let calls = Rc::new(RefCell::new(vec![]));
    let mut workflow = development_workflow(store.clone(), state.clone(), capture);
    assert!(workflow.wait_for_trigger().is_err());
    assert!(workflow
        .dispatch_reader(None, 0, &ReaderHandoff::NextPage)
        .is_err());
    let mut orchestrator = Orchestrator::new(workflow, model(store.clone(), calls.clone()), ledger);
    orchestrator.set_trigger_enabled(false);
    orchestrator.run_iteration().unwrap();
    let provider_images =
        crate::device::screenshot::Screenshot::reader_images_from_owned_png(&png).unwrap();
    assert_eq!(
        *calls.borrow(),
        vec![provider_images.clone(), provider_images.clone()]
    );
    assert_eq!(provider_images.len(), 4);
    let overview = image::load_from_memory(&provider_images[0]).unwrap();
    assert_eq!((overview.width(), overview.height()), (768, 1024));
    // A consumed owned capture cannot reacquire or downgrade, and makes no third model call.
    assert!(orchestrator.run_iteration().is_err());
    assert_eq!(calls.borrow().len(), 2);
    let recorded = records(&Ledger::new(store.clone()));
    let evidence = recorded
        .iter()
        .find_map(|r| match r {
            Record::DevelopmentCapture(e) => Some(e.as_ref()),
            _ => None,
        })
        .unwrap()
        .clone();
    assert!(!recorded.iter().any(|r| matches!(
        r,
        Record::LegacyCapture(_) | Record::Capture(_) | Record::Binding(_)
    )));
    assert!(recorded.iter().any(
        |r| matches!(r,Record::OutcomeFact(f) if f.reason == AttemptReason::NativeOutputUnavailable)
    ));
    assert!(!recorded
        .iter()
        .any(|r| matches!(r,Record::OutcomeFact(f) if f.reason == AttemptReason::OutputPending)));
    assert!(!recorded.iter().any(|r| matches!(r,Record::Turn(t) if t.outcome == Outcome::Completed || t.outcome == Outcome::ReconcileRequired)));
    assert_eq!(state.borrow().pages[0].image(), source_before);
    assert!(state.borrow().counts.is_empty());
    drop(orchestrator);
    drop(store);
    let reopened = fixture.open().unwrap();
    let saved = reopened.stored_development_images(evidence.id).unwrap();
    assert_eq!(saved.evidence, evidence);
    assert_eq!(saved.completion, completion);
    assert_eq!(saved.original_png, png);
    assert_eq!(saved.images, provider_images);
    assert!(
        reopened
            .retained_media(evidence.conversation)
            .unwrap()
            .len()
            >= 3
    );
    assert_eq!(calls.borrow().len(), 2);
    assert!(state.borrow().counts.is_empty());
}
#[test]
fn development_preparation_storage_failure_makes_zero_provider_or_backend_calls() {
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open().unwrap();
    let store = ledger.store().clone();
    let state = setup();
    let (capture, _, _) = development_capture_fixture();
    let calls = Rc::new(RefCell::new(vec![]));
    store.set_fault(StorageFault::BeforeCommit).unwrap();
    let mut orchestrator = Orchestrator::new(
        development_workflow(store.clone(), state.clone(), capture),
        model(store.clone(), calls.clone()),
        ledger,
    );
    orchestrator.set_trigger_enabled(false);
    assert!(orchestrator.run_iteration().is_err());
    assert!(calls.borrow().is_empty());
    assert!(state.borrow().counts.is_empty());
    assert!(records(&Ledger::new(store)).is_empty());
}

/// Explicit saved-byte harness only. It never collects from or operates a tablet.
/// Main supplies independently collected bytes and a fresh retained Store path.
#[test]
#[ignore = "requires Main-selected collected development bytes and retained Store path"]
fn development_collected_bytes_actual_orchestrator_retained_store() {
    use crate::conversation::development::HistoricalDevelopmentBinding;
    use remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture;
    let fixture_root = std::path::PathBuf::from(
        std::env::var("SDK_DEVELOPMENT_CAPTURE_FIXTURE").expect("selected fixture folder"),
    )
    .canonicalize()
    .unwrap();
    let requested_store = std::path::PathBuf::from(
        std::env::var("SDK_DEVELOPMENT_CAPTURE_STORE").expect("fresh retained Store path"),
    );
    assert!(
        requested_store.is_absolute() && !requested_store.exists(),
        "Store must be a fresh absolute path"
    );
    let retained = requested_store
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(requested_store.file_name().unwrap());
    assert!(
        !retained.starts_with(&fixture_root),
        "Store must remain outside collected fixture directory"
    );
    let binding: HistoricalDevelopmentBinding =
        serde_json::from_slice(&std::fs::read(fixture_root.join("expected-binding.json")).unwrap())
            .unwrap();
    let completion = std::fs::read(fixture_root.join("capture-observation-complete.json")).unwrap();
    let png = std::fs::read(fixture_root.join("capture-window.png")).unwrap();
    let capture = ReadOnlyDevelopmentCapture::from_collected_v11(
        &binding.expected(),
        completion.clone(),
        png.clone(),
    )
    .unwrap();
    let open = || {
        Ledger::new(Arc::new(
            Store::open(crate::storage::StorePaths {
                data: retained.join("data"),
                cache: retained.join("cache"),
                credentials: retained.join("secrets"),
            })
            .unwrap(),
        ))
    };
    let ledger = open();
    let store = ledger.store().clone();
    let state = setup();
    let calls = Rc::new(RefCell::new(vec![]));
    let mut orchestrator = Orchestrator::new(
        development_workflow(store.clone(), state.clone(), capture),
        model(store.clone(), calls.clone()),
        ledger,
    );
    orchestrator.set_trigger_enabled(false);
    orchestrator.run_iteration().unwrap();
    assert_eq!(calls.borrow().len(), 2);
    assert_eq!(calls.borrow()[0], calls.borrow()[1]);
    assert_eq!(calls.borrow()[0].len(), 4);
    assert!(state.borrow().counts.is_empty());
    let recorded = records(&Ledger::new(store.clone()));
    let evidence = recorded
        .iter()
        .find_map(|r| match r {
            Record::DevelopmentCapture(e) => Some(e.as_ref()),
            _ => None,
        })
        .unwrap()
        .clone();
    assert_eq!(evidence.expected_binding, binding);
    assert!(!recorded.iter().any(|r| matches!(
        r,
        Record::LegacyCapture(_) | Record::Capture(_) | Record::Binding(_)
    )));
    assert!(recorded.iter().any(
        |r| matches!(r,Record::OutcomeFact(f) if f.reason == AttemptReason::NativeOutputUnavailable)
    ));
    assert!(!recorded
        .iter()
        .any(|r| matches!(r,Record::OutcomeFact(f) if f.reason == AttemptReason::OutputPending)));
    let revisions = store
        .snapshot_revisions_matching(&[Namespace::Conversation], 100, |e| {
            e.payload["record"]["conversation"].as_str() == Some(&evidence.conversation.to_string())
        })
        .unwrap();
    assert!(revisions.iter().any(|e| matches!(serde_json::from_value::<Record>(e.payload.clone()), Ok(Record::Turn(t)) if t.role == Role::Assistant && t.outcome == Outcome::Generated)));
    assert!(!recorded.iter().any(|r| matches!(r,Record::Turn(t) if t.outcome == Outcome::Completed || t.outcome == Outcome::ReconcileRequired)));
    // Independent pixel oracle checks prompt space and top/middle/bottom crops.
    let original = image::load_from_memory(&png).unwrap();
    let rgba = image::DynamicImage::ImageRgba8(original.to_rgba8());
    let overview = image::load_from_memory(&calls.borrow()[0][0]).unwrap();
    assert_eq!((overview.width(), overview.height()), (768, 1024));
    assert_eq!(
        overview.to_rgba8(),
        rgba.resize_exact(768, 1024, image::imageops::FilterType::Nearest)
            .to_rgba8()
    );
    let (w, h) = (rgba.width(), rgba.height());
    let th = h * 2 / 5;
    for (ordinal, y) in [0, (h - th) / 2, h - th].into_iter().enumerate() {
        let actual = image::load_from_memory(&calls.borrow()[0][ordinal + 1]).unwrap();
        assert_eq!(actual.to_rgba8(), rgba.crop_imm(0, y, w, th).to_rgba8());
    }
    drop(orchestrator);
    drop(store);
    let reopened = open();
    let saved = reopened.stored_development_images(evidence.id).unwrap();
    assert_eq!(saved.evidence, evidence);
    assert_eq!(saved.completion, completion);
    assert_eq!(saved.original_png, png);
    assert_eq!(saved.images, calls.borrow()[0]);
    assert!(state.borrow().counts.is_empty());
    let report = serde_json::json!({
        "kind":"development-collected-byte-reader-integration", "sdk_source":crate::conversation::development::DEVELOPMENT_SDK_SOURCE,
        "reader_source":option_env!("VERGEN_GIT_SHA"), "collection_nonce":binding.nonce,
        "conversation":evidence.conversation, "evidence":evidence.id,
        "raw_completion_sha256":crate::storage::digest(&completion), "original_png_sha256":crate::storage::digest(&png),
        "original_dimensions":evidence.original_dimensions, "decoded_original_format":format!("{:?}",original.color()),
        "provider_images":evidence.images, "provider_calls":2,
        "provider_bytes_match_durable_store":true, "reopened_exact_bytes":true,
        "generated_before_native_output_unavailable":true, "pending_published":false,
        "backend_entries":0, "historical_byte_import_only":true,
        "live_collection_ownership_verified_by_harness":false, "live_provider":false,
        "native_authority":false, "render_authority":false, "facts_published":false,
        "retained_store":retained.to_string_lossy(),
    });
    let path = retained.join("integration-receipt.json");
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(&report).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    println!("PASS development collected bytes: original PNG+receipt, four durable provider images twice, reopened exact bytes, zero effects; receipt={}",path.display());
}

#[test]
fn selected_development_capture_retrieval_preserves_completion_original_and_derivatives() {
    use crate::storage::selection::{SelectionChange, SelectionScope};
    use crate::storage::{digest, Coverage, Manifest, ObjectRef, Scope, Uuid, FORMAT};
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open().unwrap();
    let store = ledger.store().clone();
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let (capture, completion, png) = development_capture_fixture();
    let turn = Turn {
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
    let original = ledger
        .prepare_development(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            turn,
            Uuid::new_v4(),
            &capture,
        )
        .unwrap();
    let mut objects = BTreeMap::new();
    let mut records = BTreeMap::new();
    let mut media: BTreeMap<String, ObjectRef> = BTreeMap::new();
    let mut namespaces = BTreeMap::new();
    for manifest in store.manifests().unwrap() {
        for reference in manifest.records {
            namespaces.insert(
                reference.sha256.clone(),
                manifest.record_namespaces[&reference.sha256],
            );
            objects.insert(
                reference.sha256.clone(),
                store.read_object(&reference).unwrap(),
            );
            records.insert(reference.sha256.clone(), reference);
        }
        for reference in manifest.media {
            objects.insert(
                reference.sha256.clone(),
                store.read_object(&reference).unwrap(),
            );
            media.insert(reference.sha256.clone(), reference);
        }
    }
    let selected = Manifest {
        format: FORMAT,
        transaction_id: Uuid::new_v4(),
        scope: Scope::SelectedRecords,
        records: records.into_values().collect(),
        record_namespaces: namespaces,
        media_coverage: media
            .keys()
            .map(|sha256| Coverage::Included {
                sha256: sha256.clone(),
            })
            .collect(),
        media: media.into_values().collect(),
    };
    let admission = SelectedAdmission::new(
        store,
        SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"development selected document"),
            binding_sha256: digest(b"historical unqualified"),
        },
    )
    .unwrap();
    let publication = admission
        .initialize(
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"fixture base"),
                selected,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    let SelectedCaptureImages::Development(actual) = admission
        .capture_images(&publication.token, original.evidence.id)
        .unwrap()
    else {
        panic!("development origin lost")
    };
    assert_eq!(actual.evidence, original.evidence);
    assert_eq!(actual.completion, completion);
    assert_eq!(actual.original_png, png);
    assert_eq!(actual.images, original.images);
    assert_eq!(
        actual.evidence.origin,
        crate::conversation::development::DevelopmentOrigin::DevelopmentUnqualified
    );
}
