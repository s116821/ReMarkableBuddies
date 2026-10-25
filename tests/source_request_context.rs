//! Real local Store/Index fixtures. Counters are declared synthetic byte counts.
#[path = "source_request_context/disk_cache.rs"]
mod disk_cache;
#[path = "source_request_context/memory.rs"]
mod memory;
#[path = "source_request_context/ranking.rs"]
mod ranking;
#[path = "source_request_context/research.rs"]
mod research;
#[path = "source_request_context/saved_policy.rs"]
mod saved_policy;
#[path = "source_request_context/web_research.rs"]
mod web_research;
use anyhow::{bail, Result};
use remarkable_reader_buddy::{
    conversation::{receiving::*, *},
    retrieval::{
        Index, IndexKey, NormalizedDocument, Passage, Provider, Retrieved, SourceFormat, SourceUnit,
    },
    storage::*,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-receiving-{}", Uuid::new_v4())))
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
                .starts_with("buddy-receiving-"));
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
fn snapshot(path: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(path: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(&path, out)
            } else {
                if path.file_name().and_then(|name| name.to_str()) == Some("store.lock") {
                    // The Store intentionally holds an OS byte-range lease on Windows.
                    out.insert(
                        path.clone(),
                        format!("held-owner-lock:{}", entry.metadata().unwrap().len()),
                    );
                } else {
                    out.insert(path.clone(), digest(&fs::read(path).unwrap()));
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(path, &mut out);
    out
}
fn error(result: Result<FrozenRequestContext>) -> ReceivingRefusal {
    result.unwrap_err().downcast::<ReceivingRefusal>().unwrap()
}

#[test]
fn fresh_followups_preserve_verbatim_history_original_source_and_store() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (first, source) = with_source(&ledger, user(conversation, "  café acceleration?\n"), book);
    let fresh = document(book, "original-v1");
    let expected = Index::build(fresh.clone()).unwrap().key().clone();
    let index = Index::build(fresh).unwrap();
    let spy = Spy::new(&index);
    let before = snapshot(&fixture.0);
    let initial = ledger
        .source_request_context(
            conversation,
            first.id,
            None,
            association(&first, &source, &expected),
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert_eq!(initial.request(), &first);
    assert!(initial.history().is_empty());
    assert_eq!(initial.retrieval().unwrap().passages[0].page, Some(1));
    assert_eq!(snapshot(&fixture.0), before);
    let mut answer = user(conversation, "original answer\nwith Unicode é");
    answer.role = Role::Assistant;
    answer.outcome = Outcome::Completed;
    answer.completion = Some(CompletionEvidence {
        native_operation: Uuid::new_v4(),
        exact_text: answer.text.clone().unwrap(),
        observation: observed(book),
        procedure: "synthetic-history-only".into(),
    });
    let answer = append(&ledger, answer, vec![]);
    let (mut followup, buddy) = with_source(&ledger, user(conversation, "orbit"), Uuid::new_v4());
    assert_ne!(buddy.observation.document, book);
    let before = snapshot(&fixture.0);
    let received = ledger
        .source_request_context(
            conversation,
            followup.id,
            None,
            association(&first, &source, &expected),
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert_eq!(received.request(), &followup);
    assert_eq!(received.history(), [first.clone(), answer]);
    assert_eq!(received.retrieval().unwrap().passages[0].page, Some(9));
    assert_eq!(received.request().sources, vec![buddy.id]);
    assert_eq!(source.turn, first.id);
    assert!(
        matches!(received.source(),SourceAssociation::Original {origin_turn,source_record,expected:key} if *origin_turn==first.id && **source_record==Record::Source(source.clone()) && key==&expected)
    );
    assert_eq!(spy.calls.borrow().len(), 2);
    assert_eq!(spy.calls.borrow()[0].1, first.text.as_deref().unwrap());
    assert_eq!(spy.calls.borrow()[1].1, "orbit");
    assert_eq!(snapshot(&fixture.0), before);
    assert_eq!(received.counted_tokens(), received.encoded_json().len());
    let json: serde_json::Value = serde_json::from_str(received.encoded_json()).unwrap();
    assert_eq!(json["scope"], "ordinary-historical");
    assert_eq!(json["request"]["text"], "orbit");
    // An older request never receives a later answer or request as its history.
    let old = ledger
        .source_request_context(
            conversation,
            first.id,
            None,
            association(&first, &source, &expected),
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert!(old.history().is_empty());
    followup.text = Some("changed caller clone".into());
    assert_eq!(received.request().text.as_deref(), Some("orbit"));
}

#[test]
fn no_document_is_explicit_and_never_queries_a_provider() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(
        &ledger,
        user(conversation, "  exact blank-start request é  "),
        vec![],
    );
    let index = Index::build(document(Uuid::new_v4(), "v1")).unwrap();
    let spy = Spy::new(&index);
    let before = snapshot(&fixture.0);
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert_eq!(frame.request(), &request);
    assert!(frame.history().is_empty());
    assert!(frame.retrieval().is_none());
    assert_eq!(frame.source(), &SourceAssociation::NoDocument);
    assert!(spy.calls.borrow().is_empty());
    assert_eq!(snapshot(&fixture.0), before);
    assert!(ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter
        )
        .is_ok());
}

#[test]
fn wrong_buddy_document_stale_keys_and_unavailable_provider_do_not_fall_back() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (origin, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let key = Index::build(document(book, "v1")).unwrap().key().clone();
    let spy = Spy::new(&index);
    let (current, buddy) = with_source(&ledger, user(conversation, "orbit"), Uuid::new_v4());
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            current.id,
            None,
            association(&current, &buddy, &key),
            budget(),
            Some(&spy),
            counter
        )),
        ReceivingRefusal::AssociationMismatch
    );
    assert!(spy.calls.borrow().is_empty());
    for field in 0..4 {
        let mut stale = key.clone();
        match field {
            0 => stale.source_sha256 = digest(b"changed"),
            1 => stale.extractor_revision = "next".into(),
            2 => stale.normalized_sha256 = digest(b"changed units"),
            _ => stale.format = SourceFormat::Epub,
        }
        assert_eq!(
            error(ledger.source_request_context(
                conversation,
                current.id,
                None,
                association(&origin, &source, &stale),
                budget(),
                Some(&spy),
                counter
            )),
            ReceivingRefusal::RetrievalUnavailable
        );
    }
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            current.id,
            None,
            association(&origin, &source, &key),
            budget(),
            None,
            counter
        )),
        ReceivingRefusal::RetrievalUnavailable
    );
    let fresh = document(book, "changed-source-v2");
    let fresh_key = Index::build(fresh.clone()).unwrap().key().clone();
    let fresh_index = Index::build(fresh).unwrap();
    let frame = ledger
        .source_request_context(
            conversation,
            current.id,
            None,
            association(&origin, &source, &fresh_key),
            budget(),
            Some(&fresh_index),
            counter,
        )
        .unwrap();
    assert_eq!(&frame.retrieval().unwrap().source, &fresh_key);
    let new_book = Uuid::new_v4();
    let (changed, new_source) = with_source(&ledger, user(conversation, "orbit"), new_book);
    let new_index = Index::build(document(new_book, "new target")).unwrap();
    let frame = ledger
        .source_request_context(
            conversation,
            changed.id,
            None,
            association(&changed, &new_source, new_index.key()),
            budget(),
            Some(&new_index),
            counter,
        )
        .unwrap();
    assert_eq!(frame.retrieval().unwrap().source.document, new_book);
}

