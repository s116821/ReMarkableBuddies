use remarkable_open_sdk::{
    mock::MockPlatform,
    navigation::{LogicalDirection, NavigationOutcome, NavigationRequest},
    OperationId, PageObservation, Platform, Uuid as SdkUuid,
};
use remarkable_reader_buddy::{
    conversation::*,
    storage::*,
    workflow::sdk_buddy_decision::{
        begin_current, complete_successor, AssociationState, AssociationWitness,
        HistoricalLedgerLookup, ReadOnlyBindingLookup, Recognition, RecognitionWitness,
        SyntheticDecision, UnresolvedReason,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("buddy-historical-association-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("secrets"),
        }
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(Store::open(self.paths()).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("buddy-historical-association-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn expected(ledger: &Ledger, id: Uuid) -> ExpectedHeads {
    ledger.expected(Namespace::Conversation, id).unwrap()
}
fn create(ledger: &Ledger) -> Uuid {
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 10).unwrap();
    id
}
fn claim(conversation: Uuid, document: Uuid, source: Uuid, target: Uuid) -> BindingReceipt {
    BindingReceipt {
        acquisition: Default::default(),
        operation: Uuid::new_v4(),
        conversation,
        request_fingerprint: "synthetic immutable request".into(),
        adapter_fingerprint: "synthetic adapter".into(),
        source: SourceObservation {
            document,
            page: source,
            session: "synthetic-session".into(),
            visit: "synthetic-visit".into(),
            content_revision: Some("content".into()),
            order_revision: Some("order".into()),
            capability_revision: "synthetic-profile".into(),
            evidence_procedure: "synthetic".into(),
        },
        intended_target: Some(target),
        observed_document: document,
        observed_page: target,
        expected_order: vec![source],
        observed_order: vec![source, target],
        persisted_revision: "historical revision".into(),
        observed_ms: 11,
        evidence_origin: "synthetic".into(),
    }
}
fn bind(ledger: &Ledger, receipt: BindingReceipt) -> WriteResult {
    let key = Ledger::binding_id(receipt.observed_document, receipt.observed_page);
    ledger
        .bind(
            receipt.operation,
            expected(ledger, receipt.conversation),
            expected(ledger, key),
            receipt,
        )
        .unwrap()
}
fn candidate(ledger: &Ledger, receipt: &BindingReceipt) -> HistoricalPageAssociation {
    let HistoricalPageAssociationInspection::HistoricalCandidate(candidate) = ledger
        .inspect_page_association(
            receipt.conversation,
            receipt.observed_document,
            receipt.observed_page,
        )
        .unwrap()
    else {
        panic!("historical candidate")
    };
    *candidate
}
fn revise(store: &Store, mut envelope: Envelope) {
    envelope.parents = BTreeSet::from([envelope.revision_id]);
    envelope.revision_id = Uuid::new_v4();
    envelope.operation_id = Uuid::new_v4();
    store.commit(vec![envelope], BTreeMap::new()).unwrap();
}
fn manifests(store: &Store) -> Vec<u8> {
    serde_json::to_vec(&store.manifests().unwrap()).unwrap()
}
fn sdk(id: Uuid) -> SdkUuid {
    SdkUuid::parse(&id.to_string()).unwrap()
}

fn replicate(source: &Store, target: &Store) {
    let mut pending = source.manifests().unwrap();
    while !pending.is_empty() {
        let previous = pending.len();
        pending.retain(|manifest| {
            let objects = manifest
                .records
                .iter()
                .chain(&manifest.media)
                .map(|object| (object.sha256.clone(), source.read_object(object).unwrap()))
                .collect();
            target.import(manifest.clone(), objects).is_err()
        });
        assert!(pending.len() < previous, "fixture dependencies unresolved");
    }
}
fn assert_decision_unresolved(ledger: &Ledger, receipt: &BindingReceipt) {
    let platform = MockPlatform::new(
        sdk(Uuid::new_v4()),
        sdk(receipt.observed_document),
        receipt.observed_order.iter().copied().map(sdk).collect(),
        sdk(receipt.observed_page),
    );
    let observed = platform.observe_page().unwrap();
    let recognition = RecognitionWitness::modeled(
        observed.clone(),
        observed.page(),
        sdk(receipt.conversation),
        Recognition::OrdinarySource,
    );
    let before = manifests(ledger.store());
    assert!(matches!(
        begin_current(
            &observed,
            &observed,
            sdk(receipt.conversation),
            &HistoricalLedgerLookup::new(ledger),
            Some(&recognition),
        ),
        SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
    ));
    assert_eq!(platform.gesture_count(), 0);
    assert_eq!(before, manifests(ledger.store()));
}

#[test]
fn matching_history_preserves_exact_pair_and_never_becomes_current_reuse() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let conversation = create(&ledger);
    let receipt = claim(conversation, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let written = bind(&ledger, receipt.clone());
    let before = manifests(ledger.store());
    let inspected = candidate(&ledger, &receipt);
    assert_eq!(inspected.receipt(), &receipt);
    assert_eq!(
        inspected.root_revision(),
        written.current_root[0].revision_id
    );
    assert_eq!(
        inspected.binding_revision(),
        written.current_binding[0].revision_id
    );
    assert_eq!(before, manifests(ledger.store()));
    assert_decision_unresolved(&ledger, &receipt);

    let mut root = written.current_root[0].clone();
    let Record::Root(mut value) = serde_json::from_value(root.payload.clone()).unwrap() else {
        panic!("root")
    };
    value.updated_ms += 1;
    root.payload = serde_json::to_value(Record::Root(value)).unwrap();
    revise(ledger.store(), root);
    let later = candidate(&ledger, &receipt);
    assert_ne!(later.root_revision(), inspected.root_revision());
    assert_eq!(later.binding_revision(), inspected.binding_revision());
    assert_eq!(
        inspected.root_revision(),
        written.current_root[0].revision_id
    );
}

#[test]
fn missing_and_other_conversation_heads_remain_unknown() {
    use HistoricalPageAssociationInspection::Unresolved;
    use HistoricalPageAssociationUnresolved::*;
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let a = create(&ledger);
    let receipt = claim(a, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    assert_eq!(
        ledger
            .inspect_page_association(a, receipt.observed_document, receipt.observed_page)
            .unwrap(),
        Unresolved(BindingMissing)
    );
    assert_decision_unresolved(&ledger, &receipt);
    assert_eq!(
        ledger
            .inspect_page_association(
                Uuid::new_v4(),
                receipt.observed_document,
                receipt.observed_page,
            )
            .unwrap(),
        Unresolved(RootMissing)
    );
    let b = create(&ledger);
    bind(
        &ledger,
        claim(
            b,
            receipt.observed_document,
            receipt.source.page,
            receipt.observed_page,
        ),
    );
    assert_eq!(
        ledger
            .inspect_page_association(a, receipt.observed_document, receipt.observed_page)
            .unwrap(),
        Unresolved(OtherConversation)
    );
    assert_decision_unresolved(&ledger, &receipt);
    assert!(ledger
        .inspect_page_association(
            Uuid::nil(),
            receipt.observed_document,
            receipt.observed_page
        )
        .is_err());
}

#[test]
fn tombstones_and_malformed_historical_evidence_never_fall_through() {
    use HistoricalPageAssociationInspection::Unresolved;
    use HistoricalPageAssociationUnresolved::*;
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let receipt = claim(
        create(&ledger),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let written = bind(&ledger, receipt.clone());
    let mut binding = written.current_binding[0].clone();
    let Record::Binding(mut value) = serde_json::from_value(binding.payload.clone()).unwrap()
    else {
        panic!("binding")
    };
    value.receipt.persisted_revision.clear();
    binding.payload = serde_json::to_value(Record::Binding(value)).unwrap();
    revise(ledger.store(), binding);
    assert!(ledger
        .inspect_page_association(
            receipt.conversation,
            receipt.observed_document,
            receipt.observed_page
        )
        .is_err());
    assert_decision_unresolved(&ledger, &receipt);

    let key = Ledger::binding_id(receipt.observed_document, receipt.observed_page);
    let mut tombstone = ledger.store().heads(Namespace::Conversation, key).unwrap()[0].clone();
    tombstone.kind = Kind::Tombstone;
    tombstone.payload = serde_json::Value::Null;
    revise(ledger.store(), tombstone);
    assert_eq!(
        ledger
            .inspect_page_association(
                receipt.conversation,
                receipt.observed_document,
                receipt.observed_page
            )
            .unwrap(),
        Unresolved(BindingTombstoned)
    );
    assert_decision_unresolved(&ledger, &receipt);
    let mut root = written.current_root[0].clone();
    root.kind = Kind::Tombstone;
    root.payload = serde_json::Value::Null;
    revise(ledger.store(), root);
    assert_eq!(
        ledger
            .inspect_page_association(
                receipt.conversation,
                receipt.observed_document,
                receipt.observed_page
            )
            .unwrap(),
        Unresolved(RootTombstoned)
    );
}

#[test]
fn incomplete_chronology_is_not_a_historical_association_candidate() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let receipt = claim(
        create(&ledger),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let written = bind(&ledger, receipt.clone());
    let mut root = written.current_root[0].clone();
    let Record::Root(mut value) = serde_json::from_value(root.payload.clone()).unwrap() else {
        panic!("root")
    };
    value.next_sequence = 1;
    root.payload = serde_json::to_value(Record::Root(value)).unwrap();
    revise(ledger.store(), root);
    assert!(ledger
        .inspect_page_association(
            receipt.conversation,
            receipt.observed_document,
            receipt.observed_page
        )
        .is_err());
    assert_decision_unresolved(&ledger, &receipt);
}

#[test]
fn root_and_binding_conflict_heads_are_not_hidden_by_query_filtering() {
    use HistoricalPageAssociationInspection::Unresolved;
    use HistoricalPageAssociationUnresolved::*;
    let left_fixture = Fixture::new();
    let right_fixture = Fixture::new();
    let left = Ledger::new(left_fixture.open());
    let right = Ledger::new(right_fixture.open());
    let conversation = create(&left);
    replicate(left.store(), right.store());
    let receipt = claim(conversation, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    bind(&left, receipt.clone());
    let mut other = receipt.clone();
    other.operation = Uuid::new_v4();
    bind(&right, other);
    replicate(right.store(), left.store());
    assert_eq!(
        left.inspect_page_association(
            conversation,
            receipt.observed_document,
            receipt.observed_page
        )
        .unwrap(),
        Unresolved(RootConflicted)
    );
    assert_decision_unresolved(&left, &receipt);

    // Resolve only the fixture root, leaving both real imported binding heads.
    let roots = left
        .store()
        .heads(Namespace::Conversation, conversation)
        .unwrap();
    let mut merged = roots[0].clone();
    merged.parents = roots.iter().map(|head| head.revision_id).collect();
    merged.revision_id = Uuid::new_v4();
    merged.operation_id = Uuid::new_v4();
    left.store().commit(vec![merged], BTreeMap::new()).unwrap();
    assert_eq!(
        left.inspect_page_association(
            conversation,
            receipt.observed_document,
            receipt.observed_page
        )
        .unwrap(),
        Unresolved(BindingConflicted)
    );
    assert_decision_unresolved(&left, &receipt);
}

#[test]
fn unavailable_unrelated_transaction_refuses_instead_of_claiming_missing_data() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let receipt = claim(
        create(&ledger),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    bind(&ledger, receipt.clone());
    let envelope = Envelope {
        envelope_version: FORMAT,
        namespace: Namespace::SubjectMemory,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::json!({"fixture":"unrelated committed record"}),
        media_descriptors: vec![],
    };
    let transaction = store.commit(vec![envelope], BTreeMap::new()).unwrap();
    let manifest = store
        .manifests()
        .unwrap()
        .into_iter()
        .find(|manifest| manifest.transaction_id == transaction)
        .unwrap();
    let generation: Uuid = files::json(&fixture.paths().data.join("CURRENT"), 128).unwrap();
    let object = fixture
        .paths()
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects")
        .join(&manifest.records[0].sha256);
    drop(ledger);
    drop(store);
    // Corrupt only this test-owned object's bytes before normal Store recovery.
    assert!(object.starts_with(&fixture.0));
    fs::write(object, b"corrupt test object").unwrap();
    let ledger = Ledger::new(fixture.open());
    assert_eq!(ledger.store().unavailable_commits().unwrap(), 1);
    let error = ledger
        .inspect_page_association(
            receipt.conversation,
            receipt.observed_document,
            receipt.observed_page,
        )
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<IncompleteStore>()
            .unwrap()
            .unavailable_commits,
        1
    );
    assert_decision_unresolved(&ledger, &receipt);
}

#[test]
fn unlinked_or_differently_bound_roots_do_not_reuse_a_saved_claim() {
    use HistoricalPageAssociationInspection::Unresolved;
    use HistoricalPageAssociationUnresolved::*;
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let receipt = claim(
        create(&ledger),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    bind(&ledger, receipt.clone());
    for (binding, expected_reason) in [
        (None, RootUnbound),
        (Some(Uuid::new_v4()), RootBoundElsewhere),
    ] {
        let mut envelope = ledger
            .store()
            .heads(Namespace::Conversation, receipt.conversation)
            .unwrap()[0]
            .clone();
        let Record::Root(mut root) = serde_json::from_value(envelope.payload.clone()).unwrap()
        else {
            panic!("root")
        };
        root.binding = binding;
        envelope.payload = serde_json::to_value(Record::Root(root)).unwrap();
        revise(ledger.store(), envelope);
        assert_eq!(
            ledger
                .inspect_page_association(
                    receipt.conversation,
                    receipt.observed_document,
                    receipt.observed_page
                )
                .unwrap(),
            Unresolved(expected_reason)
        );
        assert_decision_unresolved(&ledger, &receipt);
    }
}

#[test]
fn stored_successor_unknown_or_history_cannot_become_a_nonbuddy_insertion_plan() {
    // An explicitly supplied model-only ordinary source creates the existing
    // inspection plan; only the successor lookup below consults the real Store.
    struct ModelMissing;
    impl ReadOnlyBindingLookup for ModelMissing {
        fn lookup(
            &self,
            observation: &PageObservation,
            conversation: SdkUuid,
        ) -> AssociationWitness {
            AssociationWitness::modeled(
                observation.clone(),
                observation.page(),
                conversation,
                AssociationState::Missing,
            )
        }
    }
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let receipt = claim(
        create(&ledger),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let mut platform = MockPlatform::new(
        sdk(Uuid::new_v4()),
        sdk(receipt.observed_document),
        receipt.observed_order.iter().copied().map(sdk).collect(),
        sdk(receipt.source.page),
    );
    let original = platform.observe_page().unwrap();
    let ordinary = RecognitionWitness::modeled(
        original.clone(),
        original.page(),
        sdk(receipt.conversation),
        Recognition::OrdinarySource,
    );
    let SyntheticDecision::InspectImmediateSuccessor(plan) = begin_current(
        &original,
        &original,
        sdk(receipt.conversation),
        &ModelMissing,
        Some(&ordinary),
    ) else {
        panic!("explicit synthetic inspection plan")
    };
    let request = NavigationRequest::new(
        OperationId(sdk(Uuid::new_v4())),
        original,
        LogicalDirection::Next,
        plan.neighbor(),
    )
    .unwrap();
    let NavigationOutcome::SyntheticVerified(navigation) = platform.navigate(&request, false)
    else {
        panic!("fixture navigation")
    };
    let observed = platform.observe_page().unwrap();
    let nonbuddy = RecognitionWitness::modeled(
        observed.clone(),
        observed.page(),
        sdk(receipt.conversation),
        Recognition::ConfirmedNonBuddy,
    );
    for bind_history in [false, true] {
        if bind_history {
            bind(&ledger, receipt.clone());
        }
        let before = manifests(ledger.store());
        assert!(matches!(
            complete_successor(
                &plan,
                &navigation,
                &observed,
                &HistoricalLedgerLookup::new(&ledger),
                Some(&nonbuddy),
            ),
            SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
        ));
        assert_eq!(platform.gesture_count(), 1);
        assert_eq!(before, manifests(ledger.store()));
    }
}
