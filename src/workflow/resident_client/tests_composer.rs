use super::*;
use crate::{
    conversation::{
        BindingAcquisition, BindingReceipt, Ledger, Mode, Outcome as TurnOutcome, Role,
        SourceObservation, Turn,
    },
    storage::{Fault, Namespace},
};
use remarkable_open_sdk::native_adapter::composer as wire;

fn range_state() -> wire::RangeState {
    wire::RangeState {
        identity: wire::RangeIdentity {
            facts: facts(),
            visit: id(20),
            range: id(21),
        },
        revision: 4,
    }
}

fn effect(operation: OperationId, command: wire::Command) -> Frame {
    Frame {
        scope: scope(),
        sequence: 1,
        operation,
        message: Message::Request(Command::Composer(command)),
    }
}

fn provision(operation: u32) -> Frame {
    effect(
        OperationId(id(operation)),
        wire::Command::Provision(facts()),
    )
}

fn provisioned(value: wire::RangeState) -> Reply {
    Reply::Composer(wire::Reply::Provisioned(wire::Outcome::Verified {
        attempts: wire::Attempts::from_bits(7).unwrap(),
        value,
    }))
}

fn cleared(value: wire::ClearReceipt) -> Reply {
    Reply::Composer(wire::Reply::Cleared(wire::Outcome::Verified {
        attempts: wire::Attempts::from_bits(7).unwrap(),
        value,
    }))
}

fn record_reply(store: &Store, request: &Frame, reply: Reply) -> Result<()> {
    finish(
        store,
        request,
        Outcome::HistoricalReply {
            frame: encode(&response(request, reply))?,
        },
    )
}

fn bind_current_page(ledger: &Ledger, conversation: Uuid, facts: &ReaderFacts) {
    let document = Uuid::from_bytes(*facts.page().document.as_bytes());
    let page = Uuid::from_bytes(*facts.page().page.as_bytes());
    let order: Vec<_> = facts
        .order()
        .iter()
        .map(|id| Uuid::from_bytes(*id.as_bytes()))
        .collect();
    let operation = Uuid::new_v4();
    let receipt = BindingReceipt {
        operation,
        conversation,
        request_fingerprint: "fixture request".into(),
        adapter_fingerprint: "fixture adapter".into(),
        source: SourceObservation {
            document,
            page,
            session: "fixture session".into(),
            visit: "fixture visit".into(),
            content_revision: Some("fixture content".into()),
            order_revision: Some("fixture order".into()),
            capability_revision: "fixture profile".into(),
            evidence_procedure: "fixture".into(),
        },
        intended_target: Some(page),
        observed_document: document,
        observed_page: page,
        expected_order: order.clone(),
        observed_order: order,
        persisted_revision: "fixture revision".into(),
        observed_ms: 2,
        evidence_origin: "synthetic".into(),
        acquisition: BindingAcquisition::ExistingCurrent,
    };
    ledger
        .bind(
            operation,
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            ledger
                .expected(Namespace::Conversation, Ledger::binding_id(document, page))
                .unwrap(),
            receipt,
        )
        .unwrap();
}

#[test]
fn reservation_and_typed_outcomes_survive_reopen_without_replay() {
    let refused = Reply::Composer(wire::Reply::Provisioned(wire::Outcome::Refused {
        attempts: wire::Attempts::from_bits(5).unwrap(),
    }));
    let unknown = Reply::Composer(wire::Reply::Provisioned(wire::Outcome::Unknown {
        attempts: wire::Attempts::from_bits(3).unwrap(),
    }));
    for (reply, allows_new_operation) in [
        (None, false),
        (Some(refused), true),
        (Some(unknown), false),
        (Some(provisioned(range_state())), true),
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let conversation = Uuid::new_v4();
        let original = provision(50);
        claim(&store, conversation, &original).unwrap();
        if let Some(reply) = &reply {
            record_reply(&store, &original, reply.clone()).unwrap();
        }
        drop(store);
        let store = fixture.open();
        let saved = attempts(&store).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(*saved[0].request.frame(), original);
        assert_eq!(saved[0].reply, reply);
        assert!(claim(&store, conversation, &original).is_err());
        assert!(claim(&store, Uuid::new_v4(), &original).is_err());
        let select = Frame {
            message: Message::Request(Command::SelectAdjacent(SelectAdjacent {
                expected: facts(),
                target: id(12),
            })),
            ..request(51, 13)
        };
        assert_eq!(
            claim(&store, conversation, &select).is_ok(),
            allows_new_operation
        );
        assert_eq!(
            attempts(&store).unwrap().len(),
            if allows_new_operation { 2 } else { 1 }
        );
    }
}