#[test]
fn source_origin_must_be_linked_foreign_future_and_key_corruption_refuse() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let (future, source) = with_source(&ledger, user(conversation, "orbit"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let spy = Spy::new(&index);
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&future, &source, index.key()),
            budget(),
            Some(&spy),
            counter
        )),
        ReceivingRefusal::AssociationMismatch
    );
    let unlinked = append(&ledger, user(conversation, "acceleration"), vec![]);
    let choice = RequestSource::Original {
        origin_turn: unlinked.id,
        source_record: source.id,
        expected: index.key().clone(),
    };
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            unlinked.id,
            None,
            choice,
            budget(),
            Some(&spy),
            counter
        )),
        ReceivingRefusal::AssociationMismatch
    );
    let foreign = create(&ledger);
    let (foreign_turn, foreign_source) = with_source(&ledger, user(foreign, "orbit"), book);
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            unlinked.id,
            None,
            association(&foreign_turn, &foreign_source, index.key()),
            budget(),
            Some(&spy),
            counter
        )),
        ReceivingRefusal::OriginalUnavailable
    );
    for n in 0..4 {
        let mut bad = index.key().clone();
        match n {
            0 => bad.document = Uuid::nil(),
            1 => bad.source_sha256 = "F".repeat(64),
            2 => bad.normalized_sha256 = "short".into(),
            _ => bad.extractor_revision = " ".into(),
        };
        assert_eq!(
            error(ledger.source_request_context(
                conversation,
                future.id,
                None,
                association(&future, &source, &bad),
                budget(),
                Some(&spy),
                counter
            )),
            ReceivingRefusal::InvalidKey
        );
    }
    assert!(spy.calls.borrow().is_empty());
}

