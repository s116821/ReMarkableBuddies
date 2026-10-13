use remarkable_open_sdk::{
    capture::{synthetic::*, *},
    mock::MockPlatform,
    Platform,
};
use remarkable_reader_buddy::{
    conversation::{
        capture_facts::{HistoricalCapture, SDK_SOURCE},
        *,
    },
    storage::*,
};
use std::{collections::BTreeMap, io::Cursor, path::PathBuf, sync::Arc, time::Duration};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-sdk-facts-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("secrets"),
            })
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn sdk(id: Uuid) -> remarkable_open_sdk::Uuid {
    remarkable_open_sdk::Uuid::parse(&id.to_string()).unwrap()
}
fn png(image: &image::DynamicImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn batch(device: Uuid, document: Uuid, page: Uuid) -> CapturedBatch {
    let source = MockPlatform::new(sdk(device), sdk(document), vec![sdk(page)], sdk(page))
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
        native_png: png(&native),
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
        remarkable_open_sdk::OperationId(sdk(Uuid::new_v4())),
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
            png: png(&native.resize_exact(2, 3, image::imageops::FilterType::Nearest)),
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
            png: png(&native.crop_imm(0, 2, 4, 2)),
        },
    ];
    validate_batch(&request, acquisition, images, CaptureLimits::default()).unwrap()
}
fn request(conversation: Uuid) -> Turn {
    Turn {
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
        created_ms: 10,
        updated_ms: 10,
        completion: None,
    }
}
fn root(ledger: &Ledger, conversation: Uuid) -> ExpectedHeads {
    ledger
        .expected(Namespace::Conversation, conversation)
        .unwrap()
}

fn select_fixture(store: &Arc<Store>) -> (Arc<SelectedAdmission>, selection::SelectionToken) {
    select_fixture_scope(
        store,
        selection::SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"Reader text fixture"),
            binding_sha256: digest(b"fixture binding"),
        },
    )
}
fn select_fixture_scope(
    store: &Arc<Store>,
    scope: selection::SelectionScope,
) -> (Arc<SelectedAdmission>, selection::SelectionToken) {
    let mut objects = BTreeMap::new();
    let mut records = BTreeMap::new();
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
    let admission = Arc::new(SelectedAdmission::new(store.clone(), scope).unwrap());
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
    let publication = admission
        .initialize(
            selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"fixture base"),
                selected,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    (admission, publication.token)
}
fn selected_pin(store: &Store, token: &selection::SelectionToken, id: Uuid) -> IntentRecordRef {
    let snapshot = store
        .selected_snapshot(token.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let projection = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
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
        object: reference.clone(),
    }
}

