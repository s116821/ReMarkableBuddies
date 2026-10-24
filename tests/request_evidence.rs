//! Real local Store/Index fixtures. Counters are declared synthetic byte counts.
use anyhow::Result;
use remarkable_reader_buddy::{
    conversation::{evidence::*, receiving::*, *},
    retrieval::{
        Index, IndexKey, NormalizedDocument, Provider, Retrieved, SourceFormat, SourceUnit,
    },
    storage::*,
};
use std::{cell::RefCell, collections::BTreeMap, fs, io::Cursor, path::PathBuf, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-evidence-{}", Uuid::new_v4())))
    }
    fn ledger(&self) -> Ledger {
        Ledger::new(Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.exists() {
            let resolved = self.0.canonicalize().unwrap();
            assert_eq!(
                resolved.parent(),
                Some(std::env::temp_dir().canonicalize().unwrap().as_path())
            );
            assert!(resolved
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("buddy-evidence-"));
            fs::remove_dir_all(resolved).unwrap();
        }
    }
}
fn create(ledger: &Ledger) -> Uuid {
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 1).unwrap();
    id
}
fn heads(ledger: &Ledger, id: Uuid) -> ExpectedHeads {
    ledger.expected(Namespace::Conversation, id).unwrap()
}
fn user(conversation: Uuid, text: &str) -> Turn {
    Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange: Uuid::new_v4(),
        sequence: 0,
        role: Role::User,
        mode: Mode::Reader,
        outcome: Outcome::Interpreted,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    }
}
fn observed(document: Uuid) -> SourceObservation {
    SourceObservation {
        document,
        page: Uuid::new_v4(),
        session: "fixture".into(),
        visit: "fixture".into(),
        content_revision: Some("historical-not-content-digest".into()),
        order_revision: Some("fixture-order".into()),
        capability_revision: "fixture".into(),
        evidence_procedure: "unqualified-fixture".into(),
    }
}
fn append(ledger: &Ledger, turn: Turn, source: Vec<ImageUse>) -> Turn {
    let id = turn.id;
    let conversation = turn.conversation;
    ledger
        .append(Uuid::new_v4(), heads(ledger, conversation), turn, source)
        .unwrap();
    ledger
        .inspect(conversation, false)
        .unwrap()
        .into_iter()
        .find_map(|r| match r {
            Record::Turn(t) if t.id == id => Some(t),
            _ => None,
        })
        .unwrap()
}
fn with_source(ledger: &Ledger, mut turn: Turn, document: Uuid) -> (Turn, ImageUse) {
    let mut bytes = Cursor::new(Vec::new());
    let pixels = document
        .as_bytes()
        .iter()
        .copied()
        .chain(turn.id.as_bytes()[..2].iter().copied())
        .collect();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(2, 3, pixels).unwrap())
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let media = Media {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    };
    ledger.stage_image(&media, bytes.as_slice()).unwrap();
    let source = ImageUse {
        id: Uuid::new_v4(),
        conversation: turn.conversation,
        turn: turn.id,
        image: media,
        observation: observed(document),
        width: 2,
        height: 3,
        purpose: "current request image".into(),
        ordinal: 0,
        parent: None,
        parent_dimensions: None,
        crop: None,
        transform: [1., 0., 0., 1., 0., 0.],
        target: None,
        captured_ms: 1,
    };
    turn.sources = vec![source.id];
    let turn = append(ledger, turn, vec![source.clone()]);
    (turn, source)
}
fn document(id: Uuid, revision: &str) -> NormalizedDocument {
    NormalizedDocument {
        document: id,
        format: SourceFormat::Pdf,
        source_sha256: digest(revision.as_bytes()),
        extractor_revision: "fixture-text-v1".into(),
        units: vec![
            SourceUnit {
                page: Some(1),
                section: None,
                text: "café acceleration context".into(),
            },
            SourceUnit {
                page: Some(9),
                section: None,
                text: "orbital orbit context".into(),
            },
        ],
    }
}
fn budget() -> ReceivingBudget {
    ReceivingBudget {
        context: ContextBudget {
            max_text_bytes: 65536,
            max_turns: 64,
            provider_token_limit: Some(1_000_000),
        },
        max_encoded_bytes: 524288,
        max_passages: 2,
        max_passage_bytes: 16384,
        neighbors: false,
    }
}
fn counter(text: &str) -> Result<usize> {
    Ok(text.len())
}
fn association(origin: &Turn, source: &ImageUse, key: &IndexKey) -> RequestSource {
    RequestSource::Original {
        origin_turn: origin.id,
        source_record: source.id,
        expected: key.clone(),
    }
}
struct Spy<'a> {
    index: &'a Index,
    calls: RefCell<Vec<(IndexKey, String, usize)>>,
}
impl<'a> Spy<'a> {
    fn new(index: &'a Index) -> Self {
        Self {
            index,
            calls: RefCell::new(Vec::new()),
        }
    }
}
impl Provider for Spy<'_> {
    fn query(
        &self,
        key: &IndexKey,
        text: &str,
        count: usize,
        bytes: usize,
        neighbors: bool,
    ) -> Result<Retrieved> {
        self.calls
            .borrow_mut()
            .push((key.clone(), text.into(), bytes));
        self.index.query(key, text, count, bytes, neighbors)
    }
}

