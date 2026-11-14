use super::*;
use std::time::Duration;

struct RefusedComposedMemory;
impl crate::memory::Provider for RefusedComposedMemory {
    fn put(&self, _: crate::memory::Mutation) -> Result<Uuid> {
        panic!("composed memory put called")
    }
    fn forget(&self, _: crate::memory::Forget) -> Result<Uuid> {
        panic!("composed memory forget called")
    }
    fn inspect(&self, _: &crate::memory::Key) -> Result<Vec<crate::storage::Envelope>> {
        panic!("composed memory inspect called")
    }
    fn query(&self, _: &crate::memory::Query) -> Result<crate::memory::QueryResult> {
        panic!("composed memory query called")
    }
}

fn sdk_id(id: Uuid) -> remarkable_open_sdk::Uuid {
    remarkable_open_sdk::Uuid::parse(&id.to_string()).unwrap()
}
fn encode_png(image: &image::DynamicImage) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn composed_sdk_batch(
    device: Uuid,
    document: Uuid,
    page: Uuid,
) -> remarkable_open_sdk::capture::CapturedBatch {
    sdk_batch_with_overview(device, document, page, [2, 3])
}

fn sdk_batch_with_overview(
    device: Uuid,
    document: Uuid,
    page: Uuid,
    overview: [u32; 2],
) -> remarkable_open_sdk::capture::CapturedBatch {
    use remarkable_open_sdk::{
        capture::{synthetic::*, *},
        mock::MockPlatform,
        Platform,
    };
    let source = MockPlatform::new(
        sdk_id(device),
        sdk_id(document),
        vec![sdk_id(page)],
        sdk_id(page),
    )
    .observe_page()
    .unwrap();
    let native = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(4, 6, |x, y| {
        image::Rgba([(x * 40) as u8, (y * 30) as u8, 10, 255])
    }));
    let acquisition = Acquisition {
        before: source.clone(),
        after: source.clone(),
        rendered_owner: source.clone(),
        render_generation: u64::MAX,
        buffer_generation: u64::MAX,
        input_changed: false,
        interval: [
            Duration::new((1u64 << 53) + 1, 123_456_789),
            Duration::new(u64::MAX - 1, 999_999_999),
        ],
        viewport_revision: "fixture-viewport".into(),
        conversion_procedure: "fixture-rgba8".into(),
        procedure_revision: "fixture-generation-check".into(),
        native_png: encode_png(&native),
        native_transform: Affine::new([0.25, -0.0, 0.0, 1.0 / 6.0, -0.0, 0.0]).unwrap(),
        valid_source_region: SourceRegion {
            x: -0.0,
            y: 0.0,
            width: 4.0,
            height: 6.0,
        },
        target: Some([-0.0, 0.5]),
    };
    let request = CaptureRequest::new(
        remarkable_open_sdk::OperationId(sdk_id(Uuid::new_v4())),
        source,
        Duration::new(u64::MAX, 0),
        false,
    );
    let images = vec![
        remarkable_open_sdk::capture::synthetic::ImageInput {
            role: ImageRole::Overview,
            crop: PixelRect {
                x: 0,
                y: 0,
                width: 4,
                height: 6,
            },
            output_dimensions: overview,
            filter: ResizeFilter::Nearest,
            png: encode_png(&native.resize_exact(
                overview[0],
                overview[1],
                image::imageops::FilterType::Nearest,
            )),
        },
        remarkable_open_sdk::capture::synthetic::ImageInput {
            role: ImageRole::Detail(7),
            crop: PixelRect {
                x: 0,
                y: 2,
                width: 4,
                height: 2,
            },
            output_dimensions: [4, 2],
            filter: ResizeFilter::Nearest,
            png: encode_png(&native.crop_imm(0, 2, 4, 2)),
        },
    ];
    validate_batch(&request, acquisition, images, CaptureLimits::default()).unwrap()
}