#[test]
fn composed_memory_reader_same_key_publishes_once_without_reentrant_gate() {
    // Run the real shared-key path in a bounded child; an admission regression
    // must fail on a deadline rather than hang this test runner.
    if std::env::var_os("BUDDY_COMPOSED_GATE_CHILD").is_none() {
        for fault in [
            "none",
            "before",
            "after",
            "concurrent",
            "reader",
            "fact",
            "budget",
        ] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "composed_memory_reader_same_key_publishes_once_without_reentrant_gate",
                    "--nocapture",
                ])
                .env("BUDDY_COMPOSED_GATE_CHILD", "1")
                .env("BUDDY_COMPOSED_FAULT", fault)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("combined admission exceeded deadline");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        return;
    }
    use remarkable_reader_buddy::memory::{self, Provider};
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let conversation = create(&ledger);
    let document = Uuid::new_v4();
    let source = batch(Uuid::new_v4(), document, Uuid::new_v4());
    let original = ledger
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            root(&ledger, conversation),
            request(conversation),
            &source,
        )
        .unwrap();
    let identity = memory::catalog::Identity {
        group: Uuid::new_v4(),
        profile: Uuid::new_v4(),
        binding_sha256: digest(b"composed binding"),
    };
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
                claim: "gravity units".into(),
                value: "SI gravity units".into(),
                subject: "gravity".into(),
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
    drop(local);
    let scope = identity
        .aggregate_scope(&memory::catalog::Owner::Document { document })
        .unwrap();
    let (reader, initial) = select_fixture_scope(&store, scope);
    let genesis = store
        .selected_snapshot(initial.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap()
        .transaction;
    let memory = memory::selected::SelectedMemory::new(store.clone(), identity.clone()).unwrap();
    let global_key = memory::Key {
        profile: identity.profile,
        scope: memory::Scope::Global,
        fact: Uuid::new_v4(),
    };
    let global_registration = memory
        .initialize_aggregate(memory::selected::InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"distinct global aggregate"),
            fact: memory::Fact {
                key: global_key.clone(),
                claim: "gravity global units".into(),
                value: "global SI context".into(),
                subject: "gravity".into(),
                tags: vec![],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Explicit global fixture".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 9000,
                observed_ms: 1,
                expires_ms: None,
            },
        })
        .unwrap()
        .registration
        .unwrap();
    let mut registrations = vec![
        global_registration,
        memory::catalog::Registration {
            format: 1,
            catalog: identity.record_id().unwrap(),
            key_sha256: initial.scope().key_sha256.clone(),
            owner: memory::catalog::Owner::Document { document },
            genesis_operation: genesis.operation,
            genesis_sha256: initial.selection_sha256().into(),
        },
    ];
    if std::env::var("BUDDY_COMPOSED_FAULT").as_deref() == Ok("budget") {
        let global_scope = identity
            .aggregate_scope(&memory::catalog::Owner::GlobalFact {
                key: global_key.clone(),
            })
            .unwrap();
        let seed = store
            .selected_snapshot(&global_scope, MAX_ITEMS)
            .unwrap()
            .unwrap();
        let seed_fact: memory::Fact =
            serde_json::from_value(seed.selected_records[0].payload.clone()).unwrap();
        for index in 0..30 {
            let mut fact = seed_fact.clone();
            fact.key.fact = Uuid::new_v4();
            fact.claim = format!("unrelated {index}");
            fact.subject = "unrelated".into();
            registrations.push(
                memory
                    .initialize_aggregate(memory::selected::InitializeAggregate {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: digest(b"budget aggregate"),
                        fact,
                    })
                    .unwrap()
                    .registration
                    .unwrap(),
            );
        }
    }
    memory
        .initialize_selection(memory::selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"composed catalog base"),
            registrations,
        })
        .unwrap();
    let interpretation = SelectedReaderWrite {
        operation: Uuid::new_v4(),
        root: selected_pin(&store, &initial, conversation),
        user: selected_pin(&store, &initial, original.turn),
        updated_ms: 11,
        text: "gravity units?".into(),
        kind: SelectedReaderText::Interpret,
    };
    let SelectedReaderPublication::Published {
        publication,
        acknowledgment,
    } = reader
        .publish_reader_text(&initial, interpretation)
        .unwrap()
    else {
        panic!("interpretation replayed")
    };
    let mut query = memory::Query::new("gravity units".into(), 20);
    query.document = Some(document);
    let view = memory.query(&query).unwrap();
    assert_eq!(view.status, memory::Status::Complete);
    assert_eq!(view.context.facts.len(), 2);
    let draft = SelectedReaderWrite {
        operation: Uuid::new_v4(),
        root: acknowledgment.root,
        user: acknowledgment.turn,
        updated_ms: 12,
        text: "source and optional memory answer".into(),
        kind: SelectedReaderText::Generate {
            assistant: Uuid::new_v4(),
        },
    };
    let foreign_instance =
        memory::selected::SelectedMemory::new(store.clone(), identity.clone()).unwrap();
    assert!(foreign_instance
        .publish_selected_reader_text(&view, &reader, &publication.token, draft.clone())
        .is_err());
    let original_global_token = store
        .selected_snapshot(
            &identity
                .aggregate_scope(&memory::catalog::Owner::GlobalFact {
                    key: global_key.clone(),
                })
                .unwrap(),
            MAX_ITEMS,
        )
        .unwrap()
        .unwrap()
        .token;
    let fault = std::env::var("BUDDY_COMPOSED_FAULT").unwrap_or_else(|_| "none".into());
    if matches!(fault.as_str(), "reader" | "fact" | "budget") {
        if fault != "budget" {
            let scope = if fault == "reader" {
                initial.scope().clone()
            } else {
                identity
                    .aggregate_scope(&memory::catalog::Owner::GlobalFact { key: global_key })
                    .unwrap()
            };
            let selected = store.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
            SelectedAdmission::new(store.clone(), scope)
                .unwrap()
                .activate(
                    &selected.token,
                    selection::SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: selected.transaction.accepted_base_sha256,
                        selected: selected.transaction.selected,
                        retained: selected.transaction.retained,
                    },
                    BTreeMap::new(),
                )
                .unwrap();
        }
        let before = store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token;
        let refused = memory
            .publish_selected_reader_text(&view, &reader, &publication.token, draft.clone())
            .err()
            .expect("new publication must refuse");
        if fault == "budget" {
            assert!(format!("{refused:#}").contains("selected read budget exhausted"));
        }
        assert!(memory
            .publish_selected_reader_text(&view, &reader, &publication.token, draft)
            .is_err());
        assert_eq!(
            store
                .selected_snapshot(initial.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            before
        );
        return;
    }
    if fault == "concurrent" {
        let second_view = memory.query(&query).unwrap();
        let second_reader =
            Arc::new(SelectedAdmission::new(store.clone(), initial.scope().clone()).unwrap());
        let memory = Arc::new(memory);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for (selected_reader, selected_view) in
            [(reader.clone(), view), (second_reader, second_view)]
        {
            let memory = memory.clone();
            let barrier = barrier.clone();
            let accepted = publication.token.clone();
            let mut request = draft.clone();
            request.operation = Uuid::new_v4();
            request.kind = SelectedReaderText::Generate {
                assistant: Uuid::new_v4(),
            };
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                memory.publish_selected_reader_text(
                    &selected_view,
                    &selected_reader,
                    &accepted,
                    request,
                )
            }));
        }
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(SelectedReaderPublication::Published { .. })))
                .count(),
            1
        );
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let selected = store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let records = SelectedDomainProjection::from_snapshot(&selected)
            .unwrap()
            .inspect(conversation)
            .unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r, Record::Turn(t) if t.role == Role::Assistant))
                .count(),
            1
        );
        return;
    }
    if fault != "none" {
        store
            .set_fault(if fault == "before" {
                Fault::BeforeCommit
            } else {
                Fault::AfterCommit
            })
            .unwrap();
        let first =
            memory.publish_selected_reader_text(&view, &reader, &publication.token, draft.clone());
        if fault == "before" {
            assert!(first.is_err());
        } else {
            assert!(matches!(
                first.unwrap(),
                SelectedReaderPublication::Historical { .. }
            ));
        }
        let retry =
            memory.publish_selected_reader_text(&view, &reader, &publication.token, draft.clone());
        if fault == "before" {
            assert!(retry.is_err());
            assert_eq!(
                store
                    .selected_snapshot(initial.scope(), MAX_ITEMS)
                    .unwrap()
                    .unwrap()
                    .token,
                publication.token
            );
        } else {
            assert!(matches!(
                retry.unwrap(),
                SelectedReaderPublication::Historical { .. }
            ));
            let selected = store
                .selected_snapshot(initial.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap();
            let records = SelectedDomainProjection::from_snapshot(&selected)
                .unwrap()
                .inspect(conversation)
                .unwrap();
            assert_eq!(
                records
                    .iter()
                    .filter(|r| matches!(r, Record::Turn(t) if t.role == Role::Assistant))
                    .count(),
                1
            );
        }
        return;
    }
    let SelectedReaderPublication::Published {
        publication: generated,
        acknowledgment: ack,
    } = memory
        .publish_selected_reader_text(&view, &reader, &publication.token, draft.clone())
        .unwrap()
    else {
        panic!("new combined draft historical")
    };
    assert!(ack.written.sources.is_empty());
    assert_eq!(ack.user.sources, vec![original.capture]);
    assert!(matches!(
        memory
            .publish_selected_reader_text(&view, &reader, &publication.token, draft.clone())
            .unwrap(),
        SelectedReaderPublication::Historical { .. }
    ));
    let mut changed = draft.clone();
    changed.operation = Uuid::new_v4();
    assert!(memory
        .publish_selected_reader_text(&view, &reader, &publication.token, changed)
        .is_err());
    let mut altered = view.clone();
    altered.context.facts[0].fact.value.push('!');
    assert!(memory
        .publish_selected_reader_text(&altered, &reader, &publication.token, draft.clone())
        .is_err());
    assert_eq!(
        store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        generated.token
    );
    let SelectedCaptureImages::Sdk(images) = reader
        .capture_images(&generated.token, original.capture)
        .unwrap()
    else {
        panic!("capture origin changed")
    };
    assert_eq!(
        images
            .images
            .iter()
            .map(|image| &image.bytes)
            .collect::<Vec<_>>(),
        original
            .images
            .iter()
            .map(|image| &image.bytes)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .selected_snapshot(original_global_token.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        original_global_token
    );
    let fresh_view = memory.query(&query).unwrap();
    let catalog_scope = identity.scope().unwrap();
    let catalog_snapshot = store
        .selected_snapshot(&catalog_scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    let catalog_admission = SelectedAdmission::new(store.clone(), catalog_scope).unwrap();
    catalog_admission
        .activate(
            &catalog_snapshot.token,
            selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: catalog_snapshot.transaction.accepted_base_sha256,
                selected: catalog_snapshot.transaction.selected,
                retained: catalog_snapshot.transaction.retained,
            },
            BTreeMap::new(),
        )
        .unwrap();
    let next = SelectedReaderWrite {
        operation: Uuid::new_v4(),
        root: ack.root.clone(),
        user: ack.turn.clone(),
        updated_ms: 13,
        text: "must not publish after catalog replacement".into(),
        kind: SelectedReaderText::Generate {
            assistant: Uuid::new_v4(),
        },
    };
    assert!(memory
        .publish_selected_reader_text(&fresh_view, &reader, &generated.token, next.clone())
        .is_err());
    assert!(memory
        .publish_selected_reader_text(&fresh_view, &reader, &generated.token, next)
        .is_err());
    assert!(matches!(
        memory
            .publish_selected_reader_text(&view, &reader, &publication.token, draft)
            .unwrap(),
        SelectedReaderPublication::Historical { .. }
    ));
    assert_eq!(
        store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        generated.token
    );
    let valid = store
        .selected_snapshot(initial.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    for case in 0..12 {
        let mut malformed = store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let memory_index = malformed
            .selected_records
            .iter()
            .position(|record| record.namespace == Namespace::SubjectMemory)
            .unwrap();
        let memory_id = malformed.selected_records[memory_index].record_id;
        match case {
            0 => malformed.selected_records[memory_index].domain_schema_version = 9,
            1 => {
                malformed.selected_records[memory_index].payload["key"]["scope"] =
                    serde_json::json!("global")
            }
            2 => {
                malformed.selected_records[memory_index].payload["key"]["scope"] =
                    serde_json::json!({"document":{"document":Uuid::new_v4()}})
            }
            3 => malformed.selected_records[memory_index].payload["claim"] = serde_json::json!(""),
            4 => {
                malformed.selected_records[memory_index]
                    .parents
                    .insert(Uuid::new_v4());
            }
            5 => {
                let mut fork = malformed.selected_records[memory_index].clone();
                fork.revision_id = Uuid::new_v4();
                fork.operation_id = Uuid::new_v4();
                malformed.selected_records.push(fork);
            }
            6 => {
                let mut child = malformed.selected_records[memory_index].clone();
                child.revision_id = Uuid::new_v4();
                child.operation_id = Uuid::new_v4();
                child
                    .parents
                    .insert(malformed.selected_records[memory_index].revision_id);
                malformed.selected_records[memory_index]
                    .parents
                    .insert(child.revision_id);
                malformed.selected_records.push(child);
            }
            7 => {
                malformed.selected_records[memory_index].revision_id = malformed
                    .selected_records
                    .iter()
                    .find(|record| record.namespace != Namespace::SubjectMemory)
                    .unwrap()
                    .revision_id
            }
            8 => malformed.selected_records[memory_index].namespace = Namespace::Handwriting,
            9 => {
                malformed.selected_records[memory_index].kind = Kind::Tombstone;
                malformed.selected_records[memory_index].payload = serde_json::Value::Null;
            }
            10 => {
                let mut edit = malformed.selected_records[memory_index].clone();
                edit.revision_id = Uuid::new_v4();
                edit.operation_id = Uuid::new_v4();
                edit.parents
                    .insert(malformed.selected_records[memory_index].revision_id);
                edit.payload["claim"] = serde_json::json!("changed immutable claim");
                malformed.selected_records.push(edit);
            }
            _ => {
                let foreign = malformed
                    .selected_records
                    .iter()
                    .find(|record| record.namespace == Namespace::Conversation)
                    .unwrap()
                    .revision_id;
                malformed.selected_records[memory_index]
                    .parents
                    .insert(foreign);
            }
        }
        malformed.transaction.selected.records = malformed
            .selected_records
            .iter()
            .map(|record| {
                let raw = serde_json::to_vec(record).unwrap();
                ObjectRef {
                    sha256: digest(&raw),
                    bytes: raw.len() as u64,
                }
            })
            .collect();
        assert!(
            SelectedDomainProjection::from_snapshot(&malformed).is_err(),
            "partition case{case} accepted memory{memory_id}"
        );
    }
    for case in 0..5 {
        let mut malformed = store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        match case {
            0 => malformed
                .transaction
                .selected
                .records
                .pop()
                .map(|_| ())
                .unwrap(),
            1 => malformed.transaction.selected.records[0].sha256 = "invalid".into(),
            2 => malformed.transaction.selected.records[0].bytes = 0,
            3 => malformed.transaction.selected.records[0].bytes = MAX_RECORD as u64 + 1,
            4 => {
                let record = malformed.selected_records[0].clone();
                malformed.selected_records = vec![record; MAX_ITEMS + 1];
                malformed.transaction.selected.records =
                    vec![valid.transaction.selected.records[0].clone(); MAX_ITEMS + 1];
            }
            _ => unreachable!(),
        }
        assert!(
            SelectedDomainProjection::from_snapshot(&malformed).is_err(),
            "mixed reference case {case} accepted"
        );
    }
    let mut other_profile = store
        .selected_snapshot(initial.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let mut other = other_profile
        .selected_records
        .iter()
        .find(|record| record.namespace == Namespace::SubjectMemory)
        .unwrap()
        .clone();
    let mut fact: memory::Fact = serde_json::from_value(other.payload.clone()).unwrap();
    fact.key.profile = Uuid::new_v4();
    other.record_id = fact.key.record_id().unwrap();
    other.revision_id = Uuid::new_v4();
    other.operation_id = Uuid::new_v4();
    other.payload = serde_json::to_value(fact).unwrap();
    other.parents.clear();
    let raw = serde_json::to_vec(&other).unwrap();
    other_profile.transaction.selected.records.push(ObjectRef {
        sha256: digest(&raw),
        bytes: raw.len() as u64,
    });
    other_profile.selected_records.push(other);
    assert!(SelectedDomainProjection::from_snapshot(&other_profile).is_ok());
    // A valid later tombstone preserves its owned ancestor and never enters the
    // Reader transcript. Other profiles in this same document are also retained.
    let mut tombstone = store
        .selected_snapshot(initial.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let prior = tombstone
        .selected_records
        .iter()
        .find(|record| record.namespace == Namespace::SubjectMemory)
        .unwrap()
        .clone();
    let mut deleted = prior.clone();
    deleted.revision_id = Uuid::new_v4();
    deleted.operation_id = Uuid::new_v4();
    deleted.parents.insert(prior.revision_id);
    deleted.kind = Kind::Tombstone;
    deleted.payload = serde_json::Value::Null;
    tombstone.selected_records.push(deleted);
    tombstone.transaction.selected.records = tombstone
        .selected_records
        .iter()
        .map(|record| {
            let raw = serde_json::to_vec(record).unwrap();
            ObjectRef {
                sha256: digest(&raw),
                bytes: raw.len() as u64,
            }
        })
        .collect();
    assert!(SelectedDomainProjection::from_snapshot(&tombstone).is_ok());
    assert_eq!(
        store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        valid.token
    );
}

#[test]
fn selected_reader_generation_preserves_user_capture_ownership_and_historical_retry() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let conversation = create(&ledger);
    let source = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let original = ledger
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            root(&ledger, conversation),
            request(conversation),
            &source,
        )
        .unwrap();
    let (admission, initial) = select_fixture(&store);
    let interpretation = SelectedReaderWrite {
        operation: Uuid::new_v4(),
        root: selected_pin(&store, &initial, conversation),
        user: selected_pin(&store, &initial, original.turn),
        updated_ms: 11,
        text: "Verified question".into(),
        kind: SelectedReaderText::Interpret,
    };
    let SelectedReaderPublication::Published {
        publication: interpreted,
        acknowledgment,
    } = admission
        .publish_reader_text(&initial, interpretation.clone())
        .unwrap()
    else {
        panic!("new interpretation historical")
    };
    let generation = SelectedReaderWrite {
        operation: Uuid::new_v4(),
        root: acknowledgment.root.clone(),
        user: acknowledgment.turn.clone(),
        updated_ms: 12,
        text: "Generated answer".into(),
        kind: SelectedReaderText::Generate {
            assistant: Uuid::new_v4(),
        },
    };
    let SelectedReaderPublication::Published {
        publication: generated,
        acknowledgment: draft,
    } = admission
        .publish_reader_text(&interpreted.token, generation.clone())
        .unwrap()
    else {
        panic!("new draft historical")
    };
    assert!(draft.written.sources.is_empty());
    assert_eq!(draft.written.exchange, draft.user.exchange);
    assert_eq!(draft.user.sources, vec![original.capture]);
    assert_eq!(draft.written.sequence, 1);
    assert!(draft.receipt.admitted_intent.is_none());
    let selected = store
        .selected_snapshot(initial.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let records = SelectedDomainProjection::from_snapshot(&selected)
        .unwrap()
        .inspect(conversation)
        .unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, Record::Turn(_)))
            .count(),
        2
    );
    let SelectedCaptureImages::Sdk(actual) = admission
        .capture_images(&generated.token, original.capture)
        .unwrap()
    else {
        panic!("capture origin changed")
    };
    assert_eq!(actual.native_parent.bytes, original.native_parent.bytes);
    assert_eq!(
        actual
            .images
            .iter()
            .map(|image| (&image.bytes, image.ordinal))
            .collect::<Vec<_>>(),
        original
            .images
            .iter()
            .map(|image| (&image.bytes, image.ordinal))
            .collect::<Vec<_>>()
    );
    let before = selected.token.clone();
    for (token, input) in [
        (&initial, &interpretation),
        (&interpreted.token, &generation),
    ] {
        let SelectedReaderPublication::Historical { acknowledgment, .. } =
            admission.publish_reader_text(token, input.clone()).unwrap()
        else {
            panic!("retry minted current result")
        };
        assert!(acknowledgment.receipt.admitted_intent.is_none());
        let mut changed = input.clone();
        changed.text.push('!');
        assert!(admission.publish_reader_text(token, changed).is_err());
    }
    assert_eq!(
        store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        before
    );
    let scope = initial.scope().clone();
    drop(ledger);
    drop(admission);
    drop(store);
    let reopened = fixture.open();
    let reader = SelectedAdmission::new(reopened.clone(), scope).unwrap();
    assert!(matches!(
        reader
            .publish_reader_text(&interpreted.token, generation)
            .unwrap(),
        SelectedReaderPublication::Historical { .. }
    ));
    let snapshot = reopened
        .selected_snapshot(initial.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.token, before);
}

