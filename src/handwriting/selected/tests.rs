use super::*;
use crate::storage::{selection::SelectionChange, Omission, Store, StorePaths, MAX_ITEMS};
use serde_json::{json, Value};

fn owner() -> Owner {
    Owner::new(
        "11111111-1111-4111-8111-111111111111".parse().unwrap(),
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
    )
    .unwrap()
}

fn png() -> Vec<u8> {
    let image =
        image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 1, image::Luma([0])));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn request(command: &str, parent: Option<Uuid>) -> Value {
    let owner = owner();
    let heads: Vec<_> = parent.into_iter().collect();
    let mut input = json!({
        "handwriting_version": 1, "command": command,
        "profile": owner.profile, "correction": owner.correction,
        "operation": Uuid::new_v4(), "expected_heads": heads
    });
    if command != "forget" {
        input["confirmation"] = json!("user-confirmed");
        input["original_text"] = json!("1O N");
        input["corrected_text"] = json!("10 N, m/s²\n");
        input["confirmed_ms"] = json!(1);
        input["sample_png_base64"] = json!(STANDARD.encode(png()));
        input["provenance"] = json!({
            "kind": "caller-confirmed-sample", "description": "Fixture only",
            "source": [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()]
        });
    }
    input
}

fn prepare(actor: Uuid, input: &Value) -> Result<Mutation> {
    Mutation::prepare(
        owner(),
        actor,
        parse(&mut Cursor::new(serde_json::to_vec(input).unwrap()))?,
    )
}

fn closure(envelopes: &[&Envelope], coverage: Coverage) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut records = Vec::new();
    let mut record_namespaces = BTreeMap::new();
    let mut objects = BTreeMap::new();
    for envelope in envelopes {
        let bytes = serde_json::to_vec_pretty(envelope).unwrap();
        let sha256 = digest(&bytes);
        records.push(ObjectRef {
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        });
        record_namespaces.insert(sha256.clone(), Namespace::Handwriting);
        objects.insert(sha256, bytes);
    }
    let mut media = Vec::new();
    if matches!(coverage, Coverage::Included { .. }) {
        let descriptor = envelopes
            .iter()
            .flat_map(|e| &e.media_descriptors)
            .next()
            .unwrap();
        media.push(ObjectRef {
            sha256: descriptor.sha256.clone(),
            bytes: descriptor.bytes,
        });
        objects.insert(descriptor.sha256.clone(), png());
    }
    (
        Manifest {
            format: 1,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records,
            record_namespaces,
            media,
            media_coverage: vec![coverage],
        },
        objects,
    )
}

fn included() -> Coverage {
    Coverage::Included {
        sha256: digest(&png()),
    }
}

fn read(
    owner: Owner,
    manifest: &Manifest,
    objects: &BTreeMap<String, Vec<u8>>,
) -> Result<Projection> {
    Projection::read(owner, manifest, |r| {
        objects
            .get(&r.sha256)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing fixture object"))
    })
}

fn edit(base: &Value, parent: Uuid, text: &str) -> Value {
    let mut input = base.clone();
    input["command"] = json!("update");
    input["operation"] = json!(Uuid::new_v4());
    input["expected_heads"] = json!([parent]);
    input["corrected_text"] = json!(text);
    input
}

