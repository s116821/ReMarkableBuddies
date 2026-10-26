use remarkable_reader_buddy::{
    conversation::{feedback::*, *},
    conversation_feedback::{describe, FeedbackKind},
    conversation_layout::{plan, Geometry, PresentationIdentity},
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

fn context(turns: Vec<Turn>) -> ContextView {
    ContextView {
        turns,
        sources: vec![],
        captures: vec![],
        normalized_text: vec![],
        legacy_captures: vec![],
        development_captures: vec![],
        missing_media: vec![],
        selection: None,
    }
}
fn identity(conversation: Uuid) -> PresentationIdentity {
    PresentationIdentity::Persisted {
        conversation,
        document: Uuid::new_v4(),
        page: Uuid::new_v4(),
    }
}

#[test]
fn reopened_actual_feedback_composes_without_replacing_original_content() {
    for (outcome, reason, kind) in [
        (
            Outcome::Failed,
            AttemptReason::ProviderFailure,
            FeedbackKind::Error,
        ),
        (
            Outcome::Canceled,
            AttemptReason::Canceled,
            FeedbackKind::Canceled,
        ),
        (
            Outcome::ReconcileRequired,
            AttemptReason::DeviceUncertain,
            FeedbackKind::Reconciliation,
        ),
        (
            Outcome::Canceled,
            AttemptReason::SubmittedUnverified,
            FeedbackKind::Reconciliation,
        ),
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let (ledger, turn, fact) = seed(&store, outcome, reason);
        drop(ledger);
        drop(store);
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let ctx = context(vec![turn.clone()]);
        let original = serde_json::to_vec(&ctx.turns).unwrap();
        let id = identity(turn.conversation);
        let draft = "user-edited draft\nkeep it".to_owned();
        let geometry = Geometry {
            header_end: 0.1,
            input_end: 0.3,
            status_end: 0.4,
        };
        let before = fixture.files();
        let existing = plan(id, &ctx, &draft, geometry).unwrap();
        let result = plan_stored(&ledger, id, &ctx, &draft, geometry, fact.id).unwrap();
        assert_eq!(
            result.description,
            describe_stored(&ledger, turn.conversation, fact.id).unwrap()
        );
        assert_eq!(result.description, describe(&fact).unwrap().unwrap());
        assert_eq!(result.description.kind, kind);
        assert_eq!(
            (
                result.description.fact,
                result.description.conversation,
                result.description.turn
            ),
            (fact.id, turn.conversation, turn.id)
        );
        assert_eq!(result.exchange, turn.exchange);
        assert_eq!(
            (
                result.page.conversation,
                result.page.document,
                result.page.page
            ),
            (existing.conversation, existing.document, existing.page)
        );
        assert_eq!(
            (
                result.page.header,
                result.page.input,
                result.page.status,
                result.page.history
            ),
            (
                existing.header,
                existing.input,
                existing.status,
                existing.history
            )
        );
        assert_eq!(result.page.title, existing.title);
        assert_eq!(result.page.exchanges.len(), 1);
        assert_eq!(result.page.exchanges[0].turns.len(), 1);
        assert!(std::ptr::eq(
            result.page.exchanges[0].turns[0],
            &ctx.turns[0]
        ));
        assert!(std::ptr::eq(result.page.draft, draft.as_str()));
        assert_eq!(serde_json::to_vec(&ctx.turns).unwrap(), original);
        // Original private content remains the caller's unchanged history; only
        // the new description must exclude it, not the borrowed original page.
        assert_eq!(
            result.page.exchanges[0].turns[0].text.as_deref(),
            Some(SECRET)
        );
        let bytes = serde_json::to_vec(&result.description).unwrap();
        assert!(bytes.len() <= MAX_DESCRIPTION_BYTES);
        assert!(!String::from_utf8(bytes).unwrap().contains("SECRET-CANARY"));
        assert_eq!(fixture.files(), before);
    }
}

#[test]
fn every_selected_turn_field_must_match_the_stored_snapshot() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    let id = identity(turn.conversation);
    let before = fixture.files();
    let completion = json!({"native_operation":Uuid::new_v4(),"exact_text":SECRET,"observation":{
        "document":Uuid::new_v4(),"page":Uuid::new_v4(),"session":SECRET,"visit":"visit",
        "content_revision":null,"order_revision":null,"capability_revision":"fixture","evidence_procedure":SECRET
    },"procedure":SECRET});
    for (field, value) in [
        ("id", json!(Uuid::new_v4())),
        ("conversation", json!(Uuid::new_v4())),
        ("exchange", json!(Uuid::new_v4())),
        ("sequence", json!(9)),
        ("role", json!(Role::Assistant)),
        ("mode", json!(Mode::Writer)),
        ("outcome", json!(Outcome::Canceled)),
        ("text", json!("changed")),
        ("sources", json!([Uuid::new_v4()])),
        ("correction_of", json!(Uuid::new_v4())),
        ("created_ms", json!(9)),
        ("updated_ms", json!(9)),
        ("completion", completion),
    ] {
        let mut value_turn = serde_json::to_value(&turn).unwrap();
        value_turn[field] = value;
        let changed: Turn = serde_json::from_value(value_turn).unwrap();
        let ctx = context(vec![changed]);
        let error =
            plan_stored(&ledger, id, &ctx, "draft", Geometry::default(), fact.id).unwrap_err();
        assert_eq!(
            error.to_string(),
            "stored feedback layout turn mismatch",
            "{field}"
        );
        assert!(!format!("{error:?}").contains("SECRET-CANARY"));
    }
    assert_eq!(fixture.files(), before);
}