#[test]
fn selected_reader_write_recovers_lost_ack_and_preserves_precommit_failure() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let conversation = create(&ledger);
        let source = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let original = ledger
            .prepare_sdk_fixture(
                Uuid::new_v4(),
                root(&ledger, conversation),
                request(conversation),
                &source,
            )
            .unwrap();
        let (admission, initial) = select_fixture(&store);
        let input = SelectedReaderWrite {
            operation: Uuid::new_v4(),
            root: selected_pin(&store, &initial, conversation),
            user: selected_pin(&store, &initial, original.turn),
            updated_ms: 11,
            text: "One question".into(),
            kind: SelectedReaderText::Interpret,
        };
        store.set_fault(fault).unwrap();
        let result = admission.publish_reader_text(&initial, input.clone());
        if fault == Fault::BeforeCommit {
            assert!(result.is_err());
            assert_eq!(
                store
                    .selected_snapshot(initial.scope(), MAX_ITEMS)
                    .unwrap()
                    .unwrap()
                    .token,
                initial
            );
            assert!(matches!(
                admission
                    .publish_reader_text(&initial, input.clone())
                    .unwrap(),
                SelectedReaderPublication::Published { .. }
            ));
        } else {
            assert!(matches!(
                result.unwrap(),
                SelectedReaderPublication::Historical { .. }
            ));
        }
        assert!(matches!(
            admission.publish_reader_text(&initial, input).unwrap(),
            SelectedReaderPublication::Historical { .. }
        ));
    }
}

