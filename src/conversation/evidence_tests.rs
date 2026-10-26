//! Deterministic public-path observation and allocation-admission regressions.
use super::*;
use std::{cell::RefCell, rc::Rc};

thread_local! {
    static AFTER_ABSENCE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}
pub(super) fn after_operation_snapshot(pair: &[Option<Envelope>; 2]) {
    if pair.iter().all(Option::is_none) {
        let hook = AFTER_ABSENCE.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}
fn after_absence(hook: impl FnOnce() + 'static) {
    AFTER_ABSENCE.with(|slot| {
        assert!(slot.borrow_mut().replace(Box::new(hook)).is_none());
    });
}
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-evidence-observation-{}", Uuid::new_v4())))
    }
    fn ledger(&self) -> Arc<Ledger> {
        Arc::new(Ledger::new(Arc::new(
            Store::open(crate::storage::StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        )))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() || !self.0.exists() {
            return;
        }
        let path = self.0.canonicalize().unwrap();
        assert_eq!(
            path.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("buddy-evidence-observation-"));
        std::fs::remove_dir_all(path).unwrap();
    }
}
fn seeded(ledger: &Ledger, text: String) -> Turn {
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let turn = Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange: Uuid::new_v4(),
        sequence: 0,
        role: Role::User,
        mode: Mode::Reader,
        outcome: Outcome::Interpreted,
        text: Some(text),
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    };
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            turn.clone(),
            vec![],
        )
        .unwrap();
    turn
}
fn prepare(ledger: &Ledger, turn: &Turn) -> PreparedRequestEvidence {
    ledger
        .prepare_request_evidence(
            turn.conversation,
            turn.id,
            None,
            RequestSource::NoDocument,
            ReceivingBudget {
                context: ContextBudget {
                    max_turns: 64,
                    max_text_bytes: 65536,
                    provider_token_limit: Some(1000000),
                },
                max_encoded_bytes: 524288,
                max_passages: 2,
                max_passage_bytes: 16384,
                neighbors: false,
            },
            None,
            |text| Ok(text.len()),
        )
        .unwrap()
}
#[test]
fn identical_winner_after_absence_snapshot_is_recovered_by_postattempt_replay() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let turn = seeded(&ledger, "orbit".into());
    let prepared = prepare(&ledger, &turn);
    let winner = prepare(&ledger, &turn);
    let op = Uuid::new_v4();
    let acknowledgment = Rc::new(RefCell::new(None));
    let output = acknowledgment.clone();
    let concurrent = ledger.clone();
    after_absence(move || {
        // This real publication is after the absence snapshot, before its caller
        // attempts admission. It also proves no Store mutex is held by the hook.
        *output.borrow_mut() = Some(
            concurrent
                .persist_request_evidence(op, 7, &winner)
                .unwrap()
                .historical,
        );
    });
    let result = ledger.persist_request_evidence(op, 7, &prepared).unwrap();
    assert_eq!(Some(result.historical), *acknowledgment.borrow());
    assert_eq!(
        ledger
            .inspect(turn.conversation, false)
            .unwrap()
            .iter()
            .filter(|r| matches!(r, Record::RequestEvidence(_)))
            .count(),
        1
    );
}
#[test]
fn public_read_keeps_coherent_absence_when_winner_commits_before_return() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let turn = seeded(&ledger, "orbit".into());
    let winner = prepare(&ledger, &turn);
    let op = Uuid::new_v4();
    let concurrent = ledger.clone();
    after_absence(move || {
        concurrent.persist_request_evidence(op, 7, &winner).unwrap();
    });
    // None linearizes at the coherent absence observation. A complete commit
    // immediately afterward must not become a fictitious orphan-receipt error.
    assert!(ledger
        .request_evidence(turn.conversation, turn.id, op)
        .unwrap()
        .is_none());
    assert!(ledger
        .request_evidence(turn.conversation, turn.id, op)
        .unwrap()
        .is_some());
}
fn import_raw(store: &Store, envelopes: Vec<Envelope>, padded: Option<usize>) {
    use crate::storage::{Manifest, Scope, FORMAT};
    let mut objects = BTreeMap::new();
    let mut references = Vec::new();
    let mut namespaces = BTreeMap::new();
    for e in envelopes {
        let mut bytes = serde_json::to_vec(&e).unwrap();
        if let Some(size) = padded {
            assert!(bytes.len() <= size);
            bytes.resize(size, b' ');
        }
        let hash = digest(&bytes);
        references.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(hash.clone(), e.namespace);
        objects.insert(hash, bytes);
    }
    store
        .import(
            Manifest {
                format: FORMAT,
                transaction_id: Uuid::new_v4(),
                scope: Scope::LocalTransaction,
                records: references,
                record_namespaces: namespaces,
                media: vec![],
                media_coverage: vec![],
            },
            objects,
        )
        .unwrap();
}
fn fork(original: &Envelope) -> Envelope {
    let mut e = original.clone();
    e.parents.clear();
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e
}
#[test]
fn public_conflicted_fact_and_receipt_admit_both_sets_before_cloning_either() {
    for receipt_conflict in [false, true] {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let turn = seeded(&ledger, "\\".repeat(32000));
        let p = prepare(&ledger, &turn);
        let op = Uuid::new_v4();
        ledger.persist_request_evidence(op, 7, &p).unwrap();
        let fact = ledger
            .store
            .value(Namespace::Conversation, RequestEvidence::identity(op))
            .unwrap()
            .unwrap();
        assert!(serde_json::to_vec(&fact).unwrap().len() > 128000);
        let original = if receipt_conflict {
            ledger
                .store
                .value(Namespace::Conversation, Ledger::receipt_id(op))
                .unwrap()
                .unwrap()
        } else {
            fact
        };
        import_raw(&ledger.store, vec![fork(&original)], None);
        crate::storage::captured::take_head_clones();
        let error = ledger
            .request_evidence(turn.conversation, turn.id, op)
            .unwrap_err();
        assert!(error.to_string().contains("historical head count bound"));
        assert_eq!(
            crate::storage::captured::take_head_clones(),
            0,
            "no body may be cloned before both sets are admitted"
        );
    }
}
#[test]
fn public_retry_refuses_current_root_raw_byte_admission_without_cloning_heads() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let turn = seeded(&ledger, "orbit".into());
    let p = prepare(&ledger, &turn);
    let op = Uuid::new_v4();
    ledger.persist_request_evidence(op, 7, &p).unwrap();
    let original = ledger
        .store
        .value(Namespace::Conversation, turn.conversation)
        .unwrap()
        .unwrap();
    // Valid imported raw JSON whitespace exceeds 8 MiB cumulatively while each
    // record and each transaction retain the existing storage format bounds.
    for _ in 0..3 {
        import_raw(
            &ledger.store,
            (0..6).map(|_| fork(&original)).collect(),
            Some(500000),
        );
    }
    assert!(ledger
        .request_evidence(turn.conversation, turn.id, op)
        .unwrap()
        .is_some());
    crate::storage::captured::take_head_clones();
    let error = ledger.persist_request_evidence(op, 7, &p).err().unwrap();
    assert!(
        error.to_string().contains("historical head byte bound"),
        "{error}"
    );
    // The operation pair is two admitted clones; none of the oversized current
    // root-head set is cloned before refusing the public retry result.
    assert_eq!(crate::storage::captured::take_head_clones(), 2);
}
#[test]
fn actually_inconsistent_imported_pairs_remain_refused() {
    for receipt_only in [false, true] {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let turn = seeded(&ledger, "orbit".into());
        let p = prepare(&ledger, &turn);
        let op = Uuid::new_v4();
        let fact = RequestEvidence {
            evidence_version: 1,
            id: RequestEvidence::identity(op),
            conversation: turn.conversation,
            request_turn: turn.id,
            operation: op,
            recorded_ms: 7,
            frame_json: p.context.encoded_json().into(),
            frame_sha256: digest(p.context.encoded_json().as_bytes()),
            counted_tokens: p.context.counted_tokens(),
            limits: p.limits.clone(),
            snapshot: SnapshotIdentity {
                actor: p.capture.actor,
                generation: p.capture.generation,
                heads_sha256: p.capture.digest().unwrap(),
            },
            root: p.root.clone(),
            turns: p.turns.clone(),
            sources: p.sources.clone(),
        };
        let record = if receipt_only {
            Record::Receipt(Receipt {
                fingerprint: fact.fingerprint().unwrap(),
                acknowledgment: Acknowledgment {
                    operation: op,
                    conversation: turn.conversation,
                    records: vec![fact.id],
                    applied_root_revision: Uuid::new_v4(),
                    binding: None,
                },
                admitted_intent: None,
            })
        } else {
            Record::RequestEvidence(Box::new(fact))
        };
        let id = if receipt_only {
            Ledger::receipt_id(op)
        } else {
            RequestEvidence::identity(op)
        };
        let envelope = ledger
            .envelope(
                Namespace::Conversation,
                id,
                ExpectedHeads::default(),
                record,
                vec![],
            )
            .unwrap();
        import_raw(&ledger.store, vec![envelope], None);
        assert!(ledger
            .request_evidence(turn.conversation, turn.id, op)
            .is_err());
        assert!(ledger.persist_request_evidence(op, 7, &p).is_err());
    }
}
