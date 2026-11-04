use super::*;
use crate::storage::{Fault, StorePaths};
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("conversation-replay-race-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn identical_winner_between_receipt_and_root_reads_replays_without_a_second_commit() {
    for lost_ack in [false, true] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        let expected = ledger
            .expected(Namespace::Conversation, conversation)
            .unwrap();
        let operation = Uuid::new_v4();
        let request = Turn {
            id: Uuid::new_v4(),
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some("one exact request".into()),
            sources: vec![],
            correction_of: None,
            created_ms: 2,
            updated_ms: 2,
            completion: None,
        };
        let sources: Vec<ImageUse> = vec![];
        let fingerprint = Ledger::fingerprint(&("append", &request, &sources)).unwrap();
        // Deterministically pause the loser immediately after its initial miss.
        assert!(ledger.replay(operation, &fingerprint).unwrap().is_none());
        if lost_ack {
            store.set_fault(Fault::AfterCommit).unwrap();
        }
        let winner = ledger
            .append(operation, expected.clone(), request.clone(), vec![])
            .unwrap();
        let committed = ledger
            .expected(Namespace::Conversation, conversation)
            .unwrap();
        assert_ne!(committed.0, expected.0);
        // Resume the exact production continuation with the original stale heads.
        let replay = ledger
            .append_unreplayed(
                operation,
                expected.clone(),
                request.clone(),
                vec![],
                fingerprint,
            )
            .unwrap();
        assert_eq!(replay.historical, winner.historical);
        assert_eq!(
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap()
                .0,
            committed.0
        );

        let mut changed = request.clone();
        changed.text = Some("different request".into());
        let changed_fingerprint = Ledger::fingerprint(&("append", &changed, &sources)).unwrap();
        let error = ledger
            .append_unreplayed(
                operation,
                expected.clone(),
                changed,
                vec![],
                changed_fingerprint,
            )
            .err()
            .unwrap();
        assert!(error.to_string().contains("operation fingerprint conflict"));

        let fingerprint = Ledger::fingerprint(&("append", &request, &sources)).unwrap();
        let error = ledger
            .append_unreplayed(Uuid::new_v4(), expected, request, vec![], fingerprint)
            .err()
            .unwrap();
        assert!(error.to_string().contains("stale conversation heads"));
        assert_eq!(
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap()
                .0,
            committed.0
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