#[test]
fn typed_mutation_preserves_original_across_tombstone_and_rollback() {
    let actor = Uuid::new_v4();
    let input = request("confirm", None);
    let original = prepare(actor, &input).unwrap();
    let repeat = prepare(actor, &input).unwrap();
    assert_eq!(original.fingerprint(), repeat.fingerprint());
    assert!(original.envelope() == repeat.envelope());
    assert_eq!(original.check_predecessor(None), Ok(()));
    let first = original.envelope().revision_id;
    let (manifest, objects) = closure(&[original.envelope()], included());
    let current = read(owner(), &manifest, &objects).unwrap();
    assert_eq!(
        original.check_predecessor(Some(&current)),
        Err(Failure::HeadsConflict)
    );

    let mut canceled = input.clone();
    canceled["confirmation"] = json!("canceled");
    assert_eq!(
        prepare(actor, &canceled).err(),
        Some(Failure::InvalidConfirmation)
    );
    canceled["confirmation"] = json!("user-confirmed");
    canceled["original_text"] = Value::Null;
    assert!(prepare(actor, &canceled).is_err());
    let mut enrollment =
        read_request(&mut Cursor::new(serde_json::to_vec(&input).unwrap())).unwrap();
    let Request::Confirm(confirmation) = &mut enrollment else {
        unreachable!("expected confirmation fixture");
    };
    confirmation.original_text = None;
    confirmation.provenance.kind = ProvenanceKind::CallerConfirmedAfterAbstention;
    let ordinary = validate_enrollment_request(enrollment).unwrap();
    let (action, operation, expected) = ordinary.mutation().unwrap();
    let (_, abstention) = mutation_envelope(
        action,
        operation,
        owner().record_id(),
        expected,
        ordinary.correction(),
        actor,
    )
    .unwrap();
    assert_eq!(
        Mutation::prepare(owner(), actor, ordinary).err(),
        Some(Failure::InvalidText)
    );
    let (manifest, objects) = closure(&[&abstention], included());
    assert_eq!(
        read(owner(), &manifest, &objects).err(),
        Some(Failure::UnsupportedRecord)
    );

    let forget = prepare(actor, &request("forget", Some(first))).unwrap();
    assert_eq!(forget.check_predecessor(None), Err(Failure::HeadsConflict));
    assert_eq!(forget.check_predecessor(Some(&current)), Ok(()));
    let tombstone = forget.envelope().revision_id;
    let (manifest, objects) = closure(&[forget.envelope(), original.envelope()], included());
    let removed = read(owner(), &manifest, &objects).unwrap();
    assert_eq!(removed.head(), tombstone);
    assert!(removed.current().is_none() && removed.sample().is_none());

    let mut rollback = edit(&input, tombstone, "10 N, m/s²\n");
    let confirmed = prepare(actor, &rollback).unwrap();
    assert_eq!(confirmed.check_predecessor(Some(&removed)), Ok(()));
    assert_ne!(confirmed.envelope().revision_id, first);
    rollback["original_text"] = json!("changed");
    let changed = prepare(actor, &rollback).unwrap();
    assert_eq!(
        changed.check_predecessor(Some(&removed)),
        Err(Failure::ImmutableChange)
    );
    let (invalid, objects) = closure(
        &[changed.envelope(), forget.envelope(), original.envelope()],
        included(),
    );
    assert_eq!(
        read(owner(), &invalid, &objects).err(),
        Some(Failure::UnsupportedRecord)
    );
}