#[test]
fn explicit_range_preserves_corrections_and_wrong_ranges_refuse() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let first = append(&ledger, user(conversation, "old text"), vec![]);
    let mut corrected = user(conversation, "  corrected é\n");
    corrected.correction_of = Some(first.id);
    let corrected = append(&ledger, corrected, vec![]);
    let selection = TurnRange {
        start: 1,
        end_exclusive: 2,
    };
    let frame = ledger
        .source_request_context(
            conversation,
            corrected.id,
            Some(selection),
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap();
    assert_eq!(frame.request(), &corrected);
    assert!(frame.history().is_empty());
    assert_eq!(frame.selection(), Some(selection));
    assert_eq!(frame.request().correction_of, Some(first.id));
    for range in [
        TurnRange {
            start: 0,
            end_exclusive: 1,
        },
        TurnRange {
            start: 2,
            end_exclusive: 3,
        },
    ] {
        assert_eq!(
            error(ledger.source_request_context(
                conversation,
                corrected.id,
                Some(range),
                RequestSource::NoDocument,
                budget(),
                None,
                counter
            )),
            ReceivingRefusal::InvalidSelection
        );
    }
    let mut small = budget();
    small.context.max_turns = 1;
    assert!(ledger
        .source_request_context(
            conversation,
            corrected.id,
            None,
            RequestSource::NoDocument,
            small,
            None,
            counter
        )
        .unwrap_err()
        .downcast_ref::<ContextRefusal>()
        .is_some());
}

#[test]
fn exact_full_frame_encoded_bound_accounts_for_json_escaping() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, &"\"\\\n\t".repeat(100)), vec![]);
    let full = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap();
    let length = full.encoded_json().len();
    assert!(length > request.text.as_ref().unwrap().len());
    let mut exact = budget();
    exact.max_encoded_bytes = length;
    assert_eq!(
        ledger
            .source_request_context(
                conversation,
                request.id,
                None,
                RequestSource::NoDocument,
                exact,
                None,
                counter
            )
            .unwrap()
            .encoded_json(),
        full.encoded_json()
    );
    let mut short = budget();
    short.max_encoded_bytes = length - 1;
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            short,
            None,
            counter
        )),
        ReceivingRefusal::EncodedLimit
    );
}

#[test]
fn full_frame_token_limit_includes_provenance_and_counter_failure_is_not_guessed() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let full = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let mut tight = budget();
    tight.context.provider_token_limit = Some(full.counted_tokens() - 1);
    assert!(matches!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            tight,
            Some(&index),
            counter
        )),
        ReceivingRefusal::TokenLimit { .. }
    ));
    let mut exact = budget();
    exact.context.provider_token_limit = Some(full.counted_tokens());
    assert!(ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            exact,
            Some(&index),
            counter
        )
        .is_ok());
    let spy = Spy::new(&index);
    let mut unknown = budget();
    unknown.context.provider_token_limit = None;
    assert!(ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            unknown,
            Some(&spy),
            counter
        )
        .unwrap_err()
        .downcast_ref::<ContextRefusal>()
        .is_some());
    assert!(spy.calls.borrow().is_empty());
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            |_| bail!("source text must not escape")
        )),
        ReceivingRefusal::CounterUnavailable
    );
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            |_| Ok(0)
        )),
        ReceivingRefusal::CounterUnavailable
    );
}

