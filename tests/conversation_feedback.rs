use remarkable_reader_buddy::conversation::{
    AttemptReason, IntentRecordRef, IntentSelectionEvidence, IntentSettlement,
    IntentSettlementState, Outcome, OutcomeFact,
};
use remarkable_reader_buddy::conversation_feedback::{describe, FeedbackKind};
use remarkable_reader_buddy::storage::{Namespace, ObjectRef, Uuid};
fn fact(outcome: Outcome, reason: AttemptReason) -> OutcomeFact {
    OutcomeFact {
        id: Uuid::new_v4(),
        conversation: Uuid::new_v4(),
        turn: Uuid::new_v4(),
        outcome,
        reason,
        settlement: None,
    }
}
#[test]
fn settlement_diagnostics_are_not_serialized_and_fact_remains_exact() {
    let mut f = fact(Outcome::ReconcileRequired, AttemptReason::DeviceUncertain);
    let digest = "a".repeat(64);
    let reference = IntentRecordRef {
        namespace: Namespace::Conversation,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        object: ObjectRef {
            sha256: digest.clone(),
            bytes: 1,
        },
    };
    let secret="Bearer SECRET-SENTINEL\nprovider payload <script>bad</script>\nauth account@example.invalid";
    f.settlement = Some(IntentSettlement {
        operation: Uuid::new_v4(),
        request_fingerprint: digest.clone(),
        original_receipt: reference.clone(),
        original_root: reference.clone(),
        original_turn: reference,
        original_selection: IntentSelectionEvidence {
            group: Uuid::new_v4(),
            key_sha256: digest.clone(),
            binding_sha256: digest.clone(),
            store_generation: Uuid::new_v4(),
            aggregate_generation: Uuid::new_v4(),
            accepted_base_sha256: digest.clone(),
            selection_sha256: digest,
        },
        state: IntentSettlementState::Unknown,
        procedure: secret.into(),
        origin: secret.into(),
        evidence: vec![],
        media: vec![],
    });
    let original = serde_json::to_vec(&f).unwrap();
    let description = describe(&f).unwrap().unwrap();
    let wire = serde_json::to_value(&description).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 6);
    let serialized = serde_json::to_string(&wire).unwrap();
    for leaked in [
        "SECRET-SENTINEL",
        "Bearer",
        "payload",
        "<script>",
        "account@",
        "procedure",
        "settlement",
        "origin",
    ] {
        assert!(!serialized.contains(leaked));
    }
    assert_eq!(description.fact, f.id);
    assert_eq!(description.conversation, f.conversation);
    assert_eq!(description.turn, f.turn);
    assert_eq!(serde_json::to_vec(&f).unwrap(), original);
}
#[test]
fn uncertain_action_never_becomes_canceled_or_success() {
    for outcome in [
        Outcome::Failed,
        Outcome::Canceled,
        Outcome::ReconcileRequired,
    ] {
        for reason in [
            AttemptReason::OutputPending,
            AttemptReason::SubmittedUnverified,
            AttemptReason::DeviceUncertain,
        ] {
            let d = describe(&fact(outcome.clone(), reason)).unwrap().unwrap();
            assert_eq!(d.kind, FeedbackKind::Reconciliation);
            assert!(
                !d.text.contains("canceled")
                    && !d.text.contains("undone")
                    && !d.text.contains("delivered")
            );
        }
    }
    let d = describe(&fact(Outcome::ReconcileRequired, AttemptReason::Canceled))
        .unwrap()
        .unwrap();
    assert_eq!(d.kind, FeedbackKind::Reconciliation);
    assert_eq!(
        describe(&fact(Outcome::Canceled, AttemptReason::Canceled))
            .unwrap()
            .unwrap()
            .kind,
        FeedbackKind::Canceled
    );
}
#[test]
fn repeated_descriptions_keep_original_operation_identity_separate() {
    let original = fact(Outcome::Failed, AttemptReason::ProviderFailure);
    let later = fact(Outcome::Failed, AttemptReason::ProviderFailure);
    let first = describe(&original).unwrap().unwrap();
    assert_eq!(describe(&original).unwrap().unwrap(), first);
    let second = describe(&later).unwrap().unwrap();
    assert_eq!(first.text, second.text);
    assert_eq!(first.code, second.code);
    assert_ne!(first.fact, second.fact);
    assert_eq!(first.fact, original.id);
}
#[test]
fn completed_history_emits_no_new_receipt_and_unknown_or_nil_refuses() {
    assert!(
        describe(&fact(Outcome::Completed, AttemptReason::DeviceUncertain))
            .unwrap()
            .is_none()
    );
    for outcome in [Outcome::Prepared, Outcome::Interpreted, Outcome::Generated] {
        assert!(describe(&fact(outcome, AttemptReason::RequestFailed)).is_err());
    }
    for field in 0..3 {
        let mut f = fact(Outcome::Failed, AttemptReason::RequestFailed);
        match field {
            0 => f.id = Uuid::nil(),
            1 => f.conversation = Uuid::nil(),
            _ => f.turn = Uuid::nil(),
        };
        assert!(describe(&f).is_err());
    }
}