fn no_document(ledger: &Ledger, request: &Turn) -> PreparedRequestEvidence {
    ledger
        .prepare_request_evidence(
            request.conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap()
}
fn evidence_count(ledger: &Ledger, id: Uuid) -> usize {
    ledger
        .inspect(id, true)
        .unwrap()
        .iter()
        .filter(|r| matches!(r, Record::RequestEvidence(_)))
        .count()
}
fn replicate(source: &Store, target: &Store, omit: bool, pretty: bool) {
    let mut pending = source
        .manifests()
        .unwrap()
        .into_iter()
        .map(|mut m| {
            let mut objects = BTreeMap::new();
            for r in &mut m.records {
                let mut bytes = source.read_object(r).unwrap();
                if pretty {
                    let e: Envelope = serde_json::from_slice(&bytes).unwrap();
                    bytes = serde_json::to_vec_pretty(&e).unwrap();
                }
                let old = r.sha256.clone();
                r.sha256 = digest(&bytes);
                r.bytes = bytes.len() as u64;
                if let Some(ns) = m.record_namespaces.remove(&old) {
                    m.record_namespaces.insert(r.sha256.clone(), ns);
                }
                objects.insert(r.sha256.clone(), bytes);
            }
            if omit {
                m.media.clear();
                for coverage in &mut m.media_coverage {
                    *coverage = Coverage::Omitted {
                        sha256: coverage.hash().to_owned(),
                        reason: Omission::UnavailableAtSource,
                    };
                }
            } else {
                for r in &m.media {
                    objects.insert(r.sha256.clone(), source.read_object(r).unwrap());
                }
            }
            (m, objects)
        })
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let n = pending.len();
        pending.retain(|(m, objects)| target.import(m.clone(), objects.clone()).is_err());
        assert!(pending.len() < n);
    }
}