#[test]
fn encoded_room_reserves_null_replacement_and_refuses_below_provider_minimum() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let full = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(full.encoded_json()).unwrap();
    value["retrieval"] = serde_json::Value::Null;
    let base = serde_json::to_vec(&value).unwrap().len();
    let spy = Spy::new(&index);
    let mut less = budget();
    less.max_encoded_bytes = base + 511 - 4;
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            less,
            Some(&spy),
            counter
        )),
        ReceivingRefusal::EncodedLimit
    );
    assert!(spy.calls.borrow().is_empty());
    let spy = Spy::new(&index);
    let mut exact = budget();
    exact.max_encoded_bytes = base + 512 - 4;
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            exact,
            Some(&spy),
            counter,
        )
        .unwrap();
    assert_eq!(spy.calls.borrow()[0].2, 512);
    assert!(frame.encoded_json().len() <= base + 512 - 4);
}

#[test]
fn callback_replacement_keeps_captured_records_without_current_authority() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let changed = Cell::new(false);
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            |text| {
                if !changed.replace(true) {
                    ledger
                        .delete(
                            conversation,
                            Uuid::new_v4(),
                            heads(&ledger, conversation),
                            None,
                        )
                        .unwrap();
                }
                counter(text)
            },
        )
        .unwrap();
    assert_eq!(frame.request(), &request);
    assert_eq!(frame.retrieval().unwrap().source, index.key().clone());
    assert!(ledger.inspect(conversation, false).is_err());
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter
        )),
        ReceivingRefusal::InvalidHistory
    );
}

struct BadProvider {
    key: IndexKey,
    kind: u8,
    calls: Cell<usize>,
}
impl Provider for BadProvider {
    fn query(&self, _: &IndexKey, _: &str, _: usize, _: usize, _: bool) -> Result<Retrieved> {
        self.calls.set(self.calls.get() + 1);
        let mut result = Retrieved {
            source: self.key.clone(),
            passages: vec![Passage {
                unit: 0,
                page: Some(1),
                section: None,
                range: [0, 3],
                text: "abc".into(),
                direct_match: true,
            }],
            budget_limited: false,
        };
        match self.kind {
            0 => result.source.source_sha256 = digest(b"wrong"),
            1 => result.passages[0].range = [0, 4],
            2 => result.passages[0].page = Some(0),
            3 => result.passages[0].text = "\0bc".into(),
            4 => result.passages.push(result.passages[0].clone()),
            5 => result.passages[0].direct_match = false,
            6 => {
                result.passages[0].text = "x".repeat(20000);
                result.passages[0].range = [0, 20000];
            }
            _ => bail!("private question/source must never echo"),
        };
        Ok(result)
    }
}
#[test]
fn malformed_or_failed_provider_results_never_become_partial_frames() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let before = snapshot(&fixture.0);
    for kind in 0..8 {
        let provider = BadProvider {
            key: index.key().clone(),
            kind,
            calls: Cell::new(0),
        };
        let failure = ledger
            .source_request_context(
                conversation,
                request.id,
                None,
                association(&request, &source, index.key()),
                budget(),
                Some(&provider),
                counter,
            )
            .unwrap_err();
        assert!(!failure.to_string().contains("private question"));
        assert_eq!(provider.calls.get(), 1);
    }
    assert_eq!(snapshot(&fixture.0), before);
}

