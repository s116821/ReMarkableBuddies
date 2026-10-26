use super::*;
use serde_json::{json, Value};
#[path = "provider_tests.rs"]
mod provider_tests;

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([0, 0, 0, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn request() -> Value {
    json!({"handwriting_version":1,"command":"confirm",
        "profile":"11111111-1111-4111-8111-111111111111",
        "correction":"22222222-2222-4222-8222-222222222222",
        "operation":"33333333-3333-4333-8333-333333333333", "expected_heads":[],
        "confirmation":"user-confirmed", "original_text":"1O N\n", "corrected_text":"10 N; m/s²\n",
        "confirmed_ms":1, "provenance":{"kind":"caller-confirmed-sample","description":"Synthetic test only.","source":null},
        "sample_png_base64":STANDARD.encode(png(2,1))})
}
fn admitted(value: &Value) -> Result<ValidatedInput> {
    parse(&mut Cursor::new(serde_json::to_vec(value).unwrap()))
}

#[test]
fn literal_recipes_match_independently_frozen_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/handwriting-identity-v1.json"
    ))
    .unwrap();
    let value: Correction = serde_json::from_value(fixture["value"]["input"].clone()).unwrap();
    assert_eq!(
        value.value_digest().unwrap(),
        fixture["value"]["sha256"].as_str().unwrap()
    );
    assert_eq!(
        value.record_id().unwrap().to_string(),
        fixture["record_id"].as_str().unwrap()
    );
    let changed: Correction =
        serde_json::from_value(fixture["changed_value"]["input"].clone()).unwrap();
    assert_eq!(
        changed.value_digest().unwrap(),
        fixture["changed_value"]["sha256"].as_str().unwrap()
    );
    assert!(value.same_original(&changed));
    let op = Uuid::parse_str(fixture["operation"].as_str().unwrap()).unwrap();
    for (n, action) in [Action::Confirm, Action::Update, Action::Forget]
        .into_iter()
        .enumerate()
    {
        let vector = &fixture["mutations"][n];
        let recipe = &vector["input"];
        let expected = serde_json::from_value::<BTreeSet<Uuid>>(recipe[3].clone()).unwrap();
        let (hash, revision) = mutation_identity(
            action,
            op,
            value.record_id().unwrap(),
            &expected,
            recipe[4].as_str(),
        )
        .unwrap();
        assert_eq!(hash, vector["sha256"].as_str().unwrap());
        assert_eq!(
            revision.to_string(),
            vector["revision_id"].as_str().unwrap()
        );
    }
}

#[test]
fn confirm_preserves_exact_sample_and_unicode_with_explicit_provenance() {
    let admitted = admitted(&request()).unwrap();
    let value = admitted.correction().unwrap();
    assert_eq!(value.original_text, "1O N\n");
    assert_eq!(value.corrected_text, "10 N; m/s²\n");
    assert_eq!((value.width, value.height), (2, 1));
    value.verify_sample(admitted.sample().unwrap()).unwrap();
    let mut corrupt = admitted.sample().unwrap().to_vec();
    corrupt[0] ^= 1;
    assert_eq!(
        value.verify_sample(&corrupt),
        Err(Failure::MediaUnavailable)
    );
    let mut changed = value.clone();
    changed.sample.sha256 = digest(&corrupt);
    assert_eq!(
        changed.verify_sample(&corrupt),
        Err(Failure::MediaUnavailable)
    );
    assert!(!value.same_original(&changed));
}

#[test]
fn malformed_duplicate_unknown_and_unconfirmed_input_never_admit() {
    let base = request();
    for field in [
        "confirmation",
        "provenance",
        "sample_png_base64",
        "confirmed_ms",
    ] {
        let mut v = base.clone();
        v.as_object_mut().unwrap().remove(field);
        assert!(admitted(&v).is_err());
    }
    for (key, bad) in [
        ("confirmation", json!("model-guessed")),
        ("profile", json!(Uuid::nil())),
        ("profile", json!("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA")),
        ("extra", json!(true)),
        ("original_text", json!("\u{0000}")),
        ("confirmed_ms", json!(0)),
        ("sample_png_base64", json!("not png")),
    ] {
        let mut v = base.clone();
        v[key] = bad;
        assert!(admitted(&v).is_err());
    }
    let bytes = serde_json::to_string(&base).unwrap();
    let duplicate = format!("{{\"original_text\":\"other\",{}", &bytes[1..]);
    assert!(parse(&mut Cursor::new(duplicate)).is_err());
    let mut v = base;
    v["provenance"]["source"] = json!(["11111111-1111-4111-8111-111111111111"]);
    assert!(admitted(&v).is_err());
}

