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
            output_dimensions: [2, 3],
            filter: ResizeFilter::Nearest,
            png: encode_png(&native.resize_exact(2, 3, image::imageops::FilterType::Nearest)),
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
