use super::*;
use crate::{conversation::*, storage::*};
use std::{collections::BTreeMap, fs, sync::Arc};

#[test]
fn deleted_anchor_between_historical_read_and_current_snapshot_refuses() {
    let path = std::env::temp_dir().join(format!("buddy-resolution-{}", Uuid::new_v4()));
    let store = Arc::new(
        Store::open(StorePaths {
            data: path.join("data"),
            cache: path.join("cache"),
            credentials: path.join("credentials"),
        })
        .unwrap(),
    );
    let ledger = Ledger::new(store.clone());
    let c = Uuid::new_v4();
    ledger.create(c, Uuid::new_v4(), 1).unwrap();
    let append = || {
        let id = Uuid::new_v4();
        ledger
            .append(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, c).unwrap(),
                Turn {
                    id,
                    conversation: c,
                    exchange: Uuid::new_v4(),
                    sequence: 0,
                    role: Role::User,
                    mode: Mode::Reader,
                    outcome: Outcome::Interpreted,
                    text: Some("orbit".into()),
                    sources: vec![],
                    correction_of: None,
                    created_ms: 1,
                    updated_ms: 1,
                    completion: None,
                },
                vec![],
            )
            .unwrap();
        id
    };
    let prior = append();
    let prepared = ledger
        .prepare_request_evidence(
            c,
            prior,
            None,
            RequestSource::NoDocument,
            ReceivingBudget {
                context: ContextBudget {
                    max_turns: 64,
                    max_text_bytes: 65536,
                    provider_token_limit: Some(1_000_000),
                },
                max_encoded_bytes: 524288,
                max_passages: 2,
                max_passage_bytes: 16384,
                neighbors: false,
            },
            None,
            |s| Ok(s.len()),
        )
        .unwrap();
    let operation = Uuid::new_v4();
    ledger
        .persist_request_evidence(operation, 1, &prepared)
        .unwrap();
    let current = append();
    let before = store.manifests().unwrap().len();
    let hook_store = store.clone();
    AFTER_HISTORY.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut envelope = hook_store
                .value(
                    Namespace::Conversation,
                    evidence::RequestEvidence::identity(operation),
                )
                .unwrap()
                .unwrap();
            envelope.parents = [envelope.revision_id].into();
            envelope.revision_id = Uuid::new_v4();
            envelope.operation_id = Uuid::new_v4();
            envelope.kind = Kind::Tombstone;
            envelope.payload = serde_json::Value::Null;
            envelope.media_descriptors.clear();
            hook_store.commit(vec![envelope], BTreeMap::new()).unwrap();
        }));
    });
    let error = ledger
        .resolve_request_source(
            c,
            current,
            SourceDirective::Inherit {
                request_turn: prior,
                operation: Some(operation),
                expected: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<SourceResolutionRefusal>(),
        Some(&SourceResolutionRefusal::AnchorUnavailable)
    );
    assert_eq!(store.manifests().unwrap().len(), before + 1);
    assert!(AFTER_HISTORY.with(|hook| hook.borrow().is_none()));
    drop(ledger);
    drop(store);
    let resolved = path.canonicalize().unwrap();
    assert_eq!(
        resolved.parent(),
        Some(std::env::temp_dir().canonicalize().unwrap().as_path())
    );
    assert!(resolved
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("buddy-resolution-"));
    fs::remove_dir_all(resolved).unwrap();
}
