//! Disconnected selected-reference prototype against real persisted Store objects.
//! No selected activation, domain decoding, operation admission or worker is added.
use super::*;
use serde_json::json;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-projection-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Store {
        Store::open(StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        })
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn envelope(store: &Store) -> Envelope {
    Envelope {
        envelope_version: FORMAT,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"opaque_fixture": "not a domain schema"}),
        media_descriptors: vec![],
    }
}
fn committed(store: &Store, record: Envelope, media: BTreeMap<String, Vec<u8>>) -> Manifest {
    let id = store.commit(vec![record], media).unwrap();
    store
        .manifests()
        .unwrap()
        .into_iter()
        .find(|m| m.transaction_id == id)
        .unwrap()
}

struct PinnedProjection {
    generation: Uuid,
    records: Vec<Envelope>,
    media: BTreeMap<String, files::Source>,
}
/// Test-only prototype: caller supplies explicit opaque references, not a document
/// identity or permission. Pin one actual store generation and its committed
/// references under one store lock. Domain closure/ownership remains Main's work.
fn pin(store: &Store, selected: &Manifest) -> Result<PinnedProjection> {
    selected.validate()?;
    let inner = store
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("store unavailable"))?;
    let root = store.generation(inner.generation).join("objects");
    let mut records = Vec::new();
    let mut media = BTreeMap::new();
    for reference in selected.records.iter().chain(&selected.media) {
        ensure!(
            inner.index.manifests.values().any(|m| m
                .records
                .iter()
                .chain(&m.media)
                .any(|r| r == reference)),
            "uncommitted projection reference"
        );
        let mut source = files::Source::open(&root.join(&reference.sha256), reference)?;
        if selected.records.contains(reference) {
            let record: Envelope = serde_json::from_slice(&source.chunk(0, source.len())?)?;
            record.validate()?;
            ensure!(
                selected.record_namespaces.get(&reference.sha256) == Some(&record.namespace),
                "projection namespace mismatch"
            );
            for descriptor in &record.media_descriptors {
                ensure!(
                    selected
                        .media_coverage
                        .iter()
                        .any(|coverage| coverage.hash() == descriptor.sha256),
                    "projection media coverage absent"
                );
                if selected.media_coverage.iter().any(|coverage| matches!(coverage, Coverage::Included { sha256 } if sha256 == &descriptor.sha256)) {
                    ensure!(selected.media.iter().any(|item| item.sha256 == descriptor.sha256 && item.bytes == descriptor.bytes), "projection included descriptor mismatch");
                }
            }
            records.push(record);
        } else {
            media.insert(reference.sha256.clone(), source);
        }
    }
    Ok(PinnedProjection {
        generation: inner.generation,
        records,
        media,
    })
}

#[test]
fn explicit_references_preserve_opaque_bytes_after_heads_advance() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let original = envelope(&store);
    let manifest = committed(&store, original.clone(), BTreeMap::new());
    let selected = pin(&store, &manifest).unwrap();
    let mut next = original.clone();
    next.revision_id = Uuid::new_v4();
    next.operation_id = Uuid::new_v4();
    next.parents.insert(original.revision_id);
    next.payload = json!({"opaque_fixture":"new history"});
    committed(&store, next.clone(), BTreeMap::new());
    assert!(store.value(original.namespace, original.record_id).unwrap() == Some(next));
    assert!(selected.records == vec![original]);
    assert_eq!(selected.generation, store.inner.lock().unwrap().generation);
}

#[test]
fn imported_loser_and_unrelated_records_do_not_enter_explicit_projection() {
    let a = Fixture::new();
    let b = Fixture::new();
    let store = a.open();
    let other = b.open();
    let original = envelope(&store);
    let base = committed(&store, original.clone(), BTreeMap::new());
    let objects = base
        .records
        .iter()
        .map(|r| (r.sha256.clone(), store.read_object(r).unwrap()))
        .collect();
    other.import(base, objects).unwrap();
    let mut left = original.clone();
    left.revision_id = Uuid::new_v4();
    left.operation_id = Uuid::new_v4();
    left.parents.insert(original.revision_id);
    let winner = committed(&store, left.clone(), BTreeMap::new());
    let mut right = left.clone();
    right.actor_id = other.actor_id;
    right.revision_id = Uuid::new_v4();
    right.operation_id = Uuid::new_v4();
    let loser = committed(&other, right, BTreeMap::new());
    let objects = loser
        .records
        .iter()
        .map(|r| (r.sha256.clone(), other.read_object(r).unwrap()))
        .collect();
    store.import(loser, objects).unwrap();
    let mut unrelated = envelope(&store);
    unrelated.namespace = Namespace::SubjectMemory;
    committed(&store, unrelated.clone(), BTreeMap::new());
    assert_eq!(
        store
            .heads(original.namespace, original.record_id)
            .unwrap()
            .len(),
        2
    );
    assert!(pin(&store, &winner).unwrap().records == vec![left]);
    assert!(
        store
            .value(unrelated.namespace, unrelated.record_id)
            .unwrap()
            == Some(unrelated)
    );
    // The all-history index is deliberately not rewritten by this fixture.
}