#[test]
fn selected_reader_write_refuses_nonlocal_modes_stale_pins_and_omitted_media() {
    use sync::mode::SyncModeChoice;
    for case in 0..6 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let conversation = create(&ledger);
        let source = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let original = ledger
            .prepare_sdk_fixture(
                Uuid::new_v4(),
                root(&ledger, conversation),
                request(conversation),
                &source,
            )
            .unwrap();
        let (admission, initial) = select_fixture(&store);
        let mut input = SelectedReaderWrite {
            operation: Uuid::new_v4(),
            root: selected_pin(&store, &initial, conversation),
            user: selected_pin(&store, &initial, original.turn),
            updated_ms: 11,
            text: "Question".into(),
            kind: SelectedReaderText::Interpret,
        };
        let snapshot = store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        if case < 3 {
            let mode = store.sync_mode_snapshot().unwrap();
            let choice = match case {
                0 => SyncModeChoice::LocalOnly,
                1 => SyncModeChoice::IsolatedBackup {
                    binding_sha256: digest(b"isolated"),
                },
                _ => SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: initial.scope().binding_sha256.clone(),
                    root_id: "sharedroot".into(),
                },
            };
            store
                .set_sync_mode(&mode.token, Uuid::new_v4(), choice)
                .unwrap();
        } else if case == 3 || case == 5 {
            let mut selected = snapshot.transaction.selected.clone();
            if case == 5 {
                selected.media.clear();
                for coverage in &mut selected.media_coverage {
                    *coverage = Coverage::Omitted {
                        sha256: coverage.hash().into(),
                        reason: Omission::PolicyDisabled,
                    };
                }
            }
            let next = admission
                .activate(
                    &initial,
                    selection::SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: snapshot.transaction.accepted_base_sha256.clone(),
                        selected,
                        retained: snapshot.transaction.retained.clone(),
                    },
                    BTreeMap::new(),
                )
                .unwrap();
            if case == 5 {
                let before = next.token.clone();
                assert!(admission
                    .publish_reader_text(&next.token, input.clone())
                    .is_err());
                assert_eq!(
                    store
                        .selected_snapshot(initial.scope(), MAX_ITEMS)
                        .unwrap()
                        .unwrap()
                        .token,
                    before
                );
                continue;
            }
        } else {
            input.user.object.sha256 = digest(b"changed raw pin");
        }
        let before = store
            .selected_snapshot(initial.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token;
        assert!(
            admission.publish_reader_text(&initial, input).is_err(),
            "case{case}"
        );
        assert_eq!(
            store
                .selected_snapshot(initial.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            before
        );
    }
}
fn create(ledger: &Ledger) -> Uuid {
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 1).unwrap();
    id
}