#[test]
fn mismatched_and_generic_replies_preserve_the_original_reservation() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let provision_call = provision(50);
    let conversation = Uuid::new_v4();
    claim(&store, conversation, &provision_call).unwrap();
    let path = store.paths.data.join(JOURNAL_FILE);
    let provision_bytes = fs::read(&path).unwrap();
    let mut wrong = range_state();
    wrong.identity.facts = ReaderFacts::new(facts().page(), vec![id(11), id(13)]).unwrap();
    for reply in [provisioned(wrong), Reply::Indeterminate] {
        assert!(record_reply(&store, &provision_call, reply).is_err());
        assert_eq!(fs::read(&path).unwrap(), provision_bytes);
    }
    let state = range_state();
    record_reply(&store, &provision_call, provisioned(state.clone())).unwrap();

    let clear = effect(
        OperationId(id(51)),
        wire::Command::Clear(wire::Draft {
            state: state.clone(),
            units: vec![b'a' as u16],
        }),
    );
    claim(&store, conversation, &clear).unwrap();
    let clear_bytes = fs::read(&path).unwrap();
    let receipt = wire::ClearReceipt {
        identity: state.identity,
        before_revision: state.revision,
        after_revision: state.revision + 1,
    };
    let mut wrong_range = receipt.clone();
    wrong_range.identity.range = id(22);
    let mut wrong_visit = receipt.clone();
    wrong_visit.identity.visit = id(23);
    let mut wrong_revision = receipt;
    wrong_revision.before_revision -= 1;
    for reply in [
        cleared(wrong_range),
        cleared(wrong_visit),
        cleared(wrong_revision),
        Reply::Indeterminate,
        Reply::Rejected(Rejection::BeforeDispatch),
    ] {
        assert!(record_reply(&store, &clear, reply).is_err());
        assert_eq!(fs::read(&path).unwrap(), clear_bytes);
    }
    drop(store);
    let store = fixture.open();
    assert!(attempts(&store).unwrap()[1].reply.is_none());
    assert!(claim(&store, conversation, &clear).is_err());
    assert!(claim(&store, conversation, &provision(52)).is_err());

    let unknown = Reply::Composer(wire::Reply::Cleared(wire::Outcome::Unknown {
        attempts: wire::Attempts::from_bits(7).unwrap(),
    }));
    record_reply(&store, &clear, unknown.clone()).unwrap();
    drop(store);
    let store = fixture.open();
    assert_eq!(attempts(&store).unwrap()[1].reply, Some(unknown));
    assert!(claim(&store, conversation, &clear).is_err());
    assert!(claim(&store, conversation, &provision(52)).is_err());
}

