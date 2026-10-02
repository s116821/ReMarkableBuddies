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