#[test]
fn bound_frame_survives_restart_and_unchanged_receiving_encoding() {
    let f = Fixture::new();
    let ledger = f.ledger();
    let c = create(&ledger);
    let doc = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(c, "orbit"), doc);
    let index = Index::build(document(doc, "v1")).unwrap();
    let spy = Spy::new(&index);
    let old = ledger
        .source_request_context(
            c,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let prepared = ledger
        .prepare_request_evidence(
            c,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert_eq!(old.encoded_json(), prepared.context().encoded_json());
    let json = prepared.context().encoded_json().to_owned();
    let op = Uuid::new_v4();
    let ack = ledger
        .persist_request_evidence(op, 7, &prepared)
        .unwrap()
        .historical;
    assert_eq!(spy.calls.borrow().len(), 1);
    assert_eq!(evidence_count(&ledger, c), 1);
    drop(ledger); // Weak preparation must not retain the OS lease.
    let reopened = f.ledger();
    let read = reopened
        .request_evidence(c, request.id, op)
        .unwrap()
        .unwrap();
    assert_eq!(read.record.frame_json, json);
    assert_eq!(read.acknowledgment, ack);
    assert!(read.unavailable_media.is_empty());
    assert_eq!(read.record.limits, budget());
    assert_eq!(
        reopened
            .persist_request_evidence(op, 7, &prepared)
            .unwrap()
            .historical,
        ack
    );
    let freshly_captured = reopened
        .prepare_request_evidence(
            c,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert_eq!(
        reopened
            .persist_request_evidence(op, 7, &freshly_captured)
            .unwrap()
            .historical,
        ack
    );
    assert_eq!(
        reopened
            .request_evidence(c, request.id, op)
            .unwrap()
            .unwrap()
            .record,
        read.record
    );

    assert!(reopened
        .persist_request_evidence(Uuid::new_v4(), 7, &prepared)
        .is_err());
    assert_eq!(spy.calls.borrow().len(), 1);
}

#[test]
fn explicit_no_document_queries_zero_and_zero_matches_remain_bound() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let doc = Uuid::new_v4();
    let (r, s) = with_source(&l, user(c, "unmatchedquestion"), doc);
    let index = Index::build(document(doc, "v1")).unwrap();
    let spy = Spy::new(&index);
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            RequestSource::NoDocument,
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert!(spy.calls.borrow().is_empty());
    assert!(p.context().retrieval().is_none());
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 1, &p).unwrap();
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            association(&r, &s, index.key()),
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert!(p.context().retrieval().unwrap().passages.is_empty());
    let bound = Uuid::new_v4();
    l.persist_request_evidence(bound, 2, &p).unwrap();
    assert_ne!(
        l.request_evidence(c, r.id, op)
            .unwrap()
            .unwrap()
            .record
            .frame_json,
        l.request_evidence(c, r.id, bound)
            .unwrap()
            .unwrap()
            .record
            .frame_json
    );
    assert_eq!(spy.calls.borrow().len(), 1);
}

#[test]
fn fresh_followup_uses_explicit_original_outside_range_and_changed_source_isolated() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let doc = Uuid::new_v4();
    let (origin, source) = with_source(&l, user(c, "acceleration"), doc);
    let (request, other) = with_source(&l, user(c, "orbit"), Uuid::new_v4());
    let index = Index::build(document(doc, "v1")).unwrap();
    let selection = Some(TurnRange {
        start: request.sequence,
        end_exclusive: request.sequence + 1,
    });
    let p = l
        .prepare_request_evidence(
            c,
            request.id,
            selection,
            association(&origin, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 5, &p).unwrap();
    let fact = l
        .request_evidence(c, request.id, op)
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(fact.turns.len(), 2);
    assert_eq!(fact.sources.len(), 2);
    assert!(p.context().history().is_empty());
    let other_index = Index::build(document(other.observation.document, "v2")).unwrap();
    let p2 = l
        .prepare_request_evidence(
            c,
            request.id,
            selection,
            association(&request, &other, other_index.key()),
            budget(),
            Some(&other_index),
            counter,
        )
        .unwrap();
    assert!(l.persist_request_evidence(op, 5, &p2).is_err());
    let second = Uuid::new_v4();
    l.persist_request_evidence(second, 5, &p2).unwrap();
    assert_eq!(
        l.request_evidence(c, request.id, op)
            .unwrap()
            .unwrap()
            .record,
        fact
    );
    assert_eq!(evidence_count(&l, c), 2);
}

#[test]
fn relevant_same_content_revision_and_generation_changes_refuse_but_unrelated_writes_do_not() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let r = append(&l, user(c, "orbit"), vec![]);
    let p = no_document(&l, &r);
    create(&l);
    l.persist_request_evidence(Uuid::new_v4(), 1, &p).unwrap();
    let p = no_document(&l, &r);
    let mut changed = l
        .store()
        .value(Namespace::Conversation, r.id)
        .unwrap()
        .unwrap();
    changed.parents = std::collections::BTreeSet::from([changed.revision_id]);
    changed.revision_id = Uuid::new_v4();
    changed.operation_id = Uuid::new_v4();
    l.store().commit(vec![changed], BTreeMap::new()).unwrap();
    assert!(l.persist_request_evidence(Uuid::new_v4(), 1, &p).is_err());
    let p = no_document(&l, &r);
    let backup = f.0.join("backup");
    l.store().export(&backup).unwrap();
    l.store().restore(&backup).unwrap();
    assert!(l.persist_request_evidence(Uuid::new_v4(), 1, &p).is_err());
    assert_eq!(evidence_count(&l, c), 1);
}