#[test]
fn raw_bytes_ownership_lineage_and_required_sample_are_checked() {
    let actor = Uuid::new_v4();
    let input = request("confirm", None);
    let first = prepare(actor, &input).unwrap();
    let next = prepare(
        actor,
        &edit(&input, first.envelope().revision_id, "confirmed edit"),
    )
    .unwrap();
    let (manifest, mut objects) = closure(&[next.envelope(), first.envelope()], included());
    let valid = read(owner(), &manifest, &objects).unwrap();
    assert_eq!(valid.sample(), Some(png().as_slice()));
    assert_eq!(valid.current().unwrap().corrected_text, "confirmed edit");
    assert_ne!(
        manifest.records[0].sha256,
        digest(&serde_json::to_vec(next.envelope()).unwrap())
    );
    let foreign = Owner::new(Uuid::new_v4(), owner().correction).unwrap();
    assert_ne!(
        foreign
            .scope(Uuid::new_v4(), digest(b"binding"))
            .unwrap()
            .key_sha256,
        owner()
            .scope(Uuid::new_v4(), digest(b"binding"))
            .unwrap()
            .key_sha256
    );
    assert_eq!(
        read(foreign, &manifest, &objects).err(),
        Some(Failure::UnsupportedRecord)
    );

    objects.remove(&manifest.media[0].sha256);
    assert_eq!(
        read(owner(), &manifest, &objects).err(),
        Some(Failure::MediaUnavailable)
    );
    objects.insert(
        manifest.media[0].sha256.clone(),
        vec![0; manifest.media[0].bytes as usize],
    );
    assert_eq!(
        read(owner(), &manifest, &objects).err(),
        Some(Failure::MediaUnavailable)
    );

    let (orphan, objects) = closure(&[next.envelope()], included());
    assert_eq!(
        read(owner(), &orphan, &objects).err(),
        Some(Failure::UnsupportedRecord)
    );
    let sibling = prepare(
        actor,
        &edit(&input, first.envelope().revision_id, "competing"),
    )
    .unwrap();
    let (branched, objects) = closure(
        &[next.envelope(), sibling.envelope(), first.envelope()],
        included(),
    );
    assert_eq!(
        read(owner(), &branched, &objects).err(),
        Some(Failure::UnsupportedRecord)
    );

    let (mut duplicate, mut objects) = closure(&[first.envelope()], included());
    let previous = duplicate.records.remove(0);
    let raw = String::from_utf8(objects.remove(&previous.sha256).unwrap())
        .unwrap()
        .replace(
            "\"original_text\":",
            "\"original_text\": \"unconfirmed\", \"original_text\":",
        );
    let bytes = raw.into_bytes();
    let sha256 = digest(&bytes);
    duplicate.records.push(ObjectRef {
        sha256: sha256.clone(),
        bytes: bytes.len() as u64,
    });
    duplicate.record_namespaces.remove(&previous.sha256);
    duplicate
        .record_namespaces
        .insert(sha256.clone(), Namespace::Handwriting);
    objects.insert(sha256, bytes);
    assert_eq!(
        read(owner(), &duplicate, &objects).err(),
        Some(Failure::UnsupportedRecord)
    );
}

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn real_selected_store_keeps_retained_media_out_of_omitted_winner() {
    let fixture = Fixture(
        std::env::temp_dir().join(format!("buddy-handwriting-selected-{}", Uuid::new_v4())),
    );
    let paths = StorePaths {
        data: fixture.0.join("data"),
        cache: fixture.0.join("cache"),
        credentials: fixture.0.join("credentials"),
    };
    let store = Store::open(paths.clone()).unwrap();
    let input = request("confirm", None);
    let original = prepare(store.actor_id, &input).unwrap();
    let scope = owner()
        .scope(Uuid::new_v4(), digest(b"fixture binding"))
        .unwrap();
    let first = original.envelope();
    let (manifest, mut objects) = closure(&[first], included());
    let (_, sample) = prepare(store.actor_id, &input).unwrap().into_parts();
    assert_eq!(
        objects[&first.media_descriptors[0].sha256],
        sample[&first.media_descriptors[0].sha256]
    );
    let initial = store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: first.operation_id,
                accepted_base_sha256: digest(b"genesis"),
                selected: manifest,
                retained: vec![],
            },
            std::mem::take(&mut objects),
        )
        .unwrap();

    let local = prepare(
        store.actor_id,
        &edit(&input, first.revision_id, "local sibling"),
    )
    .unwrap();
    let remote = prepare(
        Uuid::new_v4(),
        &edit(&input, first.revision_id, "remote sibling"),
    )
    .unwrap();
    let initial_view = Projection::read(owner(), &initial.transaction.selected, |r| {
        store.read_object(r)
    })
    .unwrap();
    local.check_predecessor(Some(&initial_view)).unwrap();
    let (record, sample) = local.into_parts();
    let local_head = record.revision_id;
    let local = store
        .commit_selected(&initial.token, record.operation_id, vec![record], sample)
        .unwrap();
    let omitted = Coverage::Omitted {
        sha256: digest(&png()),
        reason: Omission::PolicyDisabled,
    };
    let (winner, objects) = closure(&[remote.envelope(), first], omitted.clone());
    store
        .activate_selected(
            &local.token,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"winner"),
                selected: winner,
                retained: vec![local.transaction.selected],
            },
            objects,
        )
        .unwrap();
    drop(store);

    let reopened = Store::open(paths).unwrap();
    let snapshot = reopened
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    let current = Projection::read(owner(), &snapshot.transaction.selected, |r| {
        assert!(snapshot.transaction.selected.records.contains(r));
        reopened.read_object(r)
    })
    .unwrap();
    assert_eq!(current.coverage(), &omitted);
    assert_eq!(current.current().unwrap().corrected_text, "remote sibling");
    assert!(current.sample().is_none());
    let history = Projection::read(owner(), &snapshot.transaction.retained[0], |r| {
        reopened.read_object(r)
    })
    .unwrap();
    assert_eq!(history.head(), local_head);
    assert_eq!(history.sample(), Some(png().as_slice()));
}
