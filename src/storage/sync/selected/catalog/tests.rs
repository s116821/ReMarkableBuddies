use super::*;
use crate::memory::{
    self,
    selected::{InitializeAggregate, InitializeSelection, SelectedMemory},
};
use crate::storage::{compound_generation, drive::shared_fixture::Wire};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-catalog-http-{}", Uuid::new_v4())))
    }
    fn paths(&self, name: &str) -> StorePaths {
        StorePaths {
            data: self.0.join(name).join("data"),
            cache: self.0.join(name).join("cache"),
            credentials: self.0.join(name).join("credentials"),
        }
    }
    fn store(&self, name: &str, group: Uuid, binding: &str) -> Arc<Store> {
        let paths = self.paths(name);
        let store = Store::open(paths.clone()).unwrap();
        let prep = compound_generation::Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: current_generation(&store).unwrap(),
            group,
            binding_sha256: binding.into(),
            expected_mode: serde_json::from_value(
                serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap(),
            )
            .unwrap(),
        };
        drop(store);
        let store = Arc::new(Store::prepare_shared_notebook(paths, prep).unwrap());
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                mode::SyncModeChoice::SharedGroup {
                    group,
                    binding_sha256: binding.into(),
                    root_id: "catalog-root".into(),
                },
            )
            .unwrap();
        store
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn group(wire: &Wire, group: Uuid, binding: &str, profile: Uuid) -> String {
    let root = Root {
        schema: WIRE,
        group,
        protocol: "buddy-shared-fixed-slot-v1".into(),
        binding_sha256: binding.into(),
        profile,
        catalog_slot: "initial-catalog".into(),
    };
    let bytes = serde_json::to_vec(&root).unwrap();
    let sha = digest(&bytes);
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: root_key(group),
            slot_id: "catalog-root".into(),
        },
        "root-bytes",
        bytes,
    );
    let catalog = Catalog {
        schema: WIRE,
        group,
        root_sha256: sha.clone(),
        base_sha256: sha.clone(),
        slot: "initial-catalog".into(),
        operation: Uuid::new_v4(),
        entries: BTreeMap::new(),
        next_slot: "catalog-append".into(),
    };
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: catalog_key(group),
            slot_id: "initial-catalog".into(),
        },
        "initial-catalog-bytes",
        serde_json::to_vec(&catalog).unwrap(),
    );
    sha
}
fn fact(profile: Uuid) -> memory::Fact {
    memory::Fact {
        key: memory::Key {
            profile,
            scope: memory::Scope::Global,
            fact: Uuid::new_v4(),
        },
        claim: "catalog registration example".into(),
        value: "original local value".into(),
        subject: "test".into(),
        tags: vec![],
        provenance: memory::Provenance {
            basis: memory::Basis::UserConfirmed,
            description: "local catalog HTTP fixture".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 1,
        expires_ms: None,
    }
}
fn seed(
    store: Arc<Store>,
    group: Uuid,
    binding: &str,
    root: &str,
    fact: memory::Fact,
) -> Registration {
    let identity = Identity {
        group,
        profile: fact.key.profile,
        binding_sha256: binding.into(),
    };
    let owner = match fact.key.scope {
        memory::Scope::Global => memory::catalog::Owner::GlobalFact {
            key: fact.key.clone(),
        },
        memory::Scope::Document { document } => memory::catalog::Owner::Document { document },
    };
    let provider = SelectedMemory::new(store, identity).unwrap();
    let initialized = provider
        .initialize_aggregate(InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: genesis_base(root, &owner.key_sha256().unwrap()),
            fact,
        })
        .unwrap();
    let registration = initialized.registration.unwrap();
    provider
        .initialize_selection(InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: root.into(),
            registrations: vec![registration.clone()],
        })
        .unwrap();
    registration
}
fn engine(wire: &Wire, store: Arc<Store>, group: Uuid, batch: usize) -> Engine {
    let transport = wire.client(store.paths.credentials.join("drive.json"), group);
    let binding = Binding {
        group,
        root_id: "catalog-root".into(),
        digest: transport.binding_digest(),
    };
    Engine::new(
        store,
        SyncPolicy {
            batch_items: batch,
            ..Default::default()
        },
        transport,
        binding,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn drive(engine: &mut Engine, mut condition: impl FnMut(&Engine) -> bool) {
    for _ in 0..100 {
        if condition(engine) {
            return;
        }
        engine.step().unwrap();
    }
    panic!("catalog postcondition not reached");
}
fn applied(engine: &Engine, key: &str) -> bool {
    engine
        .checkpoint
        .scopes
        .get(key)
        .is_some_and(|scope| scope.applied.is_some())
}

#[test]
fn real_local_registration_appends_catalog_and_passive_store_receives_exact_fact() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding, profile);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let registration = seed(a.clone(), group_id, &binding, &root, fact(profile));
    let key = registration.key_sha256;
    let scope = SelectionScope {
        group: group_id,
        key_sha256: key.clone(),
        binding_sha256: binding.clone(),
    };
    let original = a
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap()
        .transaction
        .selected;
    let mut producer = engine(&wire, a.clone(), group_id, 1);
    // Stop after the nonpublishing ID is durable; the real process Store can
    // close/reopen without changing that original control or provider identity.
    drive(&mut producer, |engine| {
        engine
            .checkpoint
            .catalog_pending
            .as_ref()
            .is_some_and(|p| p.genesis_slot.is_some())
    });
    let id = producer
        .checkpoint
        .catalog_pending
        .as_ref()
        .unwrap()
        .genesis_slot
        .clone();
    drop(producer);
    drop(a);
    let a = Arc::new(Store::open_existing_for_mode(fixture.paths("a")).unwrap());
    let mut producer = engine(&wire, a, group_id, 1);
    assert_eq!(
        producer
            .checkpoint
            .catalog_pending
            .as_ref()
            .unwrap()
            .genesis_slot,
        id
    );
    drive(&mut producer, |engine| applied(engine, &key));
    let mut receiver = engine(&wire, b.clone(), group_id, 1);
    drive(&mut receiver, |engine| applied(engine, &key));
    let selected = b.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    assert!(shared::same_membership(
        &original,
        &selected.transaction.selected
    ));
    assert_eq!(producer.checkpoint.catalog.len(), 2);
    assert!(producer.checkpoint.catalog_pending.is_none());
    assert_eq!(producer.checkpoint.retired_catalog.len(), 1);
}

