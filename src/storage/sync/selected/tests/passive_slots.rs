//! Actual Engine/two-Store routing; local HTTP faults are not provider qualification.
use super::*;

struct PassiveFixture {
    wire: Wire,
    publisher: Arc<Store>,
    receiver: Arc<Store>,
    identity: catalog::Identity,
    scope: SelectionScope,
    next_slot: String,
    _paths: Fixture,
}

impl PassiveFixture {
    fn new() -> Self {
        let paths = Fixture::new();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let publisher = paths.store("publisher");
        let receiver = paths.store("receiver");
        let binding = wire
            .client(publisher.paths.credentials.join("drive.json"), group)
            .binding_digest();
        let publisher = compound(publisher, group, &binding);
        let receiver = compound(receiver, group, &binding);
        choose(&publisher, group, &binding);
        choose(&receiver, group, &binding);
        let profile = Uuid::new_v4();
        let identity = catalog::Identity {
            group,
            profile,
            binding_sha256: binding.clone(),
        };
        let initial = fact(profile);
        let owner = catalog::Owner::GlobalFact {
            key: initial.key.clone(),
        };
        let scope = identity.aggregate_scope(&owner).unwrap();
        // Model an existing group only. All accepted fact/receipt authority below
        // comes from the actual producer and Engine, never a seeded commit.
        let root = existing_group(&wire, group, binding, profile, &scope, owner);
        let producer =
            selected_memory::SelectedMemory::new(publisher.clone(), identity.clone()).unwrap();
        let registration = producer
            .initialize_aggregate(selected_memory::InitializeAggregate {
                operation: Uuid::new_v4(),
                accepted_base_sha256: genesis_base(&root, &scope.key_sha256),
                fact: initial,
            })
            .unwrap()
            .registration
            .unwrap();
        producer
            .initialize_selection(selected_memory::InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: root,
                registrations: vec![registration],
            })
            .unwrap();
        let original = publisher
            .selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .transaction
            .operation;
        let mut engine = fixture_engine(&wire, publisher.clone(), group, 32);
        finish_application(&mut engine, &scope.key_sha256);
        let attempts = publisher.selected_attempt_inventory().unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].selection.operation, original);
        assert_eq!(attempts[0].state, SelectedAttemptState::Withdrawn);
        let recorded = engine.checkpoint.scopes[&scope.key_sha256]
            .chain
            .last()
            .unwrap();
        let commit: Commit = serde_json::from_slice(&recorded.bytes).unwrap();
        let next_slot = commit.next_slot;
        drop(engine);
        drop(producer);
        Self {
            wire,
            publisher,
            receiver,
            identity,
            scope,
            next_slot,
            _paths: paths,
        }
    }

    fn engine(&self) -> Engine {
        fixture_engine(&self.wire, self.receiver.clone(), self.identity.group, 32)
    }

    fn assert_passive(&self, engine: &Engine, start: usize, observed: &str) {
        let requests = self.wire.requests();
        let phase = &requests[start..];
        let expected_path = format!("/drive/v3/files/{observed}");
        assert!(phase.iter().any(|request| {
            request
                .split_whitespace()
                .nth(1)
                .and_then(|target| target.split('?').next())
                == Some(expected_path.as_str())
        }));
        assert!(phase
            .iter()
            .all(|request| request.starts_with("GET ") && !request.contains("generateIds")));
        assert!(self
            .receiver
            .selected_attempt_inventory()
            .unwrap()
            .is_empty());
        assert!(engine.checkpoint.catalog_pending.is_none());
        assert!(engine.checkpoint.retired_catalog.is_empty());
        assert!(engine.checkpoint.uploads.is_empty());
        for progress in engine.checkpoint.scopes.values() {
            assert!(progress.pending.is_none());
            assert!(progress.retired_pending.is_empty());
        }
    }

    fn assert_unselected(&self) {
        for scope in [&self.scope, &self.identity.scope().unwrap()] {
            assert!(self
                .receiver
                .selected_snapshot(scope, MAX_ITEMS)
                .unwrap()
                .is_none());
        }
    }
}