fn replace_record(
    ledger: &Ledger,
    namespace: Namespace,
    id: Uuid,
    record: Record,
    media: Vec<Media>,
    objects: BTreeMap<String, Vec<u8>>,
) {
    let mut envelope = ledger.store().value(namespace, id).unwrap().unwrap();
    envelope.parents = std::collections::BTreeSet::from([envelope.revision_id]);
    envelope.revision_id = Uuid::new_v4();
    envelope.operation_id = Uuid::new_v4();
    envelope.payload = serde_json::to_value(record).unwrap();
    envelope.media_descriptors = media;
    ledger.store().commit(vec![envelope], objects).unwrap();
}

#[test]
fn incomplete_allocated_history_and_uninterpreted_or_writer_requests_refuse() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "exact request"), vec![]);
    let mut changed = request.clone();
    changed.mode = Mode::Writer;
    replace_record(
        &ledger,
        Namespace::Conversation,
        request.id,
        Record::Turn(changed),
        vec![],
        BTreeMap::new(),
    );
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter
        )),
        ReceivingRefusal::InvalidRequest
    );
    let mut changed = request.clone();
    changed.outcome = Outcome::Prepared;
    changed.text = None;
    replace_record(
        &ledger,
        Namespace::Conversation,
        request.id,
        Record::Turn(changed),
        vec![],
        BTreeMap::new(),
    );
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter
        )),
        ReceivingRefusal::InvalidRequest
    );
    replace_record(
        &ledger,
        Namespace::Conversation,
        request.id,
        Record::Turn(request.clone()),
        vec![],
        BTreeMap::new(),
    );
    let mut root = ledger
        .inspect(conversation, false)
        .unwrap()
        .into_iter()
        .find_map(|r| match r {
            Record::Root(r) => Some(r),
            _ => None,
        })
        .unwrap();
    root.next_sequence += 1;
    replace_record(
        &ledger,
        Namespace::Conversation,
        conversation,
        Record::Root(root),
        vec![],
        BTreeMap::new(),
    );
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter
        )),
        ReceivingRefusal::InvalidHistory
    );
}

#[test]
fn imported_original_media_omitted_outside_history_range_stays_explicit() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (origin, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let (request, current) = with_source(&ledger, user(conversation, "orbit"), Uuid::new_v4());
    assert_ne!(source.image.sha256, current.image.sha256);
    let target_fixture = Fixture::new();
    let target = target_fixture.ledger();
    let mut pending = ledger.store().manifests().unwrap();
    for manifest in &mut pending {
        manifest
            .media
            .retain(|reference| reference.sha256 != source.image.sha256);
        for coverage in &mut manifest.media_coverage {
            if coverage.hash() == source.image.sha256 {
                *coverage = Coverage::Omitted {
                    sha256: source.image.sha256.clone(),
                    reason: Omission::UnavailableAtSource,
                };
            }
        }
    }
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|manifest| {
            let objects = manifest
                .records
                .iter()
                .chain(&manifest.media)
                .map(|reference| {
                    (
                        reference.sha256.clone(),
                        ledger.store().read_object(reference).unwrap(),
                    )
                })
                .collect();
            target.store().import(manifest.clone(), objects).is_err()
        });
        assert!(pending.len() < before);
    }
    assert_eq!(target.store().unavailable_commits().unwrap(), 0);
    let index = Index::build(document(book, "v1")).unwrap();
    let range = TurnRange {
        start: request.sequence,
        end_exclusive: request.sequence + 1,
    };
    let frame = target
        .source_request_context(
            conversation,
            request.id,
            Some(range),
            association(&origin, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert!(frame.history().is_empty());
    assert_eq!(frame.missing_media(), std::slice::from_ref(&source.image));
    assert_eq!(frame.retrieval().unwrap().passages[0].page, Some(9));
    assert!(
        matches!(frame.source(),SourceAssociation::Original {source_record,..} if **source_record==Record::Source(source.clone()))
    );
    assert!(target
        .store()
        .open_object(&ObjectRef {
            sha256: source.image.sha256,
            bytes: source.image.bytes
        })
        .is_err());
}

#[test]
fn legacy_identity_cannot_be_supplied_by_an_index_key() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let legacy = LegacyCapture {
        schema: 1,
        id: source.id,
        conversation,
        turn: request.id,
        origin: LegacyOrigin::LegacyUnqualified,
        identity: (),
        qualification: (),
        parent: None,
        images: vec![LegacyImage {
            media: source.image.clone(),
            dimensions: [2, 3],
            role: LegacyImageRole::Overview,
            provider_ordinal: Some(0),
        }],
    };
    replace_record(
        &ledger,
        Namespace::Source,
        source.id,
        Record::LegacyCapture(legacy),
        vec![source.image.clone()],
        BTreeMap::new(),
    );
    let index = Index::build(document(book, "v1")).unwrap();
    let spy = Spy::new(&index);
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&spy),
            counter
        )),
        ReceivingRefusal::OriginalUnavailable
    );
    assert!(spy.calls.borrow().is_empty());
}