#[test]
fn different_key_catalog_race_extends_only_the_accepted_mapping_set() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding, profile);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let ka = seed(a.clone(), group_id, &binding, &root, fact(profile)).key_sha256;
    let kb = seed(b.clone(), group_id, &binding, &root, fact(profile)).key_sha256;
    let mut ea = engine(&wire, a, group_id, 1);
    let mut eb = engine(&wire, b, group_id, 1);
    for current in [&mut ea, &mut eb] {
        drive(current, |engine| {
            engine
                .checkpoint
                .catalog_pending
                .as_ref()
                .and_then(|p| p.proposal.as_ref())
                .is_some_and(|p| p.descriptor.is_some())
        });
    }
    let before_a = ea
        .checkpoint
        .catalog_pending
        .as_ref()
        .unwrap()
        .proposal
        .as_ref()
        .unwrap()
        .bytes
        .clone();
    let before_b = eb
        .checkpoint
        .catalog_pending
        .as_ref()
        .unwrap()
        .proposal
        .as_ref()
        .unwrap()
        .bytes
        .clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let joins = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = barrier.clone();
    let arrived = joins.clone();
    wire.observe_requests(move |request| {
        if request.starts_with("POST /drive/v3/files?")
            && arrived.fetch_add(1, Ordering::SeqCst) < 2
        {
            captured.wait();
        }
    });
    let (ra, rb) = thread::scope(|scope| {
        let a = scope.spawn(|| ea.step());
        let b = scope.spawn(|| eb.step());
        (a.join().unwrap(), b.join().unwrap())
    });
    ra.unwrap();
    rb.unwrap();
    assert_eq!(joins.load(Ordering::SeqCst), 2);
    for _ in 0..100 {
        ea.step().unwrap();
        eb.step().unwrap();
        if [&ka, &kb]
            .iter()
            .all(|key| applied(&ea, key) && applied(&eb, key))
        {
            break;
        }
    }
    for current in [&ea, &eb] {
        assert!(applied(current, &ka) && applied(current, &kb));
        let view = current.view().unwrap().unwrap();
        assert_eq!(view.entries.len(), 2);
        assert_eq!(current.checkpoint.catalog.len(), 3);
    }
    // The two original candidates remain byte-exact evidence at their original
    // slot even though only the losing catalog-set extension needs a successor.
    assert!(ea
        .checkpoint
        .retired_catalog
        .iter()
        .any(|p| p.bytes == before_a));
    assert!(eb
        .checkpoint
        .retired_catalog
        .iter()
        .any(|p| p.bytes == before_b));
}