#[test]
fn exact_sdk_facts_and_all_media_survive_real_store_reopen() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let conversation = create(&ledger);
    let batch = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let original = HistoricalCapture::from_batch(&batch).unwrap();
    let submitted = ledger
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            root(&ledger, conversation),
            request(conversation),
            &batch,
        )
        .unwrap();
    let capture = submitted.capture;
    assert_eq!(submitted.native_parent.bytes, batch.native_parent().bytes());
    for (index, (stored, image)) in submitted.images.iter().zip(batch.images()).enumerate() {
        assert_eq!(stored.ordinal, Some(index as u32));
        assert_eq!(stored.bytes, image.bytes());
    }
    let before = serde_json::to_value(&original).unwrap();
    assert_eq!(before["sdk_source"], SDK_SOURCE);
    assert_eq!(before["buffer_generation"], u64::MAX.to_string());
    assert_eq!(
        before["interval"][0]["seconds"],
        ((1u64 << 53) + 1).to_string()
    );
    assert_eq!(before["interval"][0]["nanoseconds"], 123_456_789);
    assert_eq!(
        before["native_parent"]["affine_bits"][1],
        (-0.0f64).to_bits().to_string()
    );
    assert_eq!(
        before["native_parent"]["valid_region_bits"][0],
        (-0.0f64).to_bits().to_string()
    );
    assert_eq!(before["images"][1]["role"]["ordinal"], 7);
    assert!(before["native_profile"].is_null() && before["global_clock_scope"].is_null());
    assert_eq!(
        before["source"]["device"],
        batch
            .source()
            .export_facts(Default::default())
            .unwrap()
            .device()
            .to_string()
    );
    drop(ledger);
    let ledger = Ledger::new(fixture.open());
    let record = ledger
        .inspect(conversation, false)
        .unwrap()
        .into_iter()
        .find_map(|record| match record {
            Record::Capture(capture) => Some(capture),
            _ => None,
        })
        .unwrap();
    assert_eq!(record.facts, original);
    assert_eq!(serde_json::to_value(&record.facts).unwrap(), before);
    let restored = ledger.stored_sdk_images(capture).unwrap();
    assert_eq!(restored.native_parent.bytes, submitted.native_parent.bytes);
    assert_eq!(
        restored
            .images
            .iter()
            .map(|image| &image.bytes)
            .collect::<Vec<_>>(),
        submitted
            .images
            .iter()
            .map(|image| &image.bytes)
            .collect::<Vec<_>>()
    );
    assert_eq!(ledger.retained_media(conversation).unwrap().len(), 3);
}