fn png_media(width: u32, height: u32, color: u8) -> (Media, Vec<u8>) {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        width,
        height,
        image::Rgb([color, 0, 0]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let bytes = bytes.into_inner();
    (
        Media {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
            media_type: "image/png".into(),
        },
        bytes,
    )
}

#[test]
fn development_source_keeps_unqualified_classification_and_requires_canonical_uuid() {
    use remarkable_reader_buddy::conversation::development::{
        DevelopmentImage, DevelopmentOrigin, HistoricalDevelopmentBinding, DEVELOPMENT_SDK_SOURCE,
    };
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let (original, original_bytes) = png_media(2, 5, 2);
    let (overview, overview_bytes) = png_media(768, 1024, 3);
    let (strip, strip_bytes) = png_media(2, 2, 4);
    let completion_bytes = b"{}".to_vec();
    let completion = Media {
        sha256: digest(&completion_bytes),
        bytes: 2,
        media_type: "application/json".into(),
    };
    let mut stored = DevelopmentCaptureHistory {
        schema: 1,
        sdk_source: DEVELOPMENT_SDK_SOURCE.into(),
        id: source.id,
        conversation,
        turn: request.id,
        origin: DevelopmentOrigin::DevelopmentUnqualified,
        expected_binding: HistoricalDevelopmentBinding {
            nonce: "fixture".into(),
            attempt_pid: "1".into(),
            attempt_start: "1".into(),
            root_device: "1".into(),
            root_inode: "1".into(),
            document_id: book.to_string(),
            expected_order: vec![Uuid::new_v4().to_string()],
        },
        completion: completion.clone(),
        original_png: original.clone(),
        original_dimensions: [2, 5],
        provider_procedure: "reader-nearest-overview-overlapping-strips-v1".into(),
        images: vec![
            DevelopmentImage {
                media: overview.clone(),
                dimensions: [768, 1024],
                provider_ordinal: 0,
                parent_crop: [0, 0, 2, 5],
            },
            DevelopmentImage {
                media: strip.clone(),
                dimensions: [2, 2],
                provider_ordinal: 1,
                parent_crop: [0, 0, 2, 2],
            },
            DevelopmentImage {
                media: strip.clone(),
                dimensions: [2, 2],
                provider_ordinal: 2,
                parent_crop: [0, 1, 2, 2],
            },
            DevelopmentImage {
                media: strip.clone(),
                dimensions: [2, 2],
                provider_ordinal: 3,
                parent_crop: [0, 3, 2, 2],
            },
        ],
    };
    let media = vec![
        completion.clone(),
        original.clone(),
        overview.clone(),
        strip.clone(),
    ];
    let objects = BTreeMap::from([
        (completion.sha256, completion_bytes),
        (original.sha256, original_bytes),
        (overview.sha256, overview_bytes),
        (strip.sha256, strip_bytes),
    ]);
    replace_record(
        &ledger,
        Namespace::Source,
        source.id,
        Record::DevelopmentCapture(Box::new(stored.clone())),
        media.clone(),
        objects,
    );
    let index = Index::build(document(book, "v1")).unwrap();
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert!(frame.encoded_json().contains("DevelopmentUnqualified"));
    assert!(
        matches!(frame.source(),SourceAssociation::Original{source_record,..} if **source_record==Record::DevelopmentCapture(Box::new(stored.clone())))
    );
    stored.expected_binding.document_id = "not-a-canonical-uuid".into();
    replace_record(
        &ledger,
        Namespace::Source,
        source.id,
        Record::DevelopmentCapture(Box::new(stored)),
        media,
        BTreeMap::new(),
    );
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter
        )),
        ReceivingRefusal::OriginalUnavailable
    );
}