#[test]
fn document_fact_without_conversation_receives_under_its_document_owner() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding, profile);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let mut value = fact(profile);
    value.key.scope = memory::Scope::Document {
        document: Uuid::new_v4(),
    };
    let registration = seed(a.clone(), group_id, &binding, &root, value);
    let scope = SelectionScope {
        group: group_id,
        key_sha256: registration.key_sha256.clone(),
        binding_sha256: binding,
    };
    let original = a
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap()
        .transaction
        .selected;
    let mut producer = engine(&wire, a, group_id, 16);
    drive(&mut producer, |engine| applied(engine, &scope.key_sha256));
    let mut receiver = engine(&wire, b.clone(), group_id, 16);
    drive(&mut receiver, |engine| applied(engine, &scope.key_sha256));
    assert!(shared::same_membership(
        &original,
        &b.selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .transaction
            .selected
    ));
}

#[test]
fn same_key_first_winner_keeps_the_other_explicit_local_fact_in_retained_custody() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding, profile);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let first = fact(profile);
    let mut second = first.clone();
    second.value = "different explicit local value".into();
    let registration = seed(a.clone(), group_id, &binding, &root, first);
    seed(b.clone(), group_id, &binding, &root, second);
    let scope = SelectionScope {
        group: group_id,
        key_sha256: registration.key_sha256.clone(),
        binding_sha256: binding,
    };
    let before_a = a
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap()
        .transaction
        .selected;
    let before_b = b
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap()
        .transaction
        .selected;
    assert!(!shared::same_membership(&before_a, &before_b));
    let mut ea = engine(&wire, a.clone(), group_id, 1);
    let mut eb = engine(&wire, b.clone(), group_id, 1);
    for _ in 0..100 {
        ea.step().unwrap();
        eb.step().unwrap();
        if applied(&ea, &scope.key_sha256) && applied(&eb, &scope.key_sha256) {
            break;
        }
    }
    assert!(applied(&ea, &scope.key_sha256) && applied(&eb, &scope.key_sha256));
    let after_a = a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    let after_b = b.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    assert!(shared::same_membership(
        &after_a.transaction.selected,
        &after_b.transaction.selected
    ));
    for (before, after) in [(&before_a, &after_a), (&before_b, &after_b)] {
        if !shared::same_membership(before, &after.transaction.selected) {
            assert!(
                after
                    .transaction
                    .retained
                    .iter()
                    .any(|retained| shared::same_membership(before, retained)),
                "losing explicit local content must remain retained for inspection"
            );
        }
    }
    assert_eq!(ea.checkpoint.catalog.len(), 2);
    assert_eq!(eb.checkpoint.catalog.len(), 2);
    drop(ea);
    drop(eb);
    drop(a);
    drop(b);
    for (name, before) in [("a", &before_a), ("b", &before_b)] {
        let store = Store::open_existing_for_mode(fixture.paths(name)).unwrap();
        let reopened = store.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        assert!(shared::same_membership(
            &reopened.transaction.selected,
            &after_a.transaction.selected
        ));
        if !shared::same_membership(before, &reopened.transaction.selected) {
            assert!(reopened
                .transaction
                .retained
                .iter()
                .any(|retained| shared::same_membership(before, retained)));
        }
    }
}