#[test]
fn marked_question_receipt_retains_exact_original_projection_and_refuses_replay() -> Result<()> {
    use crate::conversation::{marked_question, ContextBudget};
    use crate::workflow::exchange_history::{self, PageOwner};
    for case in [
        "fresh",
        "lost acknowledgment",
        "changed root",
        "changed Prepared",
    ] {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open()?;
        let conversation = Uuid::new_v4();
        let document = Uuid::new_v4();
        let page = Uuid::new_v4();
        let batch = sdk_batch_with_overview(Uuid::new_v4(), document, page, [768, 1024]);
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let mut user = Turn {
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
        let prepared = ledger.prepare_sdk_fixture(
            Uuid::new_v4(),
            ledger.expected(Namespace::Conversation, conversation)?,
            user.clone(),
            &batch,
        )?;
        let envelope = ledger
            .store()
            .value(Namespace::Source, prepared.capture)?
            .unwrap();
        let Record::Capture(capture) = Ledger::decode(&envelope)? else {
            anyhow::bail!("expected SDK capture")
        };
        let source = marked_question::modeled_source(&capture.facts);
        assert_eq!((source.document, source.page), (document, page));
        let buddy = Uuid::new_v4();
        let binding = BindingReceipt {
            acquisition: Default::default(),
            operation: Uuid::new_v4(),
            conversation,
            request_fingerprint: "modeled marked question".into(),
            adapter_fingerprint: "modeled presentation".into(),
            source: source.clone(),
            intended_target: Some(buddy),
            observed_document: document,
            observed_page: buddy,
            expected_order: vec![page],
            observed_order: vec![page, buddy],
            persisted_revision: "modeled binding".into(),
            observed_ms: 1,
            evidence_origin: "synthetic".into(),
        };
        ledger.bind(
            binding.operation,
            ledger.expected(Namespace::Conversation, conversation)?,
            Default::default(),
            binding,
        )?;
        let history = ledger.inspect(conversation, false)?;
        user = history
            .into_iter()
            .find_map(|record| match record {
                Record::Turn(turn) if turn.id == user.id => Some(turn),
                _ => None,
            })
            .unwrap();
        let root = ledger.expected(Namespace::Conversation, conversation)?;
        let captured = ledger.capture_marked_question(conversation)?;
        user.outcome = Outcome::Interpreted;
        user.text = Some("G? unc. here".into());
        user.updated_ms = 2;
        let region = [12, 24, 48, 16];
        let operation = crate::writer::stage(user.exchange, "reader-interpret-marked-question");
        let mut wrong = source.clone();
        wrong.page = Uuid::new_v4();
        assert!(ledger
            .interpret_marked_question(
                operation,
                user.clone(),
                root.clone(),
                &captured,
                prepared.capture,
                &wrong,
                region,
                None
            )
            .is_err());
        if case == "changed root" {
            let mut unrelated = user.clone();
            unrelated.id = Uuid::new_v4();
            unrelated.exchange = Uuid::new_v4();
            unrelated.sequence = 0;
            unrelated.sources.clear();
            ledger.append(Uuid::new_v4(), root.clone(), unrelated, vec![])?;
        } else if case == "changed Prepared" {
            let (old, _) = captured
                .records
                .iter()
                .find(|(envelope, _)| envelope.record_id == user.id)
                .unwrap();
            let mut changed = old.clone();
            changed.parents = [old.revision_id].into_iter().collect();
            changed.revision_id = Uuid::new_v4();
            changed.operation_id = Uuid::new_v4();
            changed.payload["record"]["updated_ms"] = serde_json::json!(2);
            ledger.store().commit(vec![changed], BTreeMap::new())?;
        } else if case == "lost acknowledgment" {
            ledger.store().set_fault(StorageFault::AfterCommit)?;
        }
        let result = ledger.interpret_marked_question(
            operation,
            user.clone(),
            root.clone(),
            &captured,
            prepared.capture,
            &source,
            region,
            None,
        );
        assert_eq!(result.is_ok(), case == "fresh", "{case}");
        assert!(ledger
            .interpret_marked_question(
                operation,
                user.clone(),
                root,
                &captured,
                prepared.capture,
                &source,
                region,
                None
            )
            .is_err());
        let records = ledger.inspect(conversation, false)?;
        let receipts: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                Record::Receipt(receipt) if receipt.marked_question.is_some() => Some(receipt),
                _ => None,
            })
            .collect();
        if case.starts_with("changed") {
            assert!(receipts.is_empty(), "{case} produced a marked receipt");
            continue;
        }
        assert_eq!(receipts.len(), 1);
        let receipt = receipts[0];
        assert_eq!(receipt.acknowledgment.operation, operation);
        let json = serde_json::to_value(receipt)?;
        let marked = &json["marked_question"];
        assert_eq!(marked["region"], serde_json::json!(region));
        assert_eq!(marked["source"], serde_json::to_value(&source)?);
        assert_eq!(
            marked["overview"],
            serde_json::to_value(capture.facts.images[0].media())?
        );
        assert_eq!(
            marked["capture"]["record"],
            serde_json::json!(prepared.capture)
        );
        assert_eq!(
            marked["capture"]["revision"],
            serde_json::json!(envelope.revision_id)
        );
        let interpreted = ledger
            .store()
            .value(Namespace::Conversation, user.id)?
            .unwrap();
        assert_eq!(
            marked["interpreted_turn"]["revision"],
            serde_json::json!(interpreted.revision_id)
        );
        assert_eq!(Ledger::decode(&interpreted)?, Record::Turn(user.clone()));
        let persisted = ledger
            .store()
            .snapshot_heads_matching(&[Namespace::Conversation], 8, |envelope| {
                envelope.payload["record"]["acknowledgment"]["operation"]
                    == serde_json::json!(operation)
            })?
            .pop()
            .unwrap();
        assert_eq!(persisted.domain_schema_version, marked_question::SCHEMA);
        let mut selected = persisted.clone();
        selected.payload["record"]["admitted_intent"] = serde_json::Value::Null;
        assert!(Ledger::decode(&selected).is_err());
        let mut wrong_schema = persisted.clone();
        wrong_schema.domain_schema_version = 1;
        assert!(Ledger::decode(&wrong_schema).is_err());
        let mut wrong_region = persisted.clone();
        wrong_region.payload["record"]["marked_question"]["region"] =
            serde_json::json!([13, 24, 48, 16]);
        assert!(Ledger::decode(&wrong_region).is_err());

        let context = ledger.presentation_context(
            conversation,
            &ContextBudget {
                max_text_bytes: 4096,
                max_turns: 8,
                provider_token_limit: Some(4096),
            },
            |turns| Ok(turns.len()),
        )?;
        assert_eq!(context.turns, vec![user.clone()]);
        let owner = PageOwner {
            document,
            page: buddy,
            visit: "modeled Buddy visit".into(),
            session: source.session.clone(),
        };
        let projection = exchange_history::project(&ledger, conversation, &owner, "")?;
        assert_eq!(projection.exchanges.len(), 1);
        assert_eq!(projection.exchanges[0].turns.len(), 1);
        let presented = &projection.exchanges[0].turns[0];
        assert_eq!(presented.id, user.id);
        assert_eq!(presented.revision, interpreted.revision_id);
        assert_eq!(presented.text, "G? unc. here");
        assert!(records
            .iter()
            .all(|record| !matches!(record, Record::Turn(turn) if turn.role == Role::Assistant)));
    }
    Ok(())
}