fn finish_application(engine: &mut Engine, key: &str) {
    // A bounded sequence of synchronous production Engine steps. No Worker
    // scheduler is launched, so a dropped thread cannot cross phase evidence.
    for _ in 0..32 {
        let status = engine.step().unwrap();
        if engine.checkpoint.scopes.get(key).is_some_and(|progress| {
            progress.applied.is_some() && progress.receive.is_none() && progress.pending.is_none()
        }) {
            assert_eq!(status, Status::Current);
            return;
        }
        assert!(matches!(status, Status::Pending | Status::Current));
    }
    panic!("original selected publication/application did not finish within 32 steps");
}

#[derive(Clone, Copy, Debug)]
enum Damage {
    Metadata,
    MissingDescriptor,
}

fn occupy_invalid(wire: &Wire, binding: SlotBinding, damage: Damage) {
    match damage {
        Damage::Metadata => wire.object(
            &binding.slot_id,
            binding.group,
            b"occupied ordinary object, not a commit-slot pointer".to_vec(),
        ),
        Damage::MissingDescriptor => {
            // The occupied pointer is correctly bound, but its promised body is
            // unavailable. That is invalid occupied data, not an absent slot.
            wire.slot(&binding, "missing-passive-body", b"promised body".to_vec());
            wire.remove_fixture_object("missing-passive-body");
        }
    }
}

#[test]
fn passive_absent_initial_root_joins_but_invalid_occupied_root_refuses() {
    let fixture = PassiveFixture::new();
    fixture.wire.remove_fixture_object("shared-root");
    let manifests = serde_json::to_vec(&fixture.receiver.manifests().unwrap()).unwrap();
    let start = fixture.wire.requests().len();
    let mut engine = fixture.engine();
    for _ in 0..2 {
        assert_eq!(engine.step().unwrap(), Status::Joining);
        assert!(engine.checkpoint.root.is_none());
        assert!(engine.checkpoint.catalog.is_empty());
        assert!(engine.checkpoint.scopes.is_empty());
    }
    fixture.assert_passive(&engine, start, "shared-root");
    for damage in [Damage::Metadata, Damage::MissingDescriptor] {
        occupy_invalid(
            &fixture.wire,
            engine.slot(root_key(fixture.identity.group), "shared-root".into()),
            damage,
        );
        drop(engine);
        engine = fixture.engine();
        let start = fixture.wire.requests().len();
        for _ in 0..2 {
            assert_eq!(engine.step().unwrap(), Status::UnsupportedOrCorrupt);
            assert!(engine.checkpoint.root.is_none());
            assert!(engine.checkpoint.catalog.is_empty());
            assert!(engine.checkpoint.scopes.is_empty());
        }
        fixture.assert_passive(&engine, start, "shared-root");
        fixture.assert_unselected();
        assert_eq!(
            serde_json::to_vec(&fixture.receiver.manifests().unwrap()).unwrap(),
            manifests
        );
    }
}

#[test]
fn passive_missing_initial_catalog_never_turns_invalid_occupancy_into_absence() {
    let fixture = PassiveFixture::new();
    fixture.wire.remove_fixture_object("catalog-first");
    let manifests = serde_json::to_vec(&fixture.receiver.manifests().unwrap()).unwrap();
    let mut engine = fixture.engine();
    let start = fixture.wire.requests().len();
    assert_eq!(engine.step().unwrap(), Status::Joining);
    fixture.assert_passive(&engine, start, "catalog-first");
    let root = serde_json::to_vec(&engine.checkpoint.root).unwrap();
    assert!(engine.checkpoint.root.is_some());
    for damage in [Damage::Metadata, Damage::MissingDescriptor] {
        occupy_invalid(
            &fixture.wire,
            engine.slot(catalog_key(fixture.identity.group), "catalog-first".into()),
            damage,
        );
        drop(engine);
        engine = fixture.engine();
        let start = fixture.wire.requests().len();
        for _ in 0..2 {
            assert_eq!(engine.step().unwrap(), Status::UnsupportedOrCorrupt);
            assert_eq!(serde_json::to_vec(&engine.checkpoint.root).unwrap(), root);
            assert!(engine.checkpoint.catalog.is_empty());
            assert!(engine.checkpoint.scopes.is_empty());
        }
        fixture.assert_passive(&engine, start, "catalog-first");
        fixture.assert_unselected();
        assert_eq!(
            serde_json::to_vec(&fixture.receiver.manifests().unwrap()).unwrap(),
            manifests
        );
    }
}