#[test]
fn strict_historical_container_rejects_noncanonical_unknown_and_missing_facts() {
    let original =
        HistoricalCapture::from_batch(&batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()))
            .unwrap();
    let json = serde_json::to_value(original).unwrap();
    for value in ["01", "+1", "1.0", "-1", "18446744073709551616", " 1"] {
        let mut invalid = json.clone();
        invalid["buffer_generation"] = value.into();
        assert!(serde_json::from_value::<HistoricalCapture>(invalid).is_err());
    }
    for field in [
        "target_bits",
        "native_profile",
        "global_clock_scope",
        "source",
        "schema",
    ] {
        let mut invalid = json.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<HistoricalCapture>(invalid).is_err(),
            "missing {field}"
        );
    }
    let mut invalid = json.clone();
    invalid["source"]["surprise"] = true.into();
    assert!(serde_json::from_value::<HistoricalCapture>(invalid).is_err());
    let mut invalid = json.clone();
    invalid["qualification"] = "NativeQualified".into();
    assert!(serde_json::from_value::<HistoricalCapture>(invalid).is_err());
    let mut invalid = json.clone();
    invalid["native_parent"]
        .as_object_mut()
        .unwrap()
        .remove("derivation");
    assert!(serde_json::from_value::<HistoricalCapture>(invalid).is_err());
    for (field, value) in [
        ("schema", "unknown"),
        ("sdk_source", "unknown"),
        ("native_profile", "invented"),
        ("global_clock_scope", "invented"),
    ] {
        let mut invalid = json.clone();
        invalid[field] = value.into();
        assert!(serde_json::from_value::<HistoricalCapture>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
    let mut invalid = json.clone();
    invalid["source"]["device"] = "00000000-0000-0000-0000-000000000000".into();
    assert!(serde_json::from_value::<HistoricalCapture>(invalid).is_err());
    let mut invalid = json.clone();
    invalid["images"][0]["derivation"]["procedure"] = "unknown-resampler".into();
    assert!(serde_json::from_value::<HistoricalCapture>(invalid)
        .unwrap()
        .validate()
        .is_err());
    let mut invalid = json;
    invalid["native_parent"]["affine_bits"][0] = f64::NAN.to_bits().to_string().into();
    assert!(serde_json::from_value::<HistoricalCapture>(invalid)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn same_page_on_different_device_or_adapter_retains_distinct_scope() {
    let device = Uuid::new_v4();
    let document = Uuid::new_v4();
    let page = Uuid::new_v4();
    let first = HistoricalCapture::from_batch(&batch(device, document, page)).unwrap();
    let second = HistoricalCapture::from_batch(&batch(device, document, page)).unwrap();
    let foreign = HistoricalCapture::from_batch(&batch(Uuid::new_v4(), document, page)).unwrap();
    let one = serde_json::to_value(first).unwrap();
    let two = serde_json::to_value(second).unwrap();
    let other = serde_json::to_value(foreign).unwrap();
    assert_eq!(one["source"]["page"], two["source"]["page"]);
    assert_ne!(one["source"]["instance"], two["source"]["instance"]);
    assert_ne!(one["source"]["device"], other["source"]["device"]);
}

#[test]
fn fixture_storage_failures_dispatch_nothing_and_retry_original_bytes_once() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::AfterCommit,
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let conversation = create(&ledger);
        let batch = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let request = request(conversation);
        let operation = Uuid::new_v4();
        let expected = root(&ledger, conversation);
        store.set_fault(fault).unwrap();
        let attempt =
            ledger.prepare_sdk_fixture(operation, expected.clone(), request.clone(), &batch);
        let mut recording_provider = Vec::new();
        if let Ok(prepared) = &attempt {
            recording_provider.push(
                prepared
                    .images
                    .iter()
                    .map(|image| image.bytes.clone())
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            recording_provider.len(),
            usize::from(fault == Fault::AfterCommit)
        );
        let retry = ledger
            .prepare_sdk_fixture(operation, expected, request, &batch)
            .unwrap();
        let verification = ledger.stored_sdk_images(retry.capture).unwrap();
        assert_eq!(
            retry
                .images
                .iter()
                .map(|image| &image.bytes)
                .collect::<Vec<_>>(),
            verification
                .images
                .iter()
                .map(|image| &image.bytes)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            verification
                .images
                .iter()
                .map(|image| &image.bytes[..])
                .collect::<Vec<_>>(),
            batch
                .images()
                .iter()
                .map(|image| image.bytes())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            ledger
                .inspect(conversation, false)
                .unwrap()
                .iter()
                .filter(|record| matches!(record, Record::Turn(_)))
                .count(),
            1
        );
    }
}

#[test]
fn imported_capture_field_tampering_refuses_retrieval_without_effects() {
    for tamper in ["dimensions", "affine", "valid-region"] {
        let fixture = Fixture::new();
        let ledger = Ledger::new(fixture.open());
        let conversation = create(&ledger);
        let prepared = ledger
            .prepare_sdk_fixture(
                Uuid::new_v4(),
                root(&ledger, conversation),
                request(conversation),
                &batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()),
            )
            .unwrap();
        let mut envelope = ledger
            .store()
            .value(Namespace::Source, prepared.capture)
            .unwrap()
            .unwrap();
        envelope.parents = std::collections::BTreeSet::from([envelope.revision_id]);
        envelope.revision_id = Uuid::new_v4();
        envelope.operation_id = Uuid::new_v4();
        let image = &mut envelope.payload["record"]["facts"]["images"][0];
        match tamper {
            "dimensions" => image["dimensions"][0] = 999.into(),
            "affine" => image["affine_bits"][0] = (0.25_f64.to_bits().to_string()).into(),
            "valid-region" => {
                image["valid_region_bits"][2] = (1.0_f64.to_bits().to_string()).into()
            }
            _ => unreachable!(),
        }
        ledger
            .store()
            .commit(vec![envelope], BTreeMap::new())
            .unwrap();
        assert!(
            ledger.stored_sdk_images(prepared.capture).is_err(),
            "{tamper}"
        );
        assert!(ledger.inspect(conversation, false).is_err(), "{tamper}");
    }
}

#[test]
fn required_media_corruption_refuses_older_root_after_reopen() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let conversation = create(&ledger);
    let mut requested = request(conversation);
    requested.mode = Mode::Writer;
    let requested_id = requested.id;
    let prepared = ledger
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            root(&ledger, conversation),
            requested,
            &batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()),
        )
        .unwrap();
    let envelope = ledger
        .store()
        .value(Namespace::Conversation, requested_id)
        .unwrap()
        .unwrap();
    let Record::Turn(mut interpreted) = serde_json::from_value(envelope.payload).unwrap() else {
        panic!("missing turn");
    };
    interpreted.outcome = Outcome::Interpreted;
    interpreted.text = Some("verified fixture request".into());
    ledger
        .advance(
            Uuid::new_v4(),
            root(&ledger, conversation),
            ledger
                .expected(Namespace::Conversation, requested_id)
                .unwrap(),
            interpreted,
        )
        .unwrap();
    let budget = ContextBudget {
        max_text_bytes: 100,
        max_turns: 10,
        provider_token_limit: Some(100),
    };
    let context = ledger.context(conversation, &budget, |_| Ok(4)).unwrap();
    assert_eq!(context.turns[0].mode, Mode::Writer);
    assert_eq!(context.captures.len(), 1);
    assert_eq!(context.captures[0].id, prepared.capture);
    assert!(context.missing_media.is_empty());
    let generation: Uuid = files::json(&ledger.store().paths.data.join("CURRENT"), 128).unwrap();
    let object = ledger
        .store()
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects")
        .join(&prepared.native_parent.media.sha256);
    std::fs::remove_file(object).unwrap();
    assert!(ledger.stored_sdk_images(prepared.capture).is_err());
    assert_eq!(
        ledger
            .context(conversation, &budget, |_| Ok(4))
            .unwrap()
            .missing_media,
        vec![prepared.native_parent.media]
    );
    drop(ledger);
    let ledger = Ledger::new(fixture.open());
    let error = ledger
        .context(conversation, &budget, |_| Ok(4))
        .err()
        .unwrap();
    assert!(error
        .downcast_ref::<IncompleteStore>()
        .is_some_and(|error| error.unavailable_commits >= 1));
    assert!(ledger.stored_sdk_images(prepared.capture).is_err());
    assert!(ledger.create(Uuid::new_v4(), Uuid::new_v4(), 2).is_err());
    assert!(ledger.inspect(conversation, true).is_err());
}