#[test]
fn clear_proof_requires_a_retained_exact_user_turn_and_range() {
    use crate::workflow::resident_client::composer::{require_page, RetainedUser};

    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let state = range_state();
    assert!(require_page(&store, conversation, &facts()).is_err());
    bind_current_page(&ledger, conversation, &facts());
    require_page(&store, conversation, &facts()).unwrap();
    let other_page = ReaderFacts::new(
        PageKey {
            document: facts().page().document,
            page: id(12),
        },
        facts().order().to_vec(),
    )
    .unwrap();
    assert!(require_page(&store, conversation, &other_page).is_err());
    let text = "A written draft \u{fffd}🖊";
    let draft = wire::Draft {
        state: state.clone(),
        units: text.encode_utf16().collect(),
    };
    let exchange = Uuid::new_v4();
    let user = Turn {
        id: crate::writer::stage(exchange, "composer-user"),
        conversation,
        exchange,
        sequence: 0,
        role: Role::User,
        mode: Mode::Writer,
        outcome: TurnOutcome::Interpreted,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 3,
        updated_ms: 3,
        completion: None,
    };
    let user_id = user.id;
    let expected = ledger
        .expected(Namespace::Conversation, conversation)
        .unwrap();
    assert!(RetainedUser::append_user(&ledger, expected, user.clone(), draft.clone()).is_err());

    let provision_call = provision(50);
    claim(&store, conversation, &provision_call).unwrap();
    record_reply(&store, &provision_call, provisioned(state)).unwrap();
    let assistant = Turn {
        role: Role::Assistant,
        outcome: TurnOutcome::Generated,
        ..user.clone()
    };
    let expected = ledger
        .expected(Namespace::Conversation, conversation)
        .unwrap();
    assert!(
        RetainedUser::append_user(&ledger, expected.clone(), assistant, draft.clone()).is_err()
    );
    let mut wrong_text = draft.clone();
    wrong_text.units.push(b' ' as u16);
    assert!(
        RetainedUser::append_user(&ledger, expected.clone(), user.clone(), wrong_text).is_err()
    );
    let malformed = wire::Draft {
        units: text
            .encode_utf16()
            .map(|unit| if unit == 0xfffd { 0xd800 } else { unit })
            .collect(),
        ..draft.clone()
    };
    assert!(RetainedUser::append_user(&ledger, expected.clone(), user.clone(), malformed).is_err());
    let mut wrong_range = draft.clone();
    wrong_range.state.identity.range = id(22);
    assert!(
        RetainedUser::append_user(&ledger, expected.clone(), user.clone(), wrong_range).is_err()
    );

    store.set_fault(Fault::BeforeCommit).unwrap();
    assert!(
        RetainedUser::append_user(&ledger, expected.clone(), user.clone(), draft.clone()).is_err()
    );
    assert!(store
        .value(Namespace::Conversation, user_id)
        .unwrap()
        .is_none());
    store.set_fault(Fault::AfterCommit).unwrap();
    assert!(RetainedUser::append_user(&ledger, expected, user.clone(), draft.clone()).is_err());
    drop(ledger);
    drop(store);

    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    assert!(store
        .value(Namespace::Conversation, user_id)
        .unwrap()
        .is_some());
    let expected = ledger
        .expected(Namespace::Conversation, conversation)
        .unwrap();
    assert!(
        RetainedUser::append_user(&ledger, expected.clone(), user.clone(), draft.clone()).is_err()
    );
    let exchange = Uuid::new_v4();
    let next_user = Turn {
        id: crate::writer::stage(exchange, "composer-user"),
        exchange,
        ..user
    };
    let next_user_id = next_user.id;
    let retained = RetainedUser::append_user(&ledger, expected, next_user, draft).unwrap();
    assert!(store
        .value(Namespace::Conversation, next_user_id)
        .unwrap()
        .is_some());
    assert!(retained.check(&store, OperationId(id(51))).is_err());
    let operation = retained.operation();
    let clear_id = crate::writer::stage(next_user_id, "native-composer-clear");
    assert_eq!(
        operation,
        OperationId(SdkUuid::parse(&clear_id.to_string()).unwrap())
    );
    retained.check(&store, operation).unwrap();
    let clear = effect(operation, wire::Command::Clear(retained.draft().clone()));
    claim(&store, retained.conversation(), &clear).unwrap();
    let reply = cleared(wire::ClearReceipt {
        identity: retained.draft().state.identity.clone(),
        before_revision: retained.draft().state.revision,
        after_revision: retained.draft().state.revision + 1,
    });
    record_reply(&store, &clear, reply.clone()).unwrap();
    drop(retained);
    drop(ledger);
    drop(store);
    let store = fixture.open();
    assert_eq!(attempts(&store).unwrap()[1].reply, Some(reply));
    assert!(claim(&store, conversation, &clear).is_err());
}
