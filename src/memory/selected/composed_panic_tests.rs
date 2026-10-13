use crate::{
    conversation::{
        capture_facts::{HistoricalCapture, SDK_SOURCE},
        *,
    },
    storage::*,
};
use remarkable_open_sdk::{
    capture::{synthetic::*, *},
    mock::MockPlatform,
    Platform,
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
    select_fixture_scope_filtered(store, scope, None)
}
fn select_fixture_scope_filtered(
    store: &Arc<Store>,
    scope: selection::SelectionScope,
    conversation: Option<Uuid>,
) -> (Arc<SelectedAdmission>, selection::SelectionToken) {
    let mut objects = BTreeMap::new();
    let mut records = BTreeMap::new();
    let mut namespaces = BTreeMap::new();
    let mut media = BTreeMap::new();
    for manifest in store.manifests().unwrap() {
        for reference in manifest.records {
            if let Some(conversation) = conversation {
                let envelope: Envelope =
                    serde_json::from_slice(&store.read_object(&reference).unwrap()).unwrap();
                if !matches!(
                    envelope.namespace,
                    Namespace::Conversation | Namespace::Source
                ) {
                    continue;
                }
                let value: Record = serde_json::from_value(envelope.payload).unwrap();
                let owner = match value {
                    Record::Root(root) => root.id,
                    Record::Turn(turn) => turn.conversation,
                    Record::Capture(capture) => capture.conversation,
                    Record::Receipt(receipt) => receipt.acknowledgment.conversation,
                    _ => panic!("unexpected opposing fixture record"),
                };
                if owner != conversation {
                    continue;
                }
            }

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

fn create(ledger: &Ledger) -> Uuid {
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 1).unwrap();
    id
}
struct PanicArm;
impl PanicArm {
    fn new() -> Self {
        super::PANIC_AFTER_CONSUMPTION_WITNESSED.with(|value| value.set(false));
        super::PANIC_AFTER_CONSUMPTION.with(|value| assert!(!value.replace(true)));
        Self
    }
}
impl Drop for PanicArm {
    fn drop(&mut self) {
        super::PANIC_AFTER_CONSUMPTION.with(|value| value.set(false));
    }
}
#[test]
fn composed_post_consumption_panic_is_sticky_without_publication() {
    const FILTER: &str = "memory::selected::composed_panic_tests::composed_post_consumption_panic_is_sticky_without_publication";
    if std::env::var_os("BUDDY_COMPOSED_PANIC_CHILD").is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FILTER, "--nocapture"])
            .env("BUDDY_COMPOSED_PANIC_CHILD", "1")
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
                panic!("composed panic witness child deadline");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    }
    use crate::memory::{self, Provider};
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
    let registrations = vec![
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

    let before = serde_json::to_vec(&store.manifests().unwrap()).unwrap();
    let media: Vec<_> = store
        .manifests()
        .unwrap()
        .iter()
        .flat_map(|manifest| manifest.media.clone())
        .map(|reference| {
            let bytes = store.read_object(&reference).unwrap();
            (reference, bytes)
        })
        .collect();
    let fresh_view = memory.query(&query).unwrap();
    let arm = PanicArm::new();
    let failure = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        memory.publish_selected_reader_text(&view, &reader, &publication.token, draft.clone())
    })) {
        Err(failure) => failure,
        Ok(_) => panic!("armed joined callback must panic"),
    };
    let message = failure
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| failure.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("composed post-consumption panic witness"));
    assert!(super::PANIC_AFTER_CONSUMPTION_WITNESSED.with(|value| value.get()));
    drop(arm);
    let same = memory
        .publish_selected_reader_text(&view, &reader, &publication.token, draft.clone())
        .err()
        .expect("poisoned same receiving view must refuse");
    assert!(format!("{same:#}").contains("selected memory pins unavailable"));
    let another_reader = SelectedAdmission::new(store.clone(), initial.scope().clone()).unwrap();
    assert!(memory
        .publish_selected_reader_text(
            &fresh_view,
            &another_reader,
            &publication.token,
            draft.clone()
        )
        .is_err());
    assert_eq!(
        serde_json::to_vec(&store.manifests().unwrap()).unwrap(),
        before
    );
    assert!(!ledger
        .inspect(conversation, true)
        .unwrap()
        .iter()
        .any(|record| matches!(record,Record::Turn(turn) if turn.role==Role::Assistant)));
    for (reference, bytes) in media {
        assert_eq!(store.read_object(&reference).unwrap(), bytes);
    }
    println!(
        "COMPOSED_PANIC_WITNESS: post-consumption panic, poisoned retries, zero publication, exact media preserved"
    );
}