#[test]
fn input_decode_and_pixel_limits_precede_any_store_boundary() {
    assert_eq!(png_dimensions(&[]), Err(Failure::ImageLimit));
    assert_eq!(
        png_dimensions(&vec![0; MAX_SAMPLE + 1]),
        Err(Failure::ImageLimit)
    );
    assert_eq!(png_dimensions(&png(2049, 1)), Err(Failure::ImageLimit));
    assert!(png_dimensions(b"not an image").is_err());
    assert!(matches!(
        parse(&mut Cursor::new(vec![b' '; MAX_INPUT + 1])),
        Err(Failure::InputLimit)
    ));
    let mut v = request();
    v["handwriting_version"] = json!(2);
    assert!(matches!(admitted(&v), Err(Failure::InvalidVersion)));
    v = request();
    v["sample_png_base64"] = json!("A".repeat(MAX_SAMPLE.div_ceil(3) * 4 + 1));
    assert!(matches!(admitted(&v), Err(Failure::ImageLimit)));
}

#[test]
fn status_is_a_distinct_strict_request_and_nullable_query_text_is_required() {
    let mut v = json!({"command":"status","handwriting_version":1,
        "profile":"11111111-1111-4111-8111-111111111111",
        "correction":"22222222-2222-4222-8222-222222222222",
        "operation":"33333333-3333-4333-8333-333333333333"});
    let parsed = admitted(&v).unwrap();
    assert!(matches!(parsed.request(), Request::Status(_)));
    assert!(parsed.sample().is_none() && parsed.correction().is_none());
    v["expected_heads"] = json!([]);
    assert!(admitted(&v).is_err());
    v = json!({"command":"query","handwriting_version":1,"profile":"11111111-1111-4111-8111-111111111111", "max_matches":16,"max_encoded_bytes":65536});
    assert!(admitted(&v).is_err());
    v["query_text"] = Value::Null;
    assert!(admitted(&v).is_ok());
    v["max_matches"] = json!(17);
    assert!(matches!(admitted(&v), Err(Failure::InvalidBudget)));
}

#[test]
fn typed_envelope_refuses_wrong_namespace_version_identity_and_media() {
    let admitted = admitted(&request()).unwrap();
    let value = admitted.correction().unwrap();
    let operation = Uuid::new_v4();
    let (_, revision) = mutation_identity(
        Action::Confirm,
        operation,
        value.record_id().unwrap(),
        &BTreeSet::new(),
        Some(&value.value_digest().unwrap()),
    )
    .unwrap();
    let envelope = Envelope {
        envelope_version: 1,
        namespace: Namespace::Handwriting,
        domain_schema_version: 1,
        record_id: value.record_id().unwrap(),
        revision_id: revision,
        parents: BTreeSet::new(),
        operation_id: operation,
        actor_id: Uuid::new_v4(),
        kind: Kind::Value,
        payload: serde_json::to_value(value).unwrap(),
        media_descriptors: vec![value.sample.clone()],
    };
    assert!(decode(&envelope).unwrap().is_some());
    for n in 0..4 {
        let mut bad = envelope.clone();
        match n {
            0 => bad.namespace = Namespace::Source,
            1 => bad.domain_schema_version = 2,
            2 => bad.record_id = Uuid::new_v4(),
            _ => bad.media_descriptors.clear(),
        }
        assert!(decode(&bad).is_err());
    }
    let mut bad = envelope;
    bad.payload["kind"] = json!("synthetic-storage-fixture");
    assert!(decode(&bad).is_err());
}