#[test]
fn counter_and_provider_callbacks_can_write_without_deadlock_and_stale_selection_cannot_commit() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let r = append(&l, user(c, "orbit"), vec![]);
    let changed = std::cell::Cell::new(false);
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            |text| {
                if !changed.replace(true) {
                    append(&l, user(c, "later"), vec![]);
                }
                counter(text)
            },
        )
        .unwrap();
    assert!(l.persist_request_evidence(Uuid::new_v4(), 1, &p).is_err());
    struct Writing<'a> {
        ledger: &'a Ledger,
        index: Index,
        conversation: Uuid,
    }
    impl Provider for Writing<'_> {
        fn query(
            &self,
            k: &IndexKey,
            q: &str,
            n: usize,
            b: usize,
            neighbors: bool,
        ) -> Result<Retrieved> {
            append(
                self.ledger,
                user(self.conversation, "provider changed history"),
                vec![],
            );
            self.index.query(k, q, n, b, neighbors)
        }
    }
    let doc = Uuid::new_v4();
    let (r, s) = with_source(&l, user(c, "orbit"), doc);
    let provider = Writing {
        ledger: &l,
        index: Index::build(document(doc, "v1")).unwrap(),
        conversation: c,
    };
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            association(&r, &s, provider.index.key()),
            budget(),
            Some(&provider),
            counter,
        )
        .unwrap();
    assert!(l.persist_request_evidence(Uuid::new_v4(), 1, &p).is_err());
    assert_eq!(evidence_count(&l, c), 0);
}

#[test]
fn all_ordinary_faults_and_lost_ack_recover_exact_original() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::AfterCommit,
    ] {
        let f = Fixture::new();
        let l = f.ledger();
        let c = create(&l);
        let r = append(&l, user(c, "orbit"), vec![]);
        let p = no_document(&l, &r);
        let op = Uuid::new_v4();
        l.store().set_fault(fault).unwrap();
        let result = l.persist_request_evidence(op, 1, &p);
        assert_eq!(result.is_ok(), fault == Fault::AfterCommit);
        assert_eq!(
            evidence_count(&l, c),
            usize::from(fault == Fault::AfterCommit)
        );
        drop(l);
        let l = f.ledger();
        let read = l.request_evidence(c, r.id, op).unwrap();
        assert_eq!(read.is_some(), fault == Fault::AfterCommit);
        if let Some(read) = read {
            assert_eq!(
                l.persist_request_evidence(op, 1, &p).unwrap().historical,
                read.acknowledgment
            );
        } else {
            let fresh = no_document(&l, &r);
            l.persist_request_evidence(op, 1, &fresh).unwrap();
        }
        assert_eq!(evidence_count(&l, c), 1);
    }
}