#[test]
fn original_planner_and_explicit_identity_refusals_still_apply() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    let id = identity(turn.conversation);
    let ctx = context(vec![turn.clone()]);
    let before = fixture.files();
    for rejected in [
        PresentationIdentity::Unbound,
        PresentationIdentity::Legacy,
        PresentationIdentity::Persisted {
            conversation: turn.conversation,
            document: Uuid::nil(),
            page: Uuid::new_v4(),
        },
        identity(Uuid::nil()),
        identity(Uuid::new_v4()),
    ] {
        assert!(plan_stored(
            &ledger,
            rejected,
            &ctx,
            "draft",
            Geometry::default(),
            fact.id
        )
        .is_err());
    }
    for fact_id in [Uuid::nil(), Uuid::new_v4(), turn.id, turn.conversation] {
        assert!(plan_stored(&ledger, id, &ctx, "draft", Geometry::default(), fact_id).is_err());
    }
    for bad in [
        context(vec![]),
        context(vec![turn.clone(), turn.clone()]),
        context(vec![turn.clone(); MAX_ITEMS + 1]),
    ] {
        assert!(plan_stored(&ledger, id, &bad, "draft", Geometry::default(), fact.id).is_err());
    }
    let bad_geometry = Geometry {
        header_end: f32::NAN,
        ..Geometry::default()
    };
    assert!(plan_stored(&ledger, id, &ctx, "draft", bad_geometry, fact.id).is_err());
    let mut other = turn.clone();
    other.id = Uuid::new_v4();
    other.exchange = Uuid::new_v4();
    other.sequence = 1;
    other.role = Role::Assistant;
    assert!(plan_stored(
        &ledger,
        id,
        &context(vec![turn, other]),
        "draft",
        Geometry::default(),
        fact.id
    )
    .is_err());
    assert_eq!(fixture.files(), before);
}

#[test]
fn older_fact_keeps_its_identity_while_context_must_match_the_later_turn() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, mut turn, old) = seed(
        &store,
        Outcome::ReconcileRequired,
        AttemptReason::DeviceUncertain,
    );
    let stale = context(vec![turn.clone()]);
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
    let ctx = context(vec![turn.clone()]);
    let id = identity(turn.conversation);
    let before = fixture.files();
    assert!(plan_stored(&ledger, id, &stale, "draft", Geometry::default(), old.id).is_err());
    let result = plan_stored(&ledger, id, &ctx, "draft", Geometry::default(), old.id).unwrap();
    assert_eq!(result.description.fact, old.id);
    assert_eq!(result.description.kind, FeedbackKind::Reconciliation);
    assert_eq!(result.page.exchanges[0].turns[0].outcome, Outcome::Failed);
    assert_eq!(fixture.files(), before);
}

#[test]
fn composed_receiving_preserves_snapshot_refusal_privacy_and_mode_release() {
    for case in 0..5 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
        let ctx = context(vec![turn.clone()]);
        let id = identity(turn.conversation);
        match case {
            0 => edit(&store, &head(&store, fact.id), |e| {
                e.payload["record_kind"] = json!(SECRET)
            }),
            1 => edit(&store, &head(&store, fact.id), |e| {
                e.kind = Kind::Tombstone;
                e.payload = Value::Null;
            }),
            2 => edit(&store, &head(&store, fact.id), |e| {
                e.domain_schema_version = 999
            }),
            3 => {
                let current = store.sync_mode_snapshot().unwrap();
                store
                    .set_sync_mode(
                        &current.token,
                        Uuid::new_v4(),
                        SyncModeChoice::IsolatedBackup {
                            binding_sha256: "a".repeat(64),
                        },
                    )
                    .unwrap();
            }
            _ => edit(&store, &head(&store, fact.id), |e| {
                e.payload["record"]["conversation"] = json!(Uuid::new_v4())
            }),
        }
        let before = fixture.files();
        let error =
            plan_stored(&ledger, id, &ctx, "draft", Geometry::default(), fact.id).unwrap_err();
        assert!(!format!("{error:?}").contains("SECRET-CANARY"));
        assert!(describe_stored(&ledger, turn.conversation, fact.id).is_err());
        assert_eq!(fixture.files(), before);
        let current = store.sync_mode_snapshot().unwrap();
        store
            .set_sync_mode(&current.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
            .unwrap();
    }
}

#[test]
fn feedback_remains_with_its_original_exchange_in_newest_first_history() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (ledger, turn, fact) = seed(&store, Outcome::Failed, AttemptReason::ProviderFailure);
    let mut later = turn.clone();
    later.id = Uuid::new_v4();
    later.exchange = Uuid::new_v4();
    later.sequence = 0;
    later.outcome = Outcome::Prepared;
    later.text = Some("later unchanged request".into());
    later.created_ms = 4;
    later.updated_ms = 4;
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, turn.conversation)
                .unwrap(),
            later.clone(),
            vec![],
        )
        .unwrap();
    let later = ledger
        .inspect(turn.conversation, false)
        .unwrap()
        .into_iter()
        .find_map(|record| match record {
            Record::Turn(stored) if stored.id == later.id => Some(stored),
            _ => None,
        })
        .unwrap();
    let ctx = context(vec![turn.clone(), later.clone()]);
    let before = fixture.files();
    let result = plan_stored(
        &ledger,
        identity(turn.conversation),
        &ctx,
        "draft",
        Geometry::default(),
        fact.id,
    )
    .unwrap();
    assert_eq!(
        result
            .page
            .exchanges
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        [later.exchange, turn.exchange]
    );
    assert_eq!(result.exchange, turn.exchange);
    assert_eq!(result.description.turn, turn.id);
    assert!(std::ptr::eq(
        result.page.exchanges[1].turns[0],
        &ctx.turns[0]
    ));
    assert!(std::ptr::eq(
        result.page.exchanges[0].turns[0],
        &ctx.turns[1]
    ));
    assert_eq!(fixture.files(), before);
}