#[test]
fn ordinary_modeled_sdk_reader_persists_mark_and_abstains_without_any_effect() {
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open().unwrap();
    let store = ledger.store().clone();
    let state = setup();
    let initial = state.borrow().pages[0].image();
    {
        let mut device = state.borrow_mut();
        device.acquisition_kind = crate::device::backend::AcquisitionKind::Sdk;
        device.sdk_capture = Some(sdk_batch_with_overview(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            [768, 1024],
        ));
    }
    let calls = Rc::new(RefCell::new(vec![]));
    let mut recording = model(store.clone(), calls.clone());
    recording.replies = VecDeque::from([
        "QUESTION: G? unc. here\nQUESTION_BOX: 12,24,48,16\nHANDWRITTEN_QUESTION_MARK: present\n---\nANSWER: NONE".into(),
    ]);
    let mut orchestrator = Orchestrator::new(
        Workflow::with_device(Box::new(SimDevice(state.clone())), false),
        recording,
        ledger,
    );
    orchestrator.set_trigger_enabled(false);
    orchestrator.run_iteration().unwrap();
    assert_eq!(calls.borrow().len(), 1);
    let device = state.borrow();
    assert_eq!(device.pages[0].image(), initial);
    assert!(!device.events.iter().any(|event| event.action == "erase"));
    assert_eq!(device.counts.get(&Operation::Text), None);
    let saved = records(&Ledger::new(store));
    assert!(saved
        .iter()
        .any(|r| matches!(r, Record::Receipt(receipt) if receipt.marked_question.is_some())));
    assert!(!saved
        .iter()
        .any(|r| matches!(r, Record::Turn(t) if t.role == Role::Assistant)));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarkedCase {
    Applied,
    Crossing,
    NoBinding,
    NoPresentation,
    ChangedViewport,
    ChangedPage,
    ChangedSession,
    RenewedVisit,
    RenewedSession,
    ChangedAtAcceptance,
    StaleReceipt,
    LostInterpretation,
    PartialErase,
    LostErase,
    FailedPresentation,
    UncertainPresentation,
    NoMark,
    OutOfFrame,
    DuplicateBox,
    NoAnswerRoute,
    Pair,
    PairAbout,
    PairMemoryRefused,
    PairMemoryRepeated,
    PairEnrichmentFailed,
    PairPartialErase,
    PairLostErase,
    PairRefused,
    PairWrongOperation,
    PairChangedReadback,
    PairLostCompletion,
    PairStaleCompletion,
    PairLostDraft,
    PairStaleDraft,
}

impl MarkedCase {
    fn has_pair(self) -> bool {
        use MarkedCase::*;
        matches!(
            self,
            Pair | PairAbout
                | PairMemoryRefused
                | PairMemoryRepeated
                | PairEnrichmentFailed
                | PairPartialErase
                | PairLostErase
                | PairRefused
                | PairWrongOperation
                | PairChangedReadback
                | PairLostCompletion
                | PairStaleCompletion
                | PairLostDraft
                | PairStaleDraft
        )
    }
}

fn modify_marked_fixture_root(store: &Store, conversation: Uuid) {
    let mut root = store
        .value(Namespace::Conversation, conversation)
        .unwrap()
        .unwrap();
    root.parents = [root.revision_id].into_iter().collect();
    root.revision_id = Uuid::new_v4();
    root.operation_id = Uuid::new_v4();
    store.commit(vec![root], BTreeMap::new()).unwrap();
}

#[test]
fn ordinary_marked_sdk_reader_composes_one_erase_with_the_saved_user_projection() -> Result<()> {
    use super::super::{
        exchange_history::{Case, ModeledPage},
        marked_question::{EraseFault, MarkedQuestion, Stroke},
    };
    use crate::workflow::exchange_history::{
        self, Content, PageOwner, PageState, UserPresentationTarget,
    };
    use image::{Rgba, RgbaImage};
    use MarkedCase::*;

    for case in [
        Applied,
        Crossing,
        NoBinding,
        NoPresentation,
        ChangedViewport,
        ChangedPage,
        ChangedSession,
        RenewedVisit,
        RenewedSession,
        ChangedAtAcceptance,
        StaleReceipt,
        LostInterpretation,
        PartialErase,
        LostErase,
        FailedPresentation,
        UncertainPresentation,
        NoMark,
        OutOfFrame,
        DuplicateBox,
        NoAnswerRoute,
        Pair,
        PairAbout,
        PairMemoryRefused,
        PairMemoryRepeated,
        PairEnrichmentFailed,
        PairPartialErase,
        PairLostErase,
        PairRefused,
        PairWrongOperation,
        PairChangedReadback,
        PairLostCompletion,
        PairStaleCompletion,
        PairLostDraft,
        PairStaleDraft,
    ] {
        let fixture = SimulatorLedger::new();
        let ledger = if case == PairAbout {
            fixture.open_targets()?
        } else {
            fixture.open()?
        };
        let store = ledger.store().clone();
        let state = setup();
        let [conversation, document, source_page, buddy_page] =
            std::array::from_fn(|_| Uuid::new_v4());
        let target = UserPresentationTarget {
            conversation,
            document,
            page: buddy_page,
        };
        let source = SourceObservation {
            document,
            page: source_page,
            session: "1".into(),
            visit: "1".into(),
            content_revision: Some("1".into()),
            order_revision: None,
            capability_revision: "modeled-source-viewport".into(),
            evidence_procedure: "modeled-rgba8".into(),
        };
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let root = || ledger.expected(Namespace::Conversation, conversation);
        let binding = BindingReceipt {
            operation: Uuid::new_v4(),
            conversation,
            request_fingerprint: "fixture-request".into(),
            adapter_fingerprint: "fixture-owner".into(),
            source: source.clone(),
            intended_target: Some(buddy_page),
            observed_document: document,
            observed_page: buddy_page,
            expected_order: vec![source_page],
            observed_order: vec![source_page, buddy_page],
            persisted_revision: "fixture-insertion".into(),
            observed_ms: 1,
            evidence_origin: "synthetic".into(),
            acquisition: Default::default(),
        };
        ledger.bind(binding.operation, root()?, Default::default(), binding)?;
        let earlier = Turn {
            id: Uuid::new_v4(),
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some("Earlier question".into()),
            sources: vec![],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        };
        if case != PairAbout {
            ledger.append(Uuid::new_v4(), root()?, earlier.clone(), vec![])?;
        }
        let owner = PageOwner {
            document,
            page: buddy_page,
            session: "1".into(),
            visit: "0".into(),
        };
        let content = Content {
            header: "Buddy".into(),
            composer: "Current draft".into(),
            status: "Ready".into(),
            unrelated: "Unrelated Buddy ink".into(),
            exchanges: exchange_history::project(&ledger, conversation, &owner, "Current draft")?
                .exchanges,
        };
        let question_ink = [
            Stroke {
                pixels: (12..31).map(|x| (x, 24)).collect(),
                color: Rgba([0, 0, 0, 255]),
            },
            Stroke {
                pixels: (32..53).map(|x| (x, 39)).collect(),
                color: Rgba([0, 0, 0, 255]),
            },
        ];
        let note = Stroke {
            pixels: (60..71).map(|x| (x, 27)).collect(),
            color: Rgba([30, 30, 30, 255]),
        };
        let circle = Stroke {
            pixels: vec![
                (11, 29),
                (11, 30),
                (11, 31),
                (10, 28),
                (10, 32),
                (9, 28),
                (9, 32),
                (8, 29),
                (8, 30),
                (8, 31),
            ],
            color: Rgba([40, 40, 40, 255]),
        };
        let highlight = Stroke {
            pixels: (if case == Crossing { 50 } else { 60 }..73)
                .map(|x| (x, 32))
                .collect(),
            color: Rgba([240, 221, 90, 255]),
        };
        let protected = [note, circle, highlight];
        let strokes = question_ink
            .iter()
            .chain(&protected)
            .cloned()
            .collect::<Vec<_>>();
        let raster = RgbaImage::from_fn(768, 1024, |x, y| {
            if y == 24 && (12..32).contains(&x) {
                Rgba([70, 70, 70, 255])
            } else {
                Rgba([245, 245, 245, 255])
            }
        });
        let before_erase = {
            let store = store.clone();
            Some(Box::new(move || {
                let ledger = Ledger::new(store.clone());
                let receipt = records(&ledger)
                    .into_iter()
                    .find_map(|r| match r {
                        Record::Receipt(receipt) if receipt.marked_question.is_some() => {
                            Some(receipt)
                        }
                        _ => None,
                    })
                    .expect("marked interpretation committed before erase preparation");
                let mode = store.sync_mode_snapshot().unwrap();
                if case.has_pair() {
                    let saved = records(&ledger);
                    assert!(saved.iter().any(|r| matches!(r, Record::Turn(t)
                        if t.role == Role::Assistant && t.outcome == Outcome::Generated)));
                    assert!(saved.iter().any(|r| matches!(r, Record::Receipt(draft)
                        if draft.acknowledgment.operation == crate::writer::stage(
                            saved.iter().find_map(|r| match r {
                                Record::Turn(t) if t.role == Role::Assistant => Some(t.exchange),
                                _ => None,
                            }).unwrap(), "reader-marked-answer-draft"))));
                }
                assert!(store
                    .set_sync_mode(
                        &mode.token,
                        Uuid::new_v4(),
                        crate::storage::sync::mode::SyncModeChoice::LocalOnly
                    )
                    .is_err());
                if case == StaleReceipt {
                    let mut changed = store
                        .snapshot_heads_matching(&[Namespace::Conversation], 100, |envelope| {
                            envelope.domain_schema_version
                                == crate::conversation::marked_question::SCHEMA
                                && envelope.payload["record"]["acknowledgment"]["operation"]
                                    == receipt.acknowledgment.operation.to_string()
                        })
                        .unwrap()
                        .pop()
                        .unwrap();
                    changed.parents = [changed.revision_id].into_iter().collect();
                    changed.domain_schema_version = 1;
                    changed.revision_id = Uuid::new_v4();
                    changed.operation_id = Uuid::new_v4();
                    changed.kind = crate::storage::Kind::Tombstone;
                    changed.payload = serde_json::Value::Null;
                    store.commit(vec![changed], BTreeMap::new()).unwrap();
                }
            }) as Box<dyn FnOnce()>)
        };
        {
            let mut device = state.borrow_mut();
            device.acquisition_kind = crate::device::backend::AcquisitionKind::Sdk;
            device.session = 1;
            device.visit = 1;
            device.marked_question = Some(MarkedQuestion {
                document,
                source_page,
                target,
                raster: Rc::new(raster.clone()),
                strokes: strokes.clone(),
                viewport: "modeled-viewport-1".into(),
                page: ModeledPage {
                    state: PageState {
                        owner,
                        content: content.clone(),
                        seal: "initial".into(),
                    },
                    receipt: None,
                    fault: match case {
                        FailedPresentation | PairRefused => Some(Case::Refused),
                        UncertainPresentation | PairWrongOperation => Some(Case::WrongOperation),
                        PairChangedReadback => Some(Case::ChangedReadback),
                        _ => None,
                    },
                    calls: 0,
                    pair_operation: None,
                },
                has_destination: case != NoPresentation,
                has_answer_destination: case.has_pair(),
                has_source_binding: case != NoBinding,
                erase_fault: match case {
                    PartialErase | PairPartialErase => Some(EraseFault::Partial),
                    LostErase | PairLostErase => Some(EraseFault::LostReply),
                    ChangedAtAcceptance => Some(EraseFault::RenewedVisitBeforeAccept),
                    _ => None,
                },
                erase_calls: 0,
                accepted_operation: None,
                capture: None,
                captured_strokes: vec![],
                before_erase,
                before_pair_draft: None,
                after_pair: match case {
                    PairLostCompletion | PairStaleCompletion => {
                        let store = store.clone();
                        Some(Box::new(move || {
                            if case == PairLostCompletion {
                                store.set_fault(StorageFault::AfterCommit).unwrap();
                            } else {
                                modify_marked_fixture_root(&store, conversation);
                            }
                        }))
                    }
                    _ => None,
                },
            });
        }
        let calls = Rc::new(RefCell::new(vec![]));
        let mut provider = model(store.clone(), calls.clone());
        let region = match case {
            OutOfFrame => "750,24,48,16",
            DuplicateBox => "12,24,48,16\nQUESTION_BOX: 12,24,48,16",
            _ => "12,24,48,16",
        };
        let mark = if case == NoMark { "absent" } else { "present" };
        let has_answer = case == NoAnswerRoute || case.has_pair();
        let answer = if has_answer {
            "A scripted answer."
        } else {
            "NONE"
        };
        provider.replies = VecDeque::from([format!(
            "QUESTION: G? unc. here\nQUESTION_BOX: {region}\nSELECTION_CENTER: 320,510\nHANDWRITTEN_QUESTION_MARK: {mark}\n---\nANSWER: {answer}"
        )]);
        if has_answer {
            provider
                .replies
                .push_back("TRANSCRIPTION: G? unc. here".into());
        }
        if matches!(case, PairLostDraft | PairStaleDraft) {
            let store = store.clone();
            state
                .borrow_mut()
                .marked_question
                .as_mut()
                .unwrap()
                .before_pair_draft = Some(Box::new(move || {
                if case == PairLostDraft {
                    store.set_fault(StorageFault::AfterCommit).unwrap();
                } else {
                    modify_marked_fixture_root(&store, conversation);
                }
            }));
        }
        provider.after_first = {
            let state = state.clone();
            let store = store.clone();
            Some(Box::new(move || match case {
                ChangedViewport => state
                    .borrow_mut()
                    .marked_question
                    .as_mut()
                    .unwrap()
                    .viewport
                    .push('!'),
                ChangedPage => state.borrow_mut().active = 1,
                ChangedSession => state.borrow_mut().session += 1,
                RenewedVisit | RenewedSession => {
                    let mut device = state.borrow_mut();
                    if case == RenewedVisit {
                        device.visit += 1;
                    } else {
                        device.session += 1;
                    }
                    device.request_pin = Some((device.active, device.visit, device.session));
                }
                LostInterpretation => store.set_fault(StorageFault::AfterCommit).unwrap(),
                _ => {}
            }))
        };
        let mut reader = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            provider,
            ledger,
        );
        if matches!(case, PairMemoryRefused | PairMemoryRepeated) {
            reader.set_selected_subject_memory(Arc::new(RefusedPairMemory {
                inner: crate::memory::LocalMemory::new_local(store.clone(), Uuid::new_v4())?,
                repeat: case == PairMemoryRepeated,
            }));
        }
        if case == PairEnrichmentFailed {
            reader.set_personalization(
                crate::config::Personalization {
                    enabled: true,
                    text: "Use short explanations.".into(),
                    ..Default::default()
                }
                .snapshot()?,
            );
        }
        reader.set_trigger_enabled(false);
        let result = reader.run_iteration();
        let failed = matches!(
            case,
            ChangedPage
                | ChangedSession
                | StaleReceipt
                | LostInterpretation
                | PartialErase
                | LostErase
                | FailedPresentation
                | UncertainPresentation
                | PairPartialErase
                | PairLostErase
                | PairWrongOperation
                | PairChangedReadback
                | PairLostCompletion
                | PairStaleCompletion
                | PairLostDraft
                | PairStaleDraft
                | PairMemoryRefused
                | PairMemoryRepeated
                | PairEnrichmentFailed
        );
        assert_eq!(result.is_err(), failed, "{case:?}: {result:?}");
        assert_eq!(
            calls.borrow().len(),
            if case == PairEnrichmentFailed {
                3
            } else if has_answer {
                2
            } else {
                1
            },
            "{case:?}"
        );
        let overview = image::load_from_memory(&calls.borrow()[0][0])?.to_rgba8();
        assert_eq!(overview.dimensions(), raster.dimensions());
        for stroke in &strokes {
            for &(x, y) in &stroke.pixels {
                assert_eq!(overview.get_pixel(x, y), &stroke.color, "{case:?}");
            }
        }

        let accepted = matches!(
            case,
            Applied
                | PartialErase
                | LostErase
                | FailedPresentation
                | UncertainPresentation
                | Pair
                | PairAbout
                | PairPartialErase
                | PairLostErase
                | PairRefused
                | PairWrongOperation
                | PairChangedReadback
                | PairLostCompletion
                | PairStaleCompletion
        );
        let navigated = matches!(
            case,
            Applied
                | Crossing
                | NoBinding
                | ChangedViewport
                | RenewedVisit
                | RenewedSession
                | ChangedAtAcceptance
                | FailedPresentation
                | UncertainPresentation
                | Pair
                | PairAbout
                | PairRefused
                | PairWrongOperation
                | PairChangedReadback
                | PairLostCompletion
                | PairStaleCompletion
        );
        {
            let device = state.borrow();
            let modeled = device.marked_question.as_ref().unwrap();
            assert_eq!(modeled.erase_calls, usize::from(accepted), "{case:?}");
            assert_eq!(
                device.counts.get(&Operation::Next).copied().unwrap_or(0),
                usize::from(navigated),
                "{case:?}"
            );
            assert_eq!(modeled.page.calls, usize::from(navigated), "{case:?}");
            assert_eq!(modeled.raster.as_ref(), &raster);
            let expected = if matches!(case, PartialErase | PairPartialErase) {
                &strokes[1..]
            } else if accepted {
                protected.as_slice()
            } else {
                &strokes
            };
            assert_eq!(modeled.strokes.as_slice(), expected, "{case:?}");
            assert!(device.events.iter().all(|e| e.action != "erase"));
            assert_eq!(device.counts.get(&Operation::Text), None);
            let shown = navigated && !matches!(case, FailedPresentation | PairRefused);
            let mut observed = modeled.page.state.content.clone();
            if shown {
                let exchange = observed.exchanges.remove(0);
                assert_eq!(exchange.turns.len(), if case.has_pair() { 2 } else { 1 });
                assert_eq!(exchange.turns[0].text, "G? unc. here");
            }
            let mut expected_content = content.clone();
            if case == PairChangedReadback {
                expected_content.unrelated.push('!');
            }
            assert_eq!(observed, expected_content, "{case:?}");
        }
        let ledger = Ledger::new(store);
        let saved = records(&ledger);
        let interpreted = saved.iter().find_map(|r| match r {
            Record::Turn(t)
                if t.id != earlier.id
                    && t.role == Role::User
                    && t.outcome == Outcome::Interpreted =>
            {
                Some(t)
            }
            _ => None,
        });
        let saved_mark = !matches!(
            case,
            NoMark | OutOfFrame | DuplicateBox | ChangedPage | ChangedSession
        );
        assert_eq!(interpreted.is_some(), saved_mark, "{case:?}");
        if let Some(user) = interpreted {
            assert_eq!(user.text.as_deref(), Some("G? unc. here"));
            assert_eq!(user.sources.len(), 1);
            assert_eq!(
                &ledger.stored_sdk_images(user.sources[0])?.images[0].bytes,
                &calls.borrow()[0][0],
                "{case:?} retained a different overview"
            );
            if accepted {
                let operation =
                    crate::writer::stage(user.exchange, "reader-interpret-marked-question");
                assert_eq!(
                    state
                        .borrow()
                        .marked_question
                        .as_ref()
                        .unwrap()
                        .accepted_operation,
                    Some(operation)
                );
            }
        }
        assert_eq!(
            saved
                .iter()
                .filter(
                    |r| matches!(r, Record::Receipt(receipt) if receipt.marked_question.is_some())
                )
                .count(),
            usize::from(saved_mark && case != StaleReceipt)
        );
        let assistant = saved.iter().find_map(|r| match r {
            Record::Turn(t) if t.role == Role::Assistant => Some(t),
            _ => None,
        });
        assert_eq!(
            saved
                .iter()
                .filter(|r| matches!(r, Record::Turn(t) if t.role == Role::Assistant))
                .count(),
            usize::from(assistant.is_some()),
            "{case:?} published multiple drafts"
        );
        let expected = match case {
            Pair | PairAbout | PairLostCompletion => Some(Outcome::Completed),
            PairWrongOperation | PairChangedReadback => Some(Outcome::ReconcileRequired),
            PairPartialErase | PairLostErase | PairLostDraft | PairStaleCompletion
            | PairMemoryRefused | PairMemoryRepeated => Some(Outcome::Generated),
            PairRefused | NoAnswerRoute => Some(Outcome::Failed),
            _ => None,
        };
        assert_eq!(
            assistant.map(|turn| turn.outcome.clone()),
            expected,
            "{case:?}"
        );
        if let Some(answer) = assistant.filter(|_| case.has_pair()) {
            let user = interpreted.unwrap();
            assert_eq!(answer.exchange, user.exchange);
            assert_eq!(answer.sources, user.sources);
            assert!(answer
                .text
                .as_deref()
                .unwrap()
                .contains("A scripted answer."));
            if case == PairAbout {
                let text = answer.text.as_deref().unwrap();
                assert!(text.starts_with("About: Uncertain"));
                let use_record = saved
                    .iter()
                    .find_map(|r| match r {
                        Record::TargetUse(used) if used.turn == user.id => Some(used),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    use_record.inline_body_sha256.as_deref(),
                    Some(crate::storage::digest(text.as_bytes()).as_str())
                );
            }
            let device = state.borrow();
            let page = &device.marked_question.as_ref().unwrap().page;
            assert!(page.receipt.is_none());
            if answer.outcome == Outcome::Completed {
                let completion = answer.completion.as_ref().unwrap();
                assert_eq!(completion.exact_text, *answer.text.as_ref().unwrap());
                assert_eq!(
                    completion.native_operation,
                    crate::writer::stage(user.exchange, "reader-present-marked-answer")
                );
                assert_eq!(completion.observation.page, buddy_page);
                assert_eq!(
                    completion.observation.content_revision.as_deref(),
                    Some(page.state.seal.as_str())
                );
                assert_eq!(completion.procedure, "modeled-pair-edit-readback");
                let revision = ledger
                    .store()
                    .value(Namespace::Conversation, answer.id)?
                    .unwrap()
                    .revision_id;
                assert_eq!(
                    page.state.content.exchanges[0].turns[1].revision == revision,
                    matches!(case, Pair | PairAbout)
                );
            } else {
                assert!(answer.completion.is_none());
                let projection = exchange_history::project(
                    &ledger,
                    conversation,
                    &page.state.owner,
                    "Current draft",
                )?;
                assert!(!projection
                    .exchanges
                    .iter()
                    .flat_map(|exchange| &exchange.turns)
                    .any(|turn| turn.id == answer.id));
            }
        }
        assert!(!saved
            .iter()
            .any(|r| matches!(r, Record::Turn(t) if t.correction_of.is_some())));
        reader.set_subject_memory(None);
        let mode = ledger.store().sync_mode_snapshot()?;
        ledger.store().set_sync_mode(
            &mode.token,
            Uuid::new_v4(),
            crate::storage::sync::mode::SyncModeChoice::LocalOnly,
        )?;
        if let Some(user) =
            interpreted.filter(|_| navigated && !matches!(case, FailedPresentation | PairRefused))
        {
            let current = ledger
                .store()
                .value(Namespace::Conversation, user.id)?
                .unwrap();
            let device = state.borrow();
            let exchange = &device
                .marked_question
                .as_ref()
                .unwrap()
                .page
                .state
                .content
                .exchanges[0];
            assert_eq!(exchange.id, user.exchange);
            assert_eq!(exchange.turns[0].id, user.id);
            assert_eq!(exchange.turns[0].revision, current.revision_id);
        }
        assert!(
            reader.run_iteration().is_err(),
            "{case:?} reacquired the original capture"
        );
        assert_eq!(
            state.borrow().marked_question.as_ref().unwrap().erase_calls,
            usize::from(accepted),
            "{case:?} replayed"
        );
        assert_eq!(
            state.borrow().marked_question.as_ref().unwrap().page.calls,
            usize::from(navigated)
        );
    }
    Ok(())
}

struct RefusedPairMemory {
    inner: crate::memory::LocalMemory,
    repeat: bool,
}

impl crate::memory::Provider for RefusedPairMemory {
    fn put(&self, _: crate::memory::Mutation) -> Result<Uuid> {
        anyhow::bail!("unexpected memory mutation")
    }

    fn forget(&self, _: crate::memory::Forget) -> Result<Uuid> {
        anyhow::bail!("unexpected memory mutation")
    }

    fn inspect(&self, _: &crate::memory::Key) -> Result<Vec<crate::storage::Envelope>> {
        anyhow::bail!("unexpected memory inspection")
    }

    fn query(&self, query: &crate::memory::Query) -> Result<crate::memory::QueryResult> {
        crate::memory::Provider::query(&self.inner, query)
    }

    fn with_current(
        &self,
        _: &crate::memory::QueryResult,
        publish: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        publish()?;
        if self.repeat {
            return publish();
        }
        anyhow::bail!("modeled memory admission refused publication continuation")
    }
}

struct SelectedModel {
    expected: Vec<Vec<u8>>,
    images: Vec<String>,
    text: Vec<String>,
    composed: bool,
    calls: Rc<std::cell::Cell<usize>>,
    store: Arc<Store>,
    replace: Option<(
        Arc<SelectedAdmission>,
        crate::storage::selection::SelectionToken,
    )>,
    fail_write: bool,
    check_mode_lease: bool,
}
impl LLMEngine for SelectedModel {
    fn add_text_content(&mut self, text: &str) {
        self.text.push(text.into());
    }
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.into());
    }
    fn clear_content(&mut self) {
        self.images.clear();
        self.text.clear();
    }
    fn execute(&mut self) -> Result<String> {
        if self.check_mode_lease {
            let mode = self.store.sync_mode_snapshot()?;
            ensure!(
                self.store
                    .set_sync_mode(
                        &mode.token,
                        Uuid::new_v4(),
                        crate::storage::sync::mode::SyncModeChoice::LocalOnly
                    )
                    .is_err(),
                "selected Attempt lost mode lease during provider call"
            );
            ensure!(
                self.store.sync_mode_snapshot()?.mode == mode.mode,
                "refused mode mutation changed persisted state"
            );
        }
        let actual = self
            .images
            .iter()
            .map(|image| STANDARD.decode(image).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            actual == self.expected,
            "selected provider bytes/order changed"
        );
        let calls = self.calls.get() + 1;
        self.calls.set(calls);
        if self.composed && calls == 3 {
            ensure!(
                self.text
                    .iter()
                    .any(|t| t == crate::memory::context::ANSWER_POLICY),
                "answer policy missing"
            );
            ensure!(
                self.text.iter().any(|t| t == "CURRENT_REQUEST: \"Why?\""),
                "question changed by memory"
            );
            ensure!(
                self.text
                    .iter()
                    .any(|t| t.starts_with("ASSOCIATIVE_MEMORY: ")
                        && t.contains("fixture memory answer")),
                "memory context missing"
            );
            return Ok("ANSWER: Because with memory.".into());
        }
        if self.composed {
            ensure!(
                self.text.iter().all(|t| !t.contains("ASSOCIATIVE_MEMORY")),
                "memory reached interpretation"
            );
        }
        if calls == 1 {
            if let Some((admission, token)) = &self.replace {
                let snapshot = self
                    .store
                    .selected_snapshot(token.scope(), crate::storage::MAX_ITEMS)?
                    .unwrap();
                admission.activate(
                    token,
                    crate::storage::selection::SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: snapshot.transaction.accepted_base_sha256,
                        selected: snapshot.transaction.selected,
                        retained: snapshot.transaction.retained,
                    },
                    BTreeMap::new(),
                )?;
            }
            Ok("QUESTION: Why?\nQUESTION_BOX: 20,20,30,30\nSELECTION_CENTER: 300,400\n---\nANSWER: Because.".into())
        } else {
            if self.fail_write {
                self.store.set_fault(StorageFault::BeforeCommit)?;
            }
            Ok("TRANSCRIPTION: Why?".into())
        }
    }
}

