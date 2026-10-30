use remarkable_reader_buddy::{conversation::*, storage::*};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-existing-binding-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("secrets"),
            })
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn original() -> BindingReceipt {
    serde_json::from_str(include_str!("fixtures/binding-v1.json")).unwrap()
}
fn receipt(acquisition: BindingAcquisition) -> BindingReceipt {
    let mut value = original();
    value.acquisition = acquisition;
    match acquisition {
        BindingAcquisition::Inserted => {}
        BindingAcquisition::ExistingCurrent => {
            value.intended_target = Some(value.source.page);
            value.observed_page = value.source.page;
            value.observed_order = value.expected_order.clone();
        }
        BindingAcquisition::ExistingAdjacent => {
            value.expected_order = value.observed_order.clone();
        }
    }
    value
}
fn root(ledger: &Ledger, conversation: Uuid) -> ExpectedHeads {
    ledger
        .expected(Namespace::Conversation, conversation)
        .unwrap()
}
fn create(ledger: &Ledger, conversation: Uuid) {
    ledger.create(conversation, Uuid::new_v4(), 10).unwrap();
}
fn manifests(store: &Store) -> Vec<u8> {
    serde_json::to_vec(&store.manifests().unwrap()).unwrap()
}
fn candidate(ledger: &Ledger, receipt: &BindingReceipt) {
    let HistoricalPageAssociationInspection::HistoricalCandidate(found) = ledger
        .inspect_page_association(
            receipt.conversation,
            receipt.observed_document,
            receipt.observed_page,
        )
        .unwrap()
    else {
        panic!("historical candidate")
    };
    assert_eq!(found.receipt(), receipt);
}

#[test]
fn frozen_v1_insertion_serialization_is_unchanged() {
    let original = include_str!("fixtures/binding-v1.json").trim();
    let receipt: BindingReceipt = serde_json::from_str(original).unwrap();
    assert_eq!(receipt.acquisition, BindingAcquisition::Inserted);
    assert_eq!(serde_json::to_string(&receipt).unwrap(), original);
    assert_eq!(
        serde_json::to_string(&("bind", &receipt)).unwrap(),
        format!("[\"bind\",{original}]")
    );
}

#[test]
fn existing_current_and_adjacent_binding_survive_reopen_restore_and_deleted_retry() {
    for acquisition in [
        BindingAcquisition::Inserted,
        BindingAcquisition::ExistingCurrent,
        BindingAcquisition::ExistingAdjacent,
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let receipt = receipt(acquisition);
        create(&ledger, receipt.conversation);
        let old_heads = root(&ledger, receipt.conversation);
        let first = ledger
            .bind(
                receipt.operation,
                old_heads.clone(),
                ExpectedHeads::default(),
                receipt.clone(),
            )
            .unwrap();
        candidate(&ledger, &receipt);
        let key = Ledger::binding_id(receipt.observed_document, receipt.observed_page);
        let saved = store.heads(Namespace::Conversation, key).unwrap().remove(0);
        assert_eq!(
            saved.domain_schema_version,
            if acquisition == BindingAcquisition::Inserted {
                1
            } else {
                2
            }
        );
        assert_eq!(
            saved.payload["record"]["receipt"]
                .get("acquisition")
                .is_some(),
            acquisition != BindingAcquisition::Inserted
        );
        let backup = fixture.0.join("backup");
        store.export(&backup).unwrap();
        drop(ledger);
        drop(store);
        let store = fixture.open();
        store.restore(&backup).unwrap();
        let ledger = Ledger::new(store.clone());
        candidate(&ledger, &receipt);
        assert!(store.heads(Namespace::Conversation, key).unwrap() == vec![saved]);
        let replay = ledger
            .bind(
                receipt.operation,
                old_heads.clone(),
                ExpectedHeads::default(),
                receipt.clone(),
            )
            .unwrap();
        assert_eq!(replay.historical, first.historical);
        ledger
            .delete(
                receipt.conversation,
                Uuid::new_v4(),
                root(&ledger, receipt.conversation),
                Some(root(&ledger, key)),
            )
            .unwrap();
        let before = manifests(&store);
        let replay = ledger
            .bind(
                receipt.operation,
                old_heads,
                ExpectedHeads::default(),
                receipt.clone(),
            )
            .unwrap();
        assert_eq!(replay.historical, first.historical);
        assert_eq!(replay.current_root[0].kind, Kind::Tombstone);
        assert_eq!(replay.current_binding[0].kind, Kind::Tombstone);
        assert_eq!(manifests(&store), before);
        assert!(matches!(
            ledger
                .inspect_page_association(
                    receipt.conversation,
                    receipt.observed_document,
                    receipt.observed_page
                )
                .unwrap(),
            HistoricalPageAssociationInspection::Unresolved(
                HistoricalPageAssociationUnresolved::RootTombstoned
            )
        ));
    }
}