#[test]
fn concurrent_identical_winners_replay_and_different_content_refuses() {
    for different in [false, true] {
        let f = Fixture::new();
        let l = f.ledger();
        let c = create(&l);
        let r = append(&l, user(c, "orbit"), vec![]);
        let p1 = no_document(&l, &r);
        let p2 = no_document(&l, &r);
        let op = Uuid::new_v4();
        let barrier = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| {
                barrier.wait();
                l.persist_request_evidence(op, 1, &p1)
            });
            let b = s.spawn(|| {
                barrier.wait();
                l.persist_request_evidence(op, if different { 2 } else { 1 }, &p2)
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        if different {
            assert_ne!(a.is_ok(), b.is_ok());
        } else {
            assert_eq!(a.unwrap().historical, b.unwrap().historical);
        }
        assert_eq!(evidence_count(&l, c), 1);
    }
}

#[test]
fn imported_original_raw_refs_media_omission_and_deleted_retry_preserve_history() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let doc = Uuid::new_v4();
    let (origin, source) = with_source(&l, user(c, "acceleration"), doc);
    let request = append(&l, user(c, "orbit"), vec![]);
    let target_f = Fixture::new();
    let target = target_f.ledger();
    replicate(l.store(), target.store(), true, true);
    let index = Index::build(document(doc, "v1")).unwrap();
    let p = target
        .prepare_request_evidence(
            c,
            request.id,
            Some(TurnRange {
                start: 1,
                end_exclusive: 2,
            }),
            association(&origin, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert_eq!(
        p.context().missing_media(),
        std::slice::from_ref(&source.image)
    );
    let op = Uuid::new_v4();
    let ack = target
        .persist_request_evidence(op, 8, &p)
        .unwrap()
        .historical;
    let read = target.request_evidence(c, request.id, op).unwrap().unwrap();
    assert_eq!(read.unavailable_media, vec![source.image.clone()]);
    assert_ne!(
        read.record.root.object.sha256,
        digest(
            &serde_json::to_vec(
                &target
                    .store()
                    .snapshot_revisions_matching(&[Namespace::Conversation], 4096, |e| e
                        .revision_id
                        == read.record.root.revision)
                    .unwrap()[0]
            )
            .unwrap()
        )
    );
    let stale = no_document(&target, &request);
    target
        .delete(c, Uuid::new_v4(), heads(&target, c), None)
        .unwrap();
    assert_eq!(
        target
            .persist_request_evidence(op, 8, &p)
            .unwrap()
            .historical,
        ack
    );
    assert!(target
        .persist_request_evidence(Uuid::new_v4(), 8, &stale)
        .is_err());
    assert!(target.inspect(c, false).is_err());
    assert_eq!(evidence_count(&target, c), 1);
    let final_f = Fixture::new();
    let final_l = final_f.ledger();
    replicate(target.store(), final_l.store(), true, false);
    assert_eq!(
        final_l
            .request_evidence(c, request.id, op)
            .unwrap()
            .unwrap()
            .record,
        read.record
    );
}

#[test]
fn complete_export_restore_keeps_original_evidence_and_actor_history() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let doc = Uuid::new_v4();
    let (r, s) = with_source(&l, user(c, "orbit"), doc);
    let index = Index::build(document(doc, "v1")).unwrap();
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            association(&r, &s, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 7, &p).unwrap();
    let original = l.request_evidence(c, r.id, op).unwrap().unwrap();
    let backup = f.0.join("backup");
    l.store().export(&backup).unwrap();
    let target_f = Fixture::new();
    let target = target_f.ledger();
    target.store().restore(&backup).unwrap();
    let read = target.request_evidence(c, r.id, op).unwrap().unwrap();
    assert_eq!(read.record, original.record);
    assert!(read.unavailable_media.is_empty());
    assert_ne!(target.store().actor_id, read.record.snapshot.actor);
}

#[test]
fn same_content_source_revision_tombstone_and_conflict_refuse_fresh_publication() {
    for change in 0..3 {
        let f = Fixture::new();
        let l = f.ledger();
        let c = create(&l);
        let doc = Uuid::new_v4();
        let (r, source) = with_source(&l, user(c, "orbit"), doc);
        let index = Index::build(document(doc, "v1")).unwrap();
        let p = l
            .prepare_request_evidence(
                c,
                r.id,
                None,
                association(&r, &source, index.key()),
                budget(),
                Some(&index),
                counter,
            )
            .unwrap();
        let mut e = l
            .store()
            .value(Namespace::Source, source.id)
            .unwrap()
            .unwrap();
        e.parents = if change == 2 {
            Default::default()
        } else {
            std::collections::BTreeSet::from([e.revision_id])
        };
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        if change == 1 {
            e.kind = Kind::Tombstone;
            e.payload = serde_json::Value::Null;
            e.media_descriptors.clear();
        }
        if change == 2 {
            let bytes = serde_json::to_vec(&e).unwrap();
            let hash = digest(&bytes);
            let m = Manifest {
                format: FORMAT,
                transaction_id: Uuid::new_v4(),
                scope: Scope::LocalTransaction,
                records: vec![ObjectRef {
                    sha256: hash.clone(),
                    bytes: bytes.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Source)]),
                media: vec![ObjectRef {
                    sha256: source.image.sha256.clone(),
                    bytes: source.image.bytes,
                }],
                media_coverage: vec![Coverage::Included {
                    sha256: source.image.sha256.clone(),
                }],
            };
            let image = l.store().read_object(&m.media[0]).unwrap();
            l.store()
                .import(
                    m,
                    BTreeMap::from([(hash, bytes), (source.image.sha256.clone(), image)]),
                )
                .unwrap();
        } else {
            l.store().commit(vec![e], BTreeMap::new()).unwrap();
        }
        assert!(l.persist_request_evidence(Uuid::new_v4(), 2, &p).is_err());
        assert!(l
            .store()
            .value(
                Namespace::Conversation,
                RequestEvidence::identity(Uuid::nil())
            )
            .unwrap()
            .is_none());
    }
}