#[test]
fn composed_actual_orchestrator_publishes_owned_draft_and_stops_on_replacement_or_write_failure() {
    use crate::memory::{self, Provider};
    use crate::storage::selection::{SelectionChange, SelectionScope};
    use crate::storage::{Coverage, Manifest, ObjectRef, Scope, FORMAT, MAX_ITEMS};
    for fixture_case in 0..17 {
        let resident = fixture_case >= 14;
        let case = match fixture_case {
            14 => 7,
            15 => 1,
            16 => 2,
            other => other,
        };
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        let (capture, _, _) = development_capture_fixture();
        let document = Uuid::parse_str(&capture.expected_binding().document_id).unwrap();
        let identity = memory::catalog::Identity {
            group: Uuid::new_v4(),
            profile: Uuid::new_v4(),
            binding_sha256: crate::storage::digest(b"fixture source binding"),
        };
        if matches!(case, 7 | 12 | 13) {
            let local = memory::LocalMemory::new_local(store.clone(), identity.profile).unwrap();
            local
                .put(memory::Mutation {
                    operation: Uuid::new_v4(),
                    expected: Default::default(),
                    fact: memory::Fact {
                        key: memory::Key {
                            profile: identity.profile,
                            scope: memory::Scope::Document { document },
                            fact: Uuid::new_v4(),
                        },
                        claim: "why".into(),
                        value: "fixture memory answer".into(),
                        subject: "why".into(),
                        tags: vec![],
                        provenance: memory::Provenance {
                            basis: memory::Basis::UserConfirmed,
                            description: "Explicit fixture".into(),
                            conversation: None,
                            turn: None,
                            source: None,
                        },
                        confidence_basis_points: 9000,
                        observed_ms: 1,
                        expires_ms: None,
                    },
                })
                .unwrap();
        }
        let user = Turn {
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
        let (prepared_images, prepared_evidence) = if resident {
            use remarkable_open_sdk::native_adapter::{
                HistoricalCapture, ReaderFacts, RuntimeIdentity,
            };
            use remarkable_open_sdk::{OperationId, PageKey};
            use sha2::Digest;
            let page = PageKey {
                document: sdk_id(document),
                page: sdk_id(Uuid::new_v4()),
            };
            let facts = ReaderFacts::new(page, vec![page.page]).unwrap();
            let png = encode_png(&image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_pixel(12, 20, image::Rgba([17, 29, 41, 255])),
            ));
            let hash = sha2::Sha256::digest(&png).into();
            let captured = crate::conversation::ResidentSourceCapture::from_session(
                &RuntimeIdentity {
                    profile_sha256: [1; 32],
                    executable_sha256: [2; 32],
                    adapter_sha256: [3; 32],
                    boot: sdk_id(Uuid::new_v4()),
                    activation: sdk_id(Uuid::new_v4()),
                    pid: 123,
                    start_ticks: 456,
                },
                OperationId(sdk_id(Uuid::new_v4())),
                OperationId(sdk_id(Uuid::new_v4())),
                facts.clone(),
                HistoricalCapture::new(facts.clone(), facts, [12, 20], 1.0, png, hash).unwrap(),
            )
            .unwrap();
            let prepared = ledger
                .prepare_resident_historical(
                    Uuid::new_v4(),
                    ledger
                        .expected(Namespace::Conversation, conversation)
                        .unwrap(),
                    user.clone(),
                    Uuid::new_v4(),
                    &captured,
                )
                .unwrap();
            (prepared.images, prepared.evidence.id)
        } else if matches!(case, 7 | 8 | 12) {
            let batch = composed_sdk_batch(Uuid::new_v4(), document, Uuid::new_v4());
            let prepared = ledger
                .prepare_sdk_fixture(
                    Uuid::new_v4(),
                    ledger
                        .expected(Namespace::Conversation, conversation)
                        .unwrap(),
                    user.clone(),
                    &batch,
                )
                .unwrap();
            (
                prepared
                    .images
                    .into_iter()
                    .map(|image| image.bytes)
                    .collect::<Vec<_>>(),
                prepared.capture,
            )
        } else {
            let prepared = ledger
                .prepare_development(
                    Uuid::new_v4(),
                    ledger
                        .expected(Namespace::Conversation, conversation)
                        .unwrap(),
                    user.clone(),
                    Uuid::new_v4(),
                    &capture,
                )
                .unwrap();
            (prepared.images, prepared.evidence.id)
        };
        let mut objects = BTreeMap::new();
        let mut refs = BTreeMap::new();
        let mut namespaces = BTreeMap::new();
        let mut media = BTreeMap::new();
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
                refs.insert(reference.sha256.clone(), reference);
            }
            for reference in manifest.media {
                objects.insert(
                    reference.sha256.clone(),
                    store.read_object(&reference).unwrap(),
                );
                media.insert(reference.sha256.clone(), reference);
            }
        }
        let admission = Arc::new(
            SelectedAdmission::new(
                store.clone(),
                SelectionScope {
                    group: identity.group,
                    key_sha256: if matches!(case, 7 | 8 | 12 | 13) {
                        memory::catalog::document_key(document).unwrap()
                    } else {
                        crate::storage::digest(b"selected application fixture")
                    },
                    binding_sha256: identity.binding_sha256.clone(),
                },
            )
            .unwrap(),
        );
        let publication = admission
            .initialize(
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: crate::storage::digest(b"fixture base"),
                    retained: vec![],
                    selected: Manifest {
                        format: FORMAT,
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::SelectedRecords,
                        records: refs.into_values().collect(),
                        record_namespaces: namespaces,
                        media_coverage: media
                            .keys()
                            .map(|sha256| Coverage::Included {
                                sha256: sha256.clone(),
                            })
                            .collect(),
                        media: media.into_values().collect(),
                    },
                },
                objects,
            )
            .unwrap();
        if matches!(case, 5 | 12) {
            let mode = store.sync_mode_snapshot().unwrap();
            store
                .set_sync_mode(
                    &mode.token,
                    Uuid::new_v4(),
                    crate::storage::sync::mode::SyncModeChoice::SharedGroup {
                        group: publication.token.scope().group,
                        binding_sha256: publication.token.scope().binding_sha256.clone(),
                        root_id: "sharedroot".into(),
                    },
                )
                .unwrap();
        }
        let snapshot = store
            .selected_snapshot(publication.token.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        if case == 13 {
            assert!(SelectedDomainProjection::from_snapshot(&snapshot).is_err());
            continue;
        }
        let projection = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
        let pin = |id| {
            let (envelope, _) = projection.head(Namespace::Conversation, id).unwrap();
            let reference = snapshot
                .selected_records
                .iter()
                .zip(&snapshot.transaction.selected.records)
                .find(|(record, _)| record.revision_id == envelope.revision_id)
                .unwrap()
                .1;
            IntentRecordRef {
                namespace: Namespace::Conversation,
                record_id: id,
                revision_id: envelope.revision_id,
                object: ObjectRef {
                    sha256: reference.sha256.clone(),
                    bytes: reference.bytes,
                },
            }
        };
        let calls = Rc::new(std::cell::Cell::new(0));
        let model = SelectedModel {
            expected: prepared_images.clone(),
            images: vec![],
            text: vec![],
            composed: matches!(case, 7 | 12),
            calls: calls.clone(),
            store: store.clone(),
            replace: (case == 1).then(|| (admission.clone(), publication.token.clone())),
            fail_write: case == 2,
            check_mode_lease: matches!(case, 5 | 12),
        };
        let device = setup();
        let facade = crate::workflow::selected_backend::SelectedBackendFacade::new(
            admission.clone(),
            Box::new(SimDevice(device.clone())),
        );
        let workflow = Workflow::with_selected_development_capture(facade, capture, false);
        if case == 3 || case == 4 {
            crate::workflow::assert_selected_retry_control(
                &ledger,
                &workflow,
                publication.token.clone(),
                pin(conversation),
                pin(user.id),
                prepared_evidence,
                case == 4,
            );
            assert!(device.borrow().counts.is_empty());
            continue;
        }
        let mut orchestrator = Orchestrator::new(workflow, model, ledger);
        if matches!(case, 7 | 8 | 12) {
            let memory =
                memory::selected::SelectedMemory::new(store.clone(), identity.clone()).unwrap();
            memory
                .initialize_selection(memory::selected::InitializeSelection {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: crate::storage::digest(b"composed catalog base"),
                    registrations: vec![memory::catalog::Registration {
                        format: 1,
                        catalog: identity.record_id().unwrap(),
                        key_sha256: publication.token.scope().key_sha256.clone(),
                        owner: memory::catalog::Owner::Document { document },
                        genesis_operation: snapshot.transaction.operation,
                        genesis_sha256: publication.token.selection_sha256().into(),
                    }],
                })
                .unwrap();
            orchestrator.set_selected_subject_memory(Arc::new(memory));
        }
        let foreign_fixture = SimulatorLedger::new();
        if (9..=11).contains(&case) {
            let mut foreign_identity = identity.clone();
            if case == 9 {
                foreign_identity.group = Uuid::new_v4();
            }
            if case == 10 {
                foreign_identity.binding_sha256 = crate::storage::digest(b"foreign binding");
            }
            let foreign_store = if case == 11 {
                foreign_fixture.open().unwrap().store().clone()
            } else {
                store.clone()
            };
            let memory =
                memory::selected::SelectedMemory::new(foreign_store, foreign_identity).unwrap();
            orchestrator.set_selected_subject_memory(Arc::new(memory));
        }
        if case == 6 {
            orchestrator.set_selected_subject_memory(Arc::new(RefusedComposedMemory));
        }
        let result = orchestrator.run_selected_prepared(
            publication.token.clone(),
            pin(conversation),
            pin(user.id),
            prepared_evidence,
        );
        if matches!(case, 7 | 12) {
            let error = format!("{:#}", result.as_ref().unwrap_err());
            assert!(
                error.contains("Selected Generated draft acknowledged"),
                "case {fixture_case}: {error}"
            );
        }
        assert!(result.is_err());
        assert!(device.borrow().counts.is_empty());
        assert_eq!(
            calls.get(),
            if case == 6 || (9..=11).contains(&case) {
                0
            } else if matches!(case, 7 | 12) {
                3
            } else if case == 1 {
                1
            } else {
                2
            }
        );
        let current = store
            .selected_snapshot(publication.token.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let records = SelectedDomainProjection::from_snapshot(&current)
            .unwrap()
            .inspect(conversation)
            .unwrap();
        let drafts = records
            .iter()
            .filter_map(|record| match record {
                Record::Turn(turn) if turn.role == Role::Assistant => Some(turn),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            drafts.len(),
            if case == 0 || matches!(case, 5 | 12) || matches!(case, 7 | 8 | 12) {
                1
            } else {
                0
            }
        );
        if case == 0 || matches!(case, 5 | 12) || matches!(case, 7 | 8 | 12) {
            assert_eq!(drafts[0].outcome, Outcome::Generated);
            if matches!(case, 7 | 12) {
                assert!(drafts[0]
                    .text
                    .as_deref()
                    .unwrap()
                    .contains("A: Because with memory."));
            }
            assert!(drafts[0].sources.is_empty());
            assert_eq!(drafts[0].exchange, user.exchange);
        }
        assert!(!records
            .iter()
            .any(|record| matches!(record, Record::OutcomeFact(_) | Record::Binding(_))));
        if resident {
            let SelectedCaptureImages::Resident(saved) = admission
                .capture_images(&current.token, prepared_evidence)
                .unwrap()
            else {
                panic!("resident historical origin changed")
            };
            assert_eq!(saved.images, prepared_images);
            continue;
        }
        if matches!(case, 7 | 8 | 12) {
            let SelectedCaptureImages::Sdk(saved) = admission
                .capture_images(&current.token, prepared_evidence)
                .unwrap()
            else {
                panic!("synthetic origin changed")
            };
            assert_eq!(
                saved
                    .images
                    .into_iter()
                    .map(|i| i.bytes)
                    .collect::<Vec<_>>(),
                prepared_images
            );
            continue;
        }
        let SelectedCaptureImages::Development(saved) = admission
            .capture_images(&current.token, prepared_evidence)
            .unwrap()
        else {
            panic!("historical origin changed")
        };
        assert_eq!(saved.images, prepared_images);
    }
}