#[test]
fn declared_media_omission_preserves_text_facts_and_deleted_retention_after_reopen() {
    let source_fixture = Fixture::new();
    let source = Ledger::new(source_fixture.open());
    let conversation = create(&source);
    let requested = request(conversation);
    let turn_id = requested.id;
    let prepared = source
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            root(&source, conversation),
            requested,
            &batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()),
        )
        .unwrap();
    let Record::Turn(mut interpreted) = serde_json::from_value(
        source
            .store()
            .value(Namespace::Conversation, turn_id)
            .unwrap()
            .unwrap()
            .payload,
    )
    .unwrap() else {
        panic!("missing turn");
    };
    interpreted.outcome = Outcome::Interpreted;
    interpreted.text = Some("available original text".into());
    source
        .advance(
            Uuid::new_v4(),
            root(&source, conversation),
            source.expected(Namespace::Conversation, turn_id).unwrap(),
            interpreted,
        )
        .unwrap();
    let target_fixture = Fixture::new();
    let target = target_fixture.open();
    let mut projected = source.store().manifests().unwrap();
    for manifest in &mut projected {
        manifest.scope = Scope::SelectedRecords;
        manifest.transaction_id = Uuid::new_v4();
        manifest
            .media
            .retain(|object| object.sha256 != prepared.native_parent.media.sha256);
        for coverage in &mut manifest.media_coverage {
            if coverage.hash() == prepared.native_parent.media.sha256 {
                *coverage = Coverage::Omitted {
                    sha256: prepared.native_parent.media.sha256.clone(),
                    reason: Omission::UnavailableAtSource,
                };
            }
        }
    }
    while !projected.is_empty() {
        let before = projected.len();
        projected.retain(|manifest| {
            let objects = manifest
                .records
                .iter()
                .chain(&manifest.media)
                .map(|object| {
                    (
                        object.sha256.clone(),
                        source.store().read_object(object).unwrap(),
                    )
                })
                .collect();
            target.import(manifest.clone(), objects).is_err()
        });
        assert!(
            projected.len() < before,
            "projection dependencies unresolved"
        );
    }
    drop(target);
    let target = Ledger::new(target_fixture.open());
    assert_eq!(target.store().unavailable_commits().unwrap(), 0);
    let budget = ContextBudget {
        max_text_bytes: 100,
        max_turns: 10,
        provider_token_limit: Some(100),
    };
    let context = target.context(conversation, &budget, |_| Ok(4)).unwrap();
    assert_eq!(
        context.turns[0].text.as_deref(),
        Some("available original text")
    );
    assert_eq!(context.captures.len(), 1);
    assert_eq!(context.missing_media, vec![prepared.native_parent.media]);
    assert!(target.stored_sdk_images(prepared.capture).is_err());
    target
        .delete(
            conversation,
            Uuid::new_v4(),
            root(&target, conversation),
            None,
        )
        .unwrap();
    assert!(target.context(conversation, &budget, |_| Ok(4)).is_err());
    assert!(target
        .inspect(conversation, true)
        .unwrap()
        .iter()
        .any(|record| matches!(record, Record::Capture(_))));
    let retained = target.retained_media(conversation).unwrap();
    assert_eq!(retained.len(), 3);
    assert_eq!(retained.iter().filter(|item| item.available).count(), 2);
}