#[test]
fn legacy_unknown_staging_never_treats_an_absent_outbox_as_never_submitted() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding, profile);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let first = fact(profile);
    let mut second = first.clone();
    second.value = "other original".into();
    let registration = seed(a.clone(), group_id, &binding, &root, first);
    seed(b.clone(), group_id, &binding, &root, second);
    let key = registration.key_sha256;
    let mut ea = engine(&wire, a, group_id, 1);
    let mut eb = engine(&wire, b.clone(), group_id, 1);
    drive(&mut ea, |engine| {
        engine.checkpoint.catalog_pending.is_none()
            && engine
                .view()
                .unwrap()
                .is_some_and(|view| view.entries.contains_key(&key))
    });
    drive(&mut eb, |engine| {
        engine
            .checkpoint
            .scopes
            .get(&key)
            .is_some_and(|scope| scope.pending.is_some())
    });
    let pending = eb
        .checkpoint
        .scopes
        .get_mut(&key)
        .unwrap()
        .pending
        .as_mut()
        .unwrap();
    let original = pending.operation;
    assert_eq!(pending.outbox_prepared, Some(false));
    assert!(b.inspect_selected_attempt(original).unwrap().is_none());
    // A legacy checkpoint has no preparation marker. Its absence supplies no
    // authority to classify the historical submission outcome.
    pending.outbox_prepared = None;
    eb.save().unwrap();
    drive(&mut ea, |engine| applied(engine, &key));
    let mut refused = false;
    for _ in 0..100 {
        if let Err(error) = eb.step() {
            assert_eq!(
                error.to_string(),
                "receiving original outbox missing or preparation unknown"
            );
            refused = true;
            break;
        }
    }
    assert!(refused);
    let progress = &eb.checkpoint.scopes[&key];
    assert!(progress.pending.is_some() && progress.receive.as_ref().unwrap().applied);
    assert!(progress.retired_pending.is_empty());
    drop(eb);
    let mut reopened = engine(&wire, b, group_id, 1);
    assert!(reopened.step().is_err());
    assert!(reopened.checkpoint.scopes[&key].pending.is_some());
}

#[test]
fn outbox_preparation_crash_reopens_original_custody_and_refuses_a_missing_journal() {
    for remove_journal in [true, false] {
        let fixture = Fixture::new();
        let wire = Wire::new();
        let group_id = Uuid::new_v4();
        let profile = Uuid::new_v4();
        let binding = wire
            .client(fixture.0.join("provider/drive.json"), group_id)
            .binding_digest();
        let root = group(&wire, group_id, &binding, profile);
        let a = fixture.store("a", group_id, &binding);
        let b = fixture.store("b", group_id, &binding);
        let first = fact(profile);
        let mut second = first.clone();
        second.value = "other explicit original".into();
        let registration = seed(a.clone(), group_id, &binding, &root, first);
        seed(b.clone(), group_id, &binding, &root, second);
        let key = registration.key_sha256;
        let mut ea = engine(&wire, a, group_id, 1);
        let mut eb = engine(&wire, b.clone(), group_id, 1);
        drive(&mut ea, |engine| {
            engine.checkpoint.catalog_pending.is_none()
                && engine
                    .view()
                    .unwrap()
                    .is_some_and(|view| view.entries.contains_key(&key))
        });
        drive(&mut eb, |engine| {
            engine
                .checkpoint
                .scopes
                .get(&key)
                .is_some_and(|scope| scope.pending.is_some())
        });
        let original = eb.checkpoint.scopes[&key]
            .pending
            .as_ref()
            .unwrap()
            .operation;
        assert!(b.inspect_selected_attempt(original).unwrap().is_none());
        b.set_fault(Fault::AfterCommit).unwrap();
        let mut interrupted = false;
        for _ in 0..100 {
            if eb.step().is_err() {
                interrupted = true;
                break;
            }
        }
        assert!(interrupted);
        let attempt = b
            .inspect_selected_attempt(original)
            .unwrap()
            .expect("actual preparation must be durable before interruption");
        assert_eq!(attempt.request.operation, original);
        let prepared_request = attempt.request.clone();
        drop(eb);
        drop(b);
        if remove_journal {
            // Deliberately corrupt only this disposable test Store after the
            // actual durable-write / before-checkpoint crash boundary.
            fs::remove_file(
                fixture
                    .paths("b")
                    .data
                    .join("sync/selected-outbox")
                    .join(format!("{original}.json")),
            )
            .unwrap();
        }
        drive(&mut ea, |engine| applied(engine, &key));
        let b = Arc::new(Store::open_existing_for_mode(fixture.paths("b")).unwrap());
        let mut eb = engine(&wire, b.clone(), group_id, 1);
        let marker = eb.checkpoint.scopes[&key]
            .pending
            .as_ref()
            .unwrap()
            .outbox_prepared;
        let posts = wire
            .requests()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count();
        let slot_posts = wire.slot_posts().len();
        if remove_journal {
            let mut refused = false;
            for _ in 0..100 {
                if let Err(error) = eb.step() {
                    assert_eq!(
                        error.to_string(),
                        "receiving original outbox missing or preparation unknown"
                    );
                    refused = true;
                    break;
                }
                if eb.checkpoint.scopes[&key].pending.is_none() {
                    break;
                }
            }
            assert!(
                refused,
                "missing actually prepared journal must not become never-submitted staging"
            );
            assert!(eb.checkpoint.scopes[&key].pending.is_some());
            assert!(b.inspect_selected_attempt(original).unwrap().is_none());
        } else {
            drive(&mut eb, |engine| {
                applied(engine, &key) && engine.checkpoint.scopes[key.as_str()].receive.is_none()
            });
            let reconciled = b.inspect_selected_attempt(original).unwrap().unwrap();
            assert_eq!(reconciled.request, prepared_request);
            assert_eq!(reconciled.state, SelectedAttemptState::Withdrawn);
            assert!(eb.checkpoint.scopes[&key]
                .retired_pending
                .iter()
                .any(|pending| pending.operation == original));
        }
        assert_eq!(
            marker, None,
            "preparation intent must be conservative across the actual write boundary"
        );
        assert_eq!(
            wire.requests()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            posts + usize::from(!remove_journal)
        );
        let retries = wire.slot_posts();
        if remove_journal {
            assert_eq!(retries.len(), slot_posts);
        } else {
            assert_eq!(&retries[slot_posts..], [prepared_request.slot_id]);
        }
    }
}

