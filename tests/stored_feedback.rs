use remarkable_reader_buddy::{
    conversation::{feedback::*, *},
    conversation_feedback::{describe, FeedbackKind},
    storage::{sync::mode::SyncModeChoice, *},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

const SECRET: &str = "Bearer SECRET-CANARY request answer auth@example.invalid";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-stored-feedback-{}", Uuid::new_v4())))
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
    fn files(&self) -> BTreeMap<PathBuf, String> {
        fn visit(path: &std::path::Path, result: &mut BTreeMap<PathBuf, String>) {
            if path.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(&entry.unwrap().path(), result);
                }
            } else {
                result.insert(path.to_path_buf(), digest(&fs::read(path).unwrap()));
            }
        }
        let mut files = BTreeMap::new();
        visit(&self.0, &mut files);
        files
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn seed(
    store: &Arc<Store>,
    outcome: Outcome,
    reason: AttemptReason,
) -> (Ledger, Turn, OutcomeFact) {
    let ledger = Ledger::new(store.clone());
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let mut turn = Turn {
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
        created_ms: 2,
        updated_ms: 2,
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
    turn.outcome = outcome;
    turn.text = Some(SECRET.into());
    turn.updated_ms = 3;
    ledger
        .record_attempt_outcome(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            ledger.expected(Namespace::Conversation, turn.id).unwrap(),
            turn.clone(),
            reason,
        )
        .unwrap();
    let fact = ledger
        .inspect(conversation, false)
        .unwrap()
        .into_iter()
        .find_map(|record| {
            if let Record::OutcomeFact(fact) = record {
                Some(fact)
            } else {
                None
            }
        })
        .unwrap();
    (ledger, turn, fact)
}
fn head(store: &Store, id: Uuid) -> Envelope {
    store.value(Namespace::Conversation, id).unwrap().unwrap()
}
fn edit(store: &Store, old: &Envelope, change: impl FnOnce(&mut Envelope)) {
    let mut value = old.clone();
    value.parents = [old.revision_id].into();
    value.revision_id = Uuid::new_v4();
    value.operation_id = Uuid::new_v4();
    value.actor_id = store.actor_id;
    change(&mut value);
    store.commit(vec![value], BTreeMap::new()).unwrap();
}
fn replicate(source: &Store, target: &Store) {
    let mut pending = source.manifests().unwrap();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|manifest| {
            let objects = manifest
                .records
                .iter()
                .chain(&manifest.media)
                .map(|r| (r.sha256.clone(), source.read_object(r).unwrap()))
                .collect();
            target.import(manifest.clone(), objects).is_err()
        });
        assert!(pending.len() < before);
    }
}

#[test]
fn actual_terminal_facts_reopen_with_exact_identity_static_text_and_1024_byte_bound() {
    for outcome in [
        Outcome::Failed,
        Outcome::Canceled,
        Outcome::ReconcileRequired,
    ] {
        for reason in [
            AttemptReason::RequestFailed,
            AttemptReason::NativeOutputUnavailable,
            AttemptReason::NoSelection,
            AttemptReason::TranscriptionDisagreement,
            AttemptReason::ProviderFailure,
            AttemptReason::Canceled,
            AttemptReason::OutputPending,
            AttemptReason::SubmittedUnverified,
            AttemptReason::DeviceUncertain,
            AttemptReason::NoSuccessor,
            AttemptReason::InvalidSuccessor,
        ] {
            let fixture = Fixture::new();
            let store = fixture.open();
            let (ledger, turn, fact) = seed(&store, outcome.clone(), reason);
            let expected = describe(&fact).unwrap().unwrap();
            drop(ledger);
            drop(store);
            let store = fixture.open();
            let ledger = Ledger::new(store.clone());
            let before = fixture.files();
            for _ in 0..2 {
                let actual = describe_stored(&ledger, turn.conversation, fact.id).unwrap();
                assert_eq!(actual, expected);
                let bytes = serde_json::to_vec(&actual).unwrap();
                assert!(bytes.len() <= MAX_DESCRIPTION_BYTES);
                assert_eq!(MAX_DESCRIPTION_BYTES, 1024);
                assert!(!String::from_utf8(bytes).unwrap().contains("SECRET-CANARY"));
            }
            assert_eq!(fixture.files(), before);
        }
    }
}