#[test]
fn invalid_adoption_preserves_the_store_without_insertion_shaped_fallback() {
    for acquisition in [
        BindingAcquisition::ExistingCurrent,
        BindingAcquisition::ExistingAdjacent,
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let good = receipt(acquisition);
        create(&ledger, good.conversation);
        let heads = root(&ledger, good.conversation);
        let before = manifests(&store);
        for mutation in 0..10 {
            let mut bad = good.clone();
            match mutation {
                0 => bad.observed_document = Uuid::new_v4(),
                1 => bad.intended_target = None,
                2 => bad.observed_order.reverse(),
                3 => bad.expected_order.push(bad.source.page),
                4 => {
                    bad.expected_order.remove(0);
                }
                5 => {
                    bad.expected_order.push(Uuid::nil());
                    bad.observed_order = bad.expected_order.clone();
                }
                6 => {
                    bad.observed_page = *bad.expected_order.last().unwrap();
                    bad.intended_target = Some(bad.observed_page);
                }
                7 => bad.persisted_revision.clear(),
                8 => {
                    bad.observed_page = Uuid::new_v4();
                    bad.intended_target = Some(bad.observed_page);
                }
                _ => bad.acquisition = BindingAcquisition::Inserted,
            }
            assert!(
                ledger
                    .bind(bad.operation, heads.clone(), ExpectedHeads::default(), bad)
                    .is_err(),
                "{acquisition:?} mutation {mutation}"
            );
            assert_eq!(manifests(&store), before);
        }
    }
}

#[test]
fn adoption_keeps_claim_cas_and_consumed_operation_fingerprints() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let good = receipt(BindingAcquisition::ExistingAdjacent);
    create(&ledger, good.conversation);
    let heads = root(&ledger, good.conversation);
    let before = manifests(&store);
    assert!(ledger
        .bind(
            good.operation,
            ExpectedHeads::default(),
            ExpectedHeads::default(),
            good.clone()
        )
        .is_err());
    assert!(ledger
        .bind(
            good.operation,
            heads.clone(),
            ExpectedHeads([Uuid::new_v4()].into()),
            good.clone()
        )
        .is_err());
    assert_eq!(manifests(&store), before);
    ledger
        .bind(
            good.operation,
            heads.clone(),
            ExpectedHeads::default(),
            good.clone(),
        )
        .unwrap();
    let key = Ledger::binding_id(good.observed_document, good.observed_page);
    let mut changed = good.clone();
    // A second independently valid acquisition shape cannot reuse the operation.
    changed.acquisition = BindingAcquisition::ExistingCurrent;
    changed.source.page = changed.observed_page;
    let before = manifests(&store);
    assert!(ledger
        .bind(changed.operation, heads, ExpectedHeads::default(), changed)
        .is_err());
    assert_eq!(manifests(&store), before);
    for competing_conversation in [false, true] {
        let mut competing = good.clone();
        competing.operation = Uuid::new_v4();
        if competing_conversation {
            competing.conversation = Uuid::new_v4();
            create(&ledger, competing.conversation);
        } else {
            competing.acquisition = BindingAcquisition::ExistingCurrent;
            competing.observed_page = competing.source.page;
            competing.intended_target = Some(competing.observed_page);
        }
        let target = Ledger::binding_id(competing.observed_document, competing.observed_page);
        let before = manifests(&store);
        assert!(ledger
            .bind(
                competing.operation,
                root(&ledger, competing.conversation),
                root(&ledger, target),
                competing
            )
            .is_err());
        assert_eq!(manifests(&store), before);
    }
    assert_eq!(store.heads(Namespace::Conversation, key).unwrap().len(), 1);
}

#[test]
fn schema_and_discriminator_confusion_cannot_become_historical_binding() {
    for mutation in 0..8 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let good = receipt(BindingAcquisition::ExistingAdjacent);
        create(&ledger, good.conversation);
        ledger
            .bind(
                good.operation,
                root(&ledger, good.conversation),
                ExpectedHeads::default(),
                good.clone(),
            )
            .unwrap();
        let key = Ledger::binding_id(good.observed_document, good.observed_page);
        let mut saved = store.heads(Namespace::Conversation, key).unwrap().remove(0);
        saved.parents = [saved.revision_id].into();
        saved.revision_id = Uuid::new_v4();
        saved.operation_id = Uuid::new_v4();
        let body = &mut saved.payload["record"]["receipt"];
        match mutation {
            0 => saved.domain_schema_version = 1,
            1 => {
                body.as_object_mut().unwrap().remove("acquisition");
            }
            2 => body["acquisition"] = serde_json::json!("Inserted"),
            3 => body["acquisition"] = serde_json::json!("Future"),
            4 => body["acquisition"] = serde_json::Value::Null,
            5 => body["observed_order"].as_array_mut().unwrap().reverse(),
            6 => body["acquisition"] = serde_json::json!("ExistingCurrent"),
            _ => body["unexpected"] = true.into(),
        }
        store.commit(vec![saved], BTreeMap::new()).unwrap();
        let before = manifests(&store);
        assert!(
            ledger
                .inspect_page_association(
                    good.conversation,
                    good.observed_document,
                    good.observed_page
                )
                .is_err(),
            "mutation {mutation}"
        );
        assert_eq!(manifests(&store), before);
    }
}