fn import_mutated(source: &Store, target: &Store, mutation: impl Fn(&mut Envelope)) {
    let mut pending = source
        .manifests()
        .unwrap()
        .into_iter()
        .map(|mut m| {
            let mut objects = BTreeMap::new();
            for r in &mut m.records {
                let old = r.sha256.clone();
                let mut e: Envelope =
                    serde_json::from_slice(&source.read_object(r).unwrap()).unwrap();
                mutation(&mut e);
                let bytes = serde_json::to_vec(&e).unwrap();
                r.sha256 = digest(&bytes);
                r.bytes = bytes.len() as u64;
                let ns = m.record_namespaces.remove(&old).unwrap();
                m.record_namespaces.insert(r.sha256.clone(), ns);
                objects.insert(r.sha256.clone(), bytes);
            }
            for r in &m.media {
                objects.insert(r.sha256.clone(), source.read_object(r).unwrap());
            }
            (m, objects)
        })
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let n = pending.len();
        pending.retain(|(m, objects)| target.import(m.clone(), objects.clone()).is_err());
        assert!(pending.len() < n);
    }
}

#[test]
fn malformed_foreign_missing_noncanonical_and_oversized_historical_refs_refuse_inspection() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let r = append(&l, user(c, "orbit"), vec![]);
    let p = no_document(&l, &r);
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 1, &p).unwrap();
    for case in 0..9 {
        let bad_f = Fixture::new();
        let bad = bad_f.ledger();
        import_mutated(l.store(), bad.store(), |e| {
            let Ok(Record::RequestEvidence(mut fact)) = serde_json::from_value(e.payload.clone())
            else {
                return;
            };
            match case {
                0 => fact.root.object.sha256 = "a".repeat(64),
                1 => fact.root.record = Uuid::new_v4(),
                2 => fact.turns[0].revision = Uuid::new_v4(),
                3 => fact.root.object.bytes += 1,
                4 => fact.turns.clear(),
                5 => fact.frame_json.push(' '),
                6 => {
                    let mut frame: serde_json::Value =
                        serde_json::from_str(&fact.frame_json).unwrap();
                    frame["unknown"] = true.into();
                    fact.frame_json = serde_json::to_string(&frame).unwrap();
                }
                7 => fact.root.object.bytes = MAX_RECORD as u64 + 1,
                8 => fact.counted_tokens = fact.limits.context.provider_token_limit.unwrap() + 1,
                _ => unreachable!(),
            }
            fact.frame_sha256 = digest(fact.frame_json.as_bytes());
            e.payload = serde_json::to_value(Record::RequestEvidence(fact)).unwrap();
        });
        assert!(bad.request_evidence(c, r.id, op).is_err(), "case {case}");
        assert!(bad.inspect(c, true).is_err(), "inspection case {case}");
        assert_eq!(bad.store().unavailable_commits().unwrap(), 0);
    }
}

#[test]
fn receipt_mismatch_and_ambiguous_transaction_are_not_historical_success() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let r = append(&l, user(c, "orbit"), vec![]);
    let p = no_document(&l, &r);
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 1, &p).unwrap();
    let bad_f = Fixture::new();
    let bad = bad_f.ledger();
    import_mutated(l.store(), bad.store(), |e| {
        if let Ok(Record::Receipt(mut receipt)) = serde_json::from_value(e.payload.clone()) {
            if receipt.acknowledgment.operation == op {
                receipt.acknowledgment.records.clear();
                e.payload = serde_json::to_value(Record::Receipt(receipt)).unwrap();
            }
        }
    });
    assert!(bad.request_evidence(c, r.id, op).is_err());
    let mut m = l
        .store()
        .manifests()
        .unwrap()
        .into_iter()
        .find(|m| {
            m.records.iter().any(|r| {
                let e: Envelope =
                    serde_json::from_slice(&l.store().read_object(r).unwrap()).unwrap();
                e.record_id == RequestEvidence::identity(op)
            })
        })
        .unwrap();
    m.transaction_id = Uuid::new_v4();
    let objects = m
        .records
        .iter()
        .map(|r| (r.sha256.clone(), l.store().read_object(r).unwrap()))
        .collect();
    l.store().import(m, objects).unwrap();
    assert!(l.request_evidence(c, r.id, op).is_err());
    assert!(l.persist_request_evidence(op, 1, &p).is_err());
}