#[test]
fn nil_operation_refuses_before_any_domain_publication() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let conversation = Uuid::new_v4();
    assert!(ledger.create(conversation, Uuid::nil(), 1).is_err());
    assert!(ledger
        .store()
        .value(Namespace::Conversation, conversation)
        .unwrap()
        .is_none());
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    assert!(ledger
        .prepare_sdk_fixture(
            Uuid::nil(),
            root(&ledger, conversation),
            request(conversation),
            &batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
        )
        .is_err());
    assert_eq!(ledger.inspect(conversation, false).unwrap().len(), 2);
}

#[test]
fn selected_sdk_capture_retrieval_keeps_exact_parent_ordinals_and_no_native_guard() {
    use remarkable_reader_buddy::storage::selection::{SelectionChange, SelectionScope};
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let conversation = create(&ledger);
    let batch = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let original = ledger
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            root(&ledger, conversation),
            request(conversation),
            &batch,
        )
        .unwrap();
    let mut objects = BTreeMap::new();
    let mut records = BTreeMap::new();
    let mut media = BTreeMap::new();
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
        store.clone(),
        SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"sdk selected document"),
            binding_sha256: digest(b"historical only"),
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
    let SelectedCaptureImages::Sdk(actual) = admission
        .capture_images(&publication.token, original.capture)
        .unwrap()
    else {
        panic!("SDK historical origin lost")
    };
    assert_eq!(actual.capture, original.capture);
    assert_eq!(actual.turn, original.turn);
    assert_eq!(actual.native_parent.ordinal, None);
    assert_eq!(actual.native_parent.bytes, original.native_parent.bytes);
    for (ordinal, (actual, expected)) in actual.images.iter().zip(&original.images).enumerate() {
        assert_eq!(actual.ordinal, Some(ordinal as u32));
        assert_eq!(actual.media, expected.media);
        assert_eq!(actual.bytes, expected.bytes);
    }
    // The result contains bytes and IDs only: no live capture batch, observation,
    // qualification guard or Reader effect context is reconstructed.
}