#[test]
fn nil_missing_foreign_and_type_reused_explicit_ids_refuse_without_writes() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    let (_, other, other_fact) = seed(&store, Outcome::Canceled, AttemptReason::Canceled);
    let before = fixture.files();
    for (conversation, id) in [
        (Uuid::nil(), fact.id),
        (turn.conversation, Uuid::nil()),
        (Uuid::new_v4(), fact.id),
        (turn.conversation, Uuid::new_v4()),
        (turn.conversation, other_fact.id),
        (other.conversation, fact.id),
        (turn.conversation, turn.id),
        (turn.conversation, turn.conversation),
    ] {
        assert!(describe_stored(&ledger, conversation, id).is_err());
    }
    assert_eq!(fixture.files(), before);
}

#[test]
fn explicit_root_and_fact_ids_capture_malformed_and_foreign_payloads() {
    for target in 0..2 {
        for payload in [
            Value::Null,
            json!({"record_kind":"unknown","record":{}}),
            json!({"record_kind":"outcome-fact","record":{"conversation":Uuid::new_v4()}}),
        ] {
            let fixture = Fixture::new();
            let store = fixture.open();
            let (ledger, turn, fact) =
                seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
            let id = if target == 0 {
                turn.conversation
            } else {
                fact.id
            };
            edit(&store, &head(&store, id), |e| e.payload = payload);
            let before = fixture.files();
            assert!(describe_stored(&ledger, turn.conversation, fact.id).is_err());
            assert_eq!(fixture.files(), before);
        }
    }
}

#[test]
fn explicit_tombstones_and_deleted_required_turns_refuse() {
    for target in 0..3 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
        let id = match target {
            0 => turn.conversation,
            1 => fact.id,
            _ => turn.id,
        };
        edit(&store, &head(&store, id), |e| {
            e.kind = Kind::Tombstone;
            e.payload = Value::Null;
        });
        let before = fixture.files();
        let error = describe_stored(&ledger, turn.conversation, fact.id).unwrap_err();
        if target < 2 {
            assert!(error
                .to_string()
                .contains("unsupported stored feedback history"));
        }
        assert_eq!(fixture.files(), before);
    }
}

#[test]
fn explicit_fact_id_retains_a_valid_but_foreign_payload_for_refusal() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    edit(&store, &head(&store, fact.id), |e| {
        e.payload["record"]["conversation"] = json!(Uuid::new_v4());
    });
    let before = fixture.files();
    let error = describe_stored(&ledger, turn.conversation, fact.id).unwrap_err();
    assert!(error.to_string().contains("foreign stored feedback record"));
    assert_eq!(fixture.files(), before);
}

#[test]
fn malformed_stored_values_do_not_leak_through_refusal_diagnostics() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    edit(&store, &head(&store, fact.id), |e| {
        e.payload["record_kind"] = json!(SECRET);
    });
    let before = fixture.files();
    let error = describe_stored(&ledger, turn.conversation, fact.id).unwrap_err();
    assert_eq!(error.to_string(), "invalid stored feedback record");
    assert!(!format!("{error:?}").contains("SECRET-CANARY"));
    assert_eq!(fixture.files(), before);
}