#[test]
fn encoded_boundary_is_exact_and_durable_escaping_is_not_truncated() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let r = append(&l, user(c, "\"orbit\"\\path\n"), vec![]);
    let initial = no_document(&l, &r);
    let size = initial.context().encoded_json().len();
    let mut exact = budget();
    exact.max_encoded_bytes = size;
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            RequestSource::NoDocument,
            exact.clone(),
            None,
            counter,
        )
        .unwrap();
    exact.max_encoded_bytes -= 1;
    assert!(l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            RequestSource::NoDocument,
            exact,
            None,
            counter
        )
        .is_err());
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 1, &p).unwrap();
    assert_eq!(
        l.request_evidence(c, r.id, op)
            .unwrap()
            .unwrap()
            .record
            .frame_json,
        p.context().encoded_json()
    );
}

#[test]
fn valid_near_maximum_frame_can_refuse_complete_durable_envelope() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let doc = Uuid::new_v4();
    let (r, mut source) = with_source(&l, user(c, "orbit"), doc);
    let mut normalized = document(doc, "v1");
    normalized.units[1].text = "orbit ".repeat(180);
    let index = Index::build(normalized).unwrap();
    let origin = r;
    for _ in 0..20 {
        let mut draft = user(c, "ineligible draft");
        draft.outcome = Outcome::Prepared;
        draft.text = None;
        append(&l, draft, vec![]);
    }
    let r = append(&l, user(c, "orbit"), vec![]);
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            association(&origin, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let overhead = p.context().encoded_json().len() - source.purpose.len();
    source.purpose = "\\".repeat((budget().max_encoded_bytes - overhead) / 2);
    let mut e = l
        .store()
        .value(Namespace::Source, source.id)
        .unwrap()
        .unwrap();
    e.parents = std::collections::BTreeSet::from([e.revision_id]);
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.payload = serde_json::to_value(Record::Source(source.clone())).unwrap();
    l.store().commit(vec![e], BTreeMap::new()).unwrap();
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            association(&origin, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert!(p.context().encoded_json().len() >= budget().max_encoded_bytes - 2);
    let op = Uuid::new_v4();
    assert!(l.persist_request_evidence(op, 1, &p).is_err());
    assert!(l
        .store()
        .heads(Namespace::Conversation, RequestEvidence::identity(op))
        .unwrap()
        .is_empty());
}

#[test]
fn corrupt_required_original_bytes_refuse_live_read_and_fresh_publication() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let r = append(&l, user(c, "orbit"), vec![]);
    let p = no_document(&l, &r);
    let op = Uuid::new_v4();
    l.persist_request_evidence(op, 1, &p).unwrap();
    let read = l.request_evidence(c, r.id, op).unwrap().unwrap();
    let fresh = no_document(&l, &r);
    let generation: Uuid = files::json(&f.0.join("data/CURRENT"), 128).unwrap();
    let path =
        f.0.join("data/generations")
            .join(generation.to_string())
            .join("objects")
            .join(&read.record.turns[0].object.sha256);
    assert!(path
        .canonicalize()
        .unwrap()
        .starts_with(f.0.canonicalize().unwrap()));
    fs::write(path, b"corrupt original").unwrap();
    assert!(l.request_evidence(c, r.id, op).is_err());
    assert!(l.inspect(c, true).is_err());
    assert!(l
        .persist_request_evidence(Uuid::new_v4(), 1, &fresh)
        .is_err());
}