#[test]
fn passive_invalid_root_catalog_or_successor_preserves_the_applied_original() {
    for target in ["shared-root", "catalog-first", "scope-successor"] {
        for damage in [Damage::Metadata, Damage::MissingDescriptor] {
            let fixture = PassiveFixture::new();
            let mut engine = fixture.engine();
            let start = fixture.wire.requests().len();
            finish_application(&mut engine, &fixture.scope.key_sha256);
            fixture.assert_passive(&engine, start, "fact-first");
            let received = fixture
                .receiver
                .selected_snapshot(&fixture.scope, MAX_ITEMS)
                .unwrap()
                .unwrap();
            let sent = fixture
                .publisher
                .selected_snapshot(&fixture.scope, MAX_ITEMS)
                .unwrap()
                .unwrap();
            assert_eq!(
                received.transaction.selected.records,
                sent.transaction.selected.records
            );
            assert_ne!(received.transaction.operation, sent.transaction.operation);
            for reference in &sent.transaction.selected.records {
                assert_eq!(
                    fixture.receiver.read_object(reference).unwrap(),
                    fixture.publisher.read_object(reference).unwrap()
                );
            }
            // An absent agreed successor is a healthy no-change observation;
            // pin the real accepted original before corrupting any occupied slot.
            let start = fixture.wire.requests().len();
            assert_eq!(engine.step().unwrap(), Status::Current);
            fixture.assert_passive(&engine, start, &fixture.next_slot);
            let checkpoint = serde_json::to_vec(&engine.checkpoint).unwrap();
            let receipt = serde_json::to_vec(&received.transaction).unwrap();
            let manifests = serde_json::to_vec(&fixture.receiver.manifests().unwrap()).unwrap();
            let catalog_scope = fixture.identity.scope().unwrap();
            let local_catalog = fixture
                .receiver
                .selected_snapshot(&catalog_scope, MAX_ITEMS)
                .unwrap()
                .unwrap();
            let catalog_receipt = serde_json::to_vec(&local_catalog.transaction).unwrap();
            let (key, slot) = match target {
                "shared-root" => (root_key(fixture.identity.group), target),
                "catalog-first" => (catalog_key(fixture.identity.group), target),
                _ => (fixture.scope.key_sha256.clone(), fixture.next_slot.as_str()),
            };
            occupy_invalid(&fixture.wire, engine.slot(key, slot.into()), damage);
            for _ in 0..2 {
                // Reconstruct from the actual durable checkpoint after every
                // refusal; no synthetic accepted head/receipt is installed.
                drop(engine);
                engine = fixture.engine();
                let start = fixture.wire.requests().len();
                let status = engine.step().unwrap();
                assert_eq!(status, Status::UnsupportedOrCorrupt, "{target} {damage:?}");
                assert_eq!(
                    engine.health(status)["status"],
                    serde_json::to_value(status).unwrap()
                );
                fixture.assert_passive(&engine, start, slot);
                assert_eq!(serde_json::to_vec(&engine.checkpoint).unwrap(), checkpoint);
                let current = fixture
                    .receiver
                    .selected_snapshot(&fixture.scope, MAX_ITEMS)
                    .unwrap()
                    .unwrap();
                assert_eq!(current.token, received.token);
                assert_eq!(serde_json::to_vec(&current.transaction).unwrap(), receipt);
                assert_eq!(
                    serde_json::to_vec(
                        &fixture
                            .receiver
                            .selected_receipt(&fixture.scope, received.transaction.operation)
                            .unwrap()
                            .unwrap()
                    )
                    .unwrap(),
                    receipt
                );
                let catalog = fixture
                    .receiver
                    .selected_snapshot(&catalog_scope, MAX_ITEMS)
                    .unwrap()
                    .unwrap();
                assert_eq!(catalog.token, local_catalog.token);
                assert_eq!(
                    serde_json::to_vec(&catalog.transaction).unwrap(),
                    catalog_receipt
                );
                assert_eq!(
                    serde_json::to_vec(&fixture.receiver.manifests().unwrap()).unwrap(),
                    manifests
                );
                for reference in &received.transaction.selected.records {
                    assert_eq!(
                        fixture.receiver.read_object(reference).unwrap(),
                        fixture.publisher.read_object(reference).unwrap()
                    );
                }
            }
        }
    }
}