#[test]
fn all_heads_of_matching_records_include_foreign_and_null_tombstone_siblings() {
    for sibling in 0..3 {
        let fixture = Fixture::new();
        let peer_fixture = Fixture::new();
        let store = fixture.open();
        let peer = peer_fixture.open();
        let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
        // Select this second fact only through its conversation payload, not
        // either explicit ID. A nonmatching peer head must still be retained.
        let mut child = head(&store, fact.id);
        let child_id = Uuid::new_v4();
        child.record_id = child_id;
        child.revision_id = Uuid::new_v4();
        child.parents.clear();
        child.operation_id = Uuid::new_v4();
        child.payload["record"]["id"] = json!(child_id);
        store.commit(vec![child], BTreeMap::new()).unwrap();
        replicate(&store, &peer);
        let original = head(&store, child_id);
        edit(&store, &original, |_| {});
        edit(&peer, &original, |e| match sibling {
            1 => e.payload["record"]["conversation"] = json!(Uuid::new_v4()),
            2 => {
                e.kind = Kind::Tombstone;
                e.payload = Value::Null;
            }
            _ => (),
        });
        replicate(&peer, &store);
        assert_eq!(
            store
                .heads(Namespace::Conversation, child_id)
                .unwrap()
                .len(),
            2
        );
        let before = fixture.files();
        assert!(describe_stored(&ledger, turn.conversation, fact.id).is_err());
        assert_eq!(fixture.files(), before);
    }
}

#[test]
fn completed_and_selected_schema_records_are_never_ordinary_successes() {
    for (schema, outcome) in [
        (1, Outcome::Completed),
        (2, Outcome::Completed),
        (2, Outcome::Failed),
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
        edit(&store, &head(&store, fact.id), |e| {
            e.domain_schema_version = schema;
            e.payload["record"]["outcome"] = json!(outcome);
        });
        let before = fixture.files();
        if schema == 1 {
            // Typed serde serialization above uses canonical kebab-case. The
            // actual public inspector must reach the domain completion guard,
            // rather than fail parsing an invented enum spelling.
            let domain = ledger.inspect(turn.conversation, false).unwrap_err();
            assert!(domain.to_string().contains("invalid attempt outcome fact"));
        }
        let error = describe_stored(&ledger, turn.conversation, fact.id).unwrap_err();
        assert_eq!(
            error.to_string(),
            if schema == 1 {
                "invalid stored feedback record"
            } else {
                "unsupported stored feedback history"
            }
        );
        assert_eq!(fixture.files(), before);
    }
}

#[test]
fn fact_membership_requires_an_existing_original_turn_and_complete_chronology() {
    for target in 0..4 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
        let id = if target < 2 { fact.id } else { turn.id };
        edit(&store, &head(&store, id), |e| match target {
            0 => e.payload["record"]["turn"] = json!(Uuid::new_v4()),
            1 => e.payload["record"]["turn"] = json!(Uuid::nil()),
            2 => e.payload["record"]["conversation"] = json!(Uuid::new_v4()),
            _ => e.payload["record"]["sequence"] = json!(7),
        });
        let before = fixture.files();
        assert!(describe_stored(&ledger, turn.conversation, fact.id).is_err());
        assert_eq!(fixture.files(), before);
    }
}

#[test]
fn older_fact_does_not_need_the_later_turn_outcome() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, mut turn, fact) = seed(
        &store,
        Outcome::ReconcileRequired,
        AttemptReason::DeviceUncertain,
    );
    turn.outcome = Outcome::Failed;
    turn.updated_ms += 1;
    ledger
        .record_attempt_outcome(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, turn.conversation)
                .unwrap(),
            ledger.expected(Namespace::Conversation, turn.id).unwrap(),
            turn.clone(),
            AttemptReason::NoSuccessor,
        )
        .unwrap();
    let before = fixture.files();
    let old = describe_stored(&ledger, turn.conversation, fact.id).unwrap();
    assert_eq!(old.kind, FeedbackKind::Reconciliation);
    assert_eq!(old.fact, fact.id);
    assert_eq!(fixture.files(), before);
}