#[test]
fn combined_original_refs_and_new_fact_work_refuses_before_any_publication() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let mut last = None;
    for n in 0..6 {
        let doc = Uuid::new_v4();
        let (r, mut source) = with_source(&l, user(c, "orbit"), doc);
        source.purpose = if n == 5 {
            "\\".repeat(200000)
        } else {
            "p".repeat(600000)
        };
        let mut e = l
            .store()
            .value(Namespace::Source, source.id)
            .unwrap()
            .unwrap();
        e.parents = std::collections::BTreeSet::from([e.revision_id]);
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        e.payload = serde_json::to_value(Record::Source(source.clone())).unwrap();
        l.store().commit(vec![e], BTreeMap::new()).unwrap();
        last = Some((r, source));
    }
    let (r, source) = last.unwrap();
    let index = Index::build(document(source.observation.document, "v1")).unwrap();
    let p = l
        .prepare_request_evidence(
            c,
            r.id,
            None,
            association(&r, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let before = l.store().manifests().unwrap().len();
    let op = Uuid::new_v4();
    let error = l.persist_request_evidence(op, 1, &p).err().unwrap();
    assert!(
        error.to_string().contains("reference work bound"),
        "{error}"
    );
    assert_eq!(l.store().manifests().unwrap().len(), before);
    assert!(l.request_evidence(c, r.id, op).unwrap().is_none());
}

#[test]
fn persisted_sdk_fixture_keeps_exact_synthetic_qualification_across_restart() {
    use remarkable_open_sdk::{
        capture::{
            synthetic::{validate_batch, Acquisition, ImageInput},
            *,
        },
        mock::MockPlatform,
        Platform,
    };
    use std::time::Duration;
    let sdk = |id: Uuid| remarkable_open_sdk::Uuid::parse(&id.to_string()).unwrap();
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let page = Uuid::new_v4();
    let observed = MockPlatform::new(sdk(Uuid::new_v4()), sdk(book), vec![sdk(page)], sdk(page))
        .observe_page()
        .unwrap();
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([17, 0, 0])))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let capture_request = CaptureRequest::new(
        remarkable_open_sdk::OperationId(sdk(Uuid::new_v4())),
        observed.clone(),
        Duration::from_secs(3),
        false,
    );
    let acquired = Acquisition {
        before: observed.clone(),
        after: observed.clone(),
        rendered_owner: observed,
        render_generation: 1,
        buffer_generation: 1,
        input_changed: false,
        interval: [Duration::from_secs(1), Duration::from_secs(2)],
        viewport_revision: "fixture".into(),
        conversion_procedure: "fixture".into(),
        procedure_revision: "fixture".into(),
        native_png: png.clone(),
        native_transform: Affine::new([1., 0., 0., 1., 0., 0.]).unwrap(),
        valid_source_region: SourceRegion {
            x: 0.,
            y: 0.,
            width: 1.,
            height: 1.,
        },
        target: None,
    };
    let batch = validate_batch(
        &capture_request,
        acquired,
        vec![ImageInput {
            role: ImageRole::Overview,
            crop: PixelRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            output_dimensions: [1, 1],
            filter: ResizeFilter::Nearest,
            png,
        }],
        CaptureLimits::default(),
    )
    .unwrap();
    let mut original = user(conversation, "not yet interpreted");
    original.outcome = Outcome::Prepared;
    original.text = None;
    let prepared = ledger
        .prepare_sdk_fixture(
            Uuid::new_v4(),
            heads(&ledger, conversation),
            original.clone(),
            &batch,
        )
        .unwrap();
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let index = Index::build(document(book, "v1")).unwrap();
    let choice = RequestSource::Original {
        origin_turn: original.id,
        source_record: prepared.capture,
        expected: index.key().clone(),
    };
    let evidence = ledger
        .prepare_request_evidence(
            conversation,
            request.id,
            None,
            choice,
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let frame = evidence.context();
    assert!(frame.encoded_json().contains("UnqualifiedSyntheticModel"));
    assert!(frame.encoded_json().contains(capture_facts::SDK_SOURCE));
    assert!(
        matches!(frame.source(),SourceAssociation::Original{source_record,..} if matches!(&**source_record,Record::Capture(capture) if capture.id==prepared.capture && capture.turn==original.id))
    );
    assert_eq!(frame.retrieval().unwrap().passages[0].page, Some(1));
    let json = frame.encoded_json().to_owned();
    let operation = Uuid::new_v4();
    ledger
        .persist_request_evidence(operation, 9, &evidence)
        .unwrap();
    drop(ledger);
    let ledger = fixture.ledger();
    let historical = ledger
        .request_evidence(conversation, request.id, operation)
        .unwrap()
        .unwrap();
    assert_eq!(historical.record.frame_json, json);
    assert!(historical
        .record
        .frame_json
        .contains(capture_facts::SDK_SOURCE));
}