#[test]
fn retained_intent_media_remains_separate_and_readable_after_reopen() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"owned unresolved evidence".to_vec();
    let hash = digest(&bytes);
    let mut intent = envelope(&store);
    intent.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let retained = committed(
        &store,
        intent.clone(),
        BTreeMap::from([(hash.clone(), bytes.clone())]),
    );
    let winner = committed(&store, envelope(&store), BTreeMap::new());
    let selected = pin(&store, &winner).unwrap();
    assert!(selected
        .records
        .iter()
        .all(|r| r.record_id != intent.record_id));
    drop(store);
    let reopened = fixture.open();
    let mut evidence = pin(&reopened, &retained).unwrap();
    assert!(evidence.records == vec![intent]);
    let source = evidence.media.get_mut(&hash).unwrap();
    assert_eq!(source.chunk(0, source.len()).unwrap(), bytes);
    // Reopening existing facts does not implement a selected activation journal.
}

#[test]
fn corrupt_included_media_refuses_and_explicit_omission_stays_distinct() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"owned media".to_vec();
    let hash = digest(&bytes);
    let mut record = envelope(&store);
    record.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let complete = committed(&store, record, BTreeMap::from([(hash.clone(), bytes)]));
    let mut omitted = complete.clone();
    omitted.media.clear();
    omitted.media_coverage = vec![Coverage::Omitted {
        sha256: hash.clone(),
        reason: Omission::PolicyDisabled,
    }];
    assert!(pin(&store, &omitted).unwrap().media.is_empty());
    let generation = store.inner.lock().unwrap().generation;
    fs::write(
        store.generation(generation).join("objects").join(hash),
        b"corrupt",
    )
    .unwrap();
    assert!(pin(&store, &complete).is_err());
    assert!(pin(&store, &omitted).unwrap().media.is_empty());
    omitted.media_coverage.clear();
    assert!(pin(&store, &omitted).is_err());
}

#[test]
fn captured_media_handle_does_not_follow_a_new_head_descriptor() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"owned first image".to_vec();
    let hash = digest(&bytes);
    let mut original = envelope(&store);
    original.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    });
    let manifest = committed(
        &store,
        original.clone(),
        BTreeMap::from([(hash.clone(), bytes.clone())]),
    );
    let mut captured = pin(&store, &manifest).unwrap();
    let replacement = b"owned later image".to_vec();
    let replacement_hash = digest(&replacement);
    let mut next = original.clone();
    next.revision_id = Uuid::new_v4();
    next.operation_id = Uuid::new_v4();
    next.parents.insert(original.revision_id);
    next.media_descriptors[0].sha256 = replacement_hash.clone();
    next.media_descriptors[0].bytes = replacement.len() as u64;
    committed(
        &store,
        next,
        BTreeMap::from([(replacement_hash, replacement)]),
    );
    let source = captured.media.get_mut(&hash).unwrap();
    assert_eq!(source.chunk(0, source.len()).unwrap(), bytes);
    assert!(captured.records == vec![original]);
}

#[test]
fn uncommitted_or_namespace_mismatched_projection_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut selected = committed(&store, envelope(&store), BTreeMap::new());
    let hash = selected.records[0].sha256.clone();
    selected
        .record_namespaces
        .insert(hash, Namespace::Handwriting);
    assert!(pin(&store, &selected).is_err());
    selected.records[0].sha256 = "a".repeat(64);
    selected.record_namespaces = BTreeMap::from([("a".repeat(64), Namespace::Conversation)]);
    assert!(pin(&store, &selected).is_err());
}