#[test]
fn staged_catalog_cannot_drop_an_accepted_mapping_before_upload() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding, profile);
    let owner: Owner = memory::catalog::Owner::GlobalFact {
        key: fact(profile).key,
    }
    .into();
    let unrelated = owner.key_sha256().unwrap();
    let initial = Catalog {
        schema: WIRE,
        group: group_id,
        root_sha256: root.clone(),
        base_sha256: root.clone(),
        slot: "initial-catalog".into(),
        operation: Uuid::new_v4(),
        entries: BTreeMap::from([(
            unrelated.clone(),
            Entry {
                owner,
                genesis_slot: "unrelated-genesis".into(),
            },
        )]),
        next_slot: "catalog-append".into(),
    };
    wire.slot(
        &SlotBinding {
            group: group_id,
            scope_sha256: catalog_key(group_id),
            slot_id: "initial-catalog".into(),
        },
        "initial-catalog-bytes",
        serde_json::to_vec(&initial).unwrap(),
    );
    let a = fixture.store("a", group_id, &binding);
    seed(a.clone(), group_id, &binding, &root, fact(profile));
    let mut ea = engine(&wire, a, group_id, 1);
    drive(&mut ea, |engine| {
        engine
            .checkpoint
            .catalog_pending
            .as_ref()
            .is_some_and(|p| p.proposal.is_some())
    });
    let path = fixture.paths("a").data.join("sync/state.json");
    let before = fs::read(&path).unwrap();
    let proposal = ea
        .checkpoint
        .catalog_pending
        .as_mut()
        .unwrap()
        .proposal
        .as_mut()
        .unwrap();
    assert!(proposal.descriptor.is_none());
    let mut corrupt: Catalog = serde_json::from_slice(&proposal.bytes).unwrap();
    assert!(corrupt.entries.remove(&unrelated).is_some());
    proposal.bytes = serde_json::to_vec(&corrupt).unwrap();
    let requests = wire.requests().len();
    assert!(ea.save().is_err());
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(wire.requests().len(), requests);
}

#[test]
fn unrelated_root_genesis_refuses_before_any_catalog_upload() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    group(&wire, group_id, &binding, profile);
    let a = fixture.store("a", group_id, &binding);
    seed(
        a.clone(),
        group_id,
        &binding,
        &digest(b"different root"),
        fact(profile),
    );
    let mut producer = engine(&wire, a, group_id, 64);
    assert!(producer.step().is_err());
    assert!(!wire.requests().iter().any(|r| r.starts_with("POST ")));
    assert!(producer.checkpoint.catalog_pending.is_none());
}