#[test]
fn ordinary_modes_are_supported_and_selected_modes_refuse_without_writes() {
    for choice in [
        SyncModeChoice::LocalOnly,
        SyncModeChoice::IsolatedBackup {
            binding_sha256: "a".repeat(64),
        },
        SyncModeChoice::SharedGroup {
            group: Uuid::new_v4(),
            binding_sha256: "b".repeat(64),
            root_id: "fixture-root".into(),
        },
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
        let allowed = matches!(choice, SyncModeChoice::LocalOnly);
        let current = store.sync_mode_snapshot().unwrap();
        store
            .set_sync_mode(&current.token, Uuid::new_v4(), choice)
            .unwrap();
        let before = fixture.files();
        assert_eq!(
            describe_stored(&ledger, turn.conversation, fact.id).is_ok(),
            allowed
        );
        assert_eq!(fixture.files(), before);
        // The receiver releases its read custody on both success and refusal.
        let current = store.sync_mode_snapshot().unwrap();
        store
            .set_sync_mode(&current.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
            .unwrap();
    }
}

#[test]
fn receiving_is_metadata_only_and_does_not_decode_source_payloads() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    let mut source = head(&store, fact.id);
    source.namespace = Namespace::Source;
    source.record_id = Uuid::new_v4();
    source.revision_id = Uuid::new_v4();
    source.parents.clear();
    source.operation_id = Uuid::new_v4();
    source.payload = json!({"record_kind":"unsupported-source-fixture","record":{"conversation":turn.conversation,"private":SECRET}});
    store.commit(vec![source], BTreeMap::new()).unwrap();
    assert!(ledger.inspect(turn.conversation, false).is_err());
    let before = fixture.files();
    assert_eq!(
        describe_stored(&ledger, turn.conversation, fact.id)
            .unwrap()
            .fact,
        fact.id
    );
    assert_eq!(fixture.files(), before);
}

#[test]
fn unavailable_committed_storage_refuses_before_a_partial_description() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    let manifest = store.manifests().unwrap().pop().unwrap();
    let object = manifest.records.first().unwrap();
    let generation: Uuid = files::json(&store.paths.data.join("CURRENT"), 128).unwrap();
    let path = store
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects")
        .join(&object.sha256);
    fs::write(path, b"deliberately corrupt fixture object").unwrap();
    drop(ledger);
    drop(store);
    let store = fixture.open();
    assert!(store.unavailable_commits().unwrap() > 0);
    let ledger = Ledger::new(store);
    let before = fixture.files();
    assert!(describe_stored(&ledger, turn.conversation, fact.id).is_err());
    assert_eq!(fixture.files(), before);
}

#[test]
fn cumulative_metadata_limit_refuses_without_truncating_feedback_context() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    for _ in 0..17 {
        let mut extra = head(&store, turn.id);
        extra.record_id = Uuid::new_v4();
        extra.revision_id = Uuid::new_v4();
        extra.parents.clear();
        extra.operation_id = Uuid::new_v4();
        extra.payload["record"]["id"] = json!(extra.record_id);
        extra.payload["record"]["text"] = json!("x".repeat(512 * 1024));
        store.commit(vec![extra], BTreeMap::new()).unwrap();
    }
    let before = fixture.files();
    let identity = turn.conversation.to_string();
    let underlying = store
        .snapshot_heads_matching(&[Namespace::Conversation], MAX_ITEMS, |e| {
            e.record_id == turn.conversation
                || e.payload["record"]["conversation"].as_str() == Some(&identity)
                || e.payload["record"]["acknowledgment"]["conversation"].as_str() == Some(&identity)
        })
        .unwrap_err();
    assert!(underlying
        .to_string()
        .contains("inspection exceeds metadata byte bound"));
    let error = describe_stored(&ledger, turn.conversation, fact.id).unwrap_err();
    assert_eq!(error.to_string(), "stored feedback snapshot unavailable");
    assert_eq!(fixture.files(), before);
}