#[test]
fn honest_zero_match_is_distinct_from_no_source_or_failure() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "unmatchedword"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert!(frame.retrieval().unwrap().passages.is_empty());
    assert!(!frame.retrieval().unwrap().budget_limited);
    assert!(matches!(frame.source(), SourceAssociation::Original { .. }));
}

#[test]
fn sdk_historical_source_labels_remain_synthetic_and_keep_exact_provenance() {
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
    let (_, png) = png_media(1, 1, 17);
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
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            choice,
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert!(frame.encoded_json().contains("UnqualifiedSyntheticModel"));
    assert!(frame.encoded_json().contains(capture_facts::SDK_SOURCE));
    assert!(
        matches!(frame.source(),SourceAssociation::Original{source_record,..} if matches!(&**source_record,Record::Capture(capture) if capture.id==prepared.capture && capture.turn==original.id))
    );
    assert_eq!(frame.retrieval().unwrap().passages[0].page, Some(1));
}

struct ReplacingProvider<'a> {
    ledger: &'a Ledger,
    index: &'a Index,
    source: &'a ImageUse,
    called: Cell<bool>,
}
impl Provider for ReplacingProvider<'_> {
    fn query(
        &self,
        key: &IndexKey,
        text: &str,
        count: usize,
        bytes: usize,
        neighbors: bool,
    ) -> Result<Retrieved> {
        assert!(!self.called.replace(true));
        let mut changed = self.source.clone();
        changed.observation.document = Uuid::new_v4();
        replace_record(
            self.ledger,
            Namespace::Source,
            changed.id,
            Record::Source(changed.clone()),
            vec![changed.image],
            BTreeMap::new(),
        );
        self.index.query(key, text, count, bytes, neighbors)
    }
}
#[test]
fn provider_callback_cannot_swap_original_source_metadata_into_captured_frame() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let provider = ReplacingProvider {
        ledger: &ledger,
        index: &index,
        source: &source,
        called: Cell::new(false),
    };
    let frame = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&provider),
            counter,
        )
        .unwrap();
    assert!(
        matches!(frame.source(),SourceAssociation::Original{source_record,..} if **source_record==Record::Source(source.clone()))
    );
    assert_eq!(
        error(ledger.source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
            counter
        )),
        ReceivingRefusal::AssociationMismatch
    );
}

#[test]
fn invalid_input_budgets_and_unshortened_query_limits_refuse_before_query() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let spy = Spy::new(&index);
    for n in 0..8 {
        let mut bad = budget();
        match n {
            0 => bad.context.max_turns = 65,
            1 => bad.context.max_text_bytes = 65537,
            2 => bad.max_encoded_bytes = 511,
            3 => bad.max_encoded_bytes = 524289,
            4 => bad.max_passages = 0,
            5 => bad.max_passage_bytes = 511,
            6 => bad.max_passage_bytes = 16385,
            _ => bad.context.provider_token_limit = Some(0),
        };
        assert_eq!(
            error(ledger.source_request_context(
                conversation,
                request.id,
                None,
                association(&request, &source, index.key()),
                bad,
                Some(&spy),
                counter
            )),
            ReceivingRefusal::InvalidBudget
        );
    }
    for text in [
        "x".repeat(2049),
        (0..65)
            .map(|n| format!("term{n}"))
            .collect::<Vec<_>>()
            .join(" "),
    ] {
        let long = append(&ledger, user(conversation, &text), vec![]);
        assert_eq!(
            error(ledger.source_request_context(
                conversation,
                long.id,
                None,
                association(&request, &source, index.key()),
                budget(),
                Some(&spy),
                counter
            )),
            ReceivingRefusal::InvalidRequest
        );
    }
    assert!(spy.calls.borrow().is_empty());
}
