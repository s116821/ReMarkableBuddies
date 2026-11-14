use super::*;
use crate::{
    config::portable::{
        record::{Condition, SelectionCondition},
        selected::Artifact,
    },
    storage::{compound_generation, drive::shared_fixture::Wire},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-preferences-http-{}", Uuid::new_v4())))
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
        let request = compound_generation::Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: catalog::current_generation(&store).unwrap(),
            group,
            binding_sha256: binding.into(),
            expected_mode: serde_json::from_value(
                serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap(),
            )
            .unwrap(),
        };
        drop(store);
        let store = Arc::new(Store::prepare_shared_notebook(paths, request).unwrap());
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                mode::SyncModeChoice::SharedGroup {
                    group,
                    binding_sha256: binding.into(),
                    root_id: "portable-root".into(),
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
fn group(wire: &Wire, group: Uuid, binding: &str) -> String {
    let root = Root {
        schema: WIRE,
        group,
        protocol: "buddy-shared-fixed-slot-v1".into(),
        binding_sha256: binding.into(),
        profile: Uuid::new_v4(),
        catalog_slot: "portable-catalog".into(),
    };
    let bytes = serde_json::to_vec(&root).unwrap();
    let sha = digest(&bytes);
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: root_key(group),
            slot_id: "portable-root".into(),
        },
        "portable-root-body",
        bytes,
    );
    let catalog = Catalog {
        schema: WIRE,
        group,
        root_sha256: sha.clone(),
        base_sha256: sha.clone(),
        slot: "portable-catalog".into(),
        operation: Uuid::new_v4(),
        entries: BTreeMap::new(),
        next_slot: "portable-catalog-append".into(),
    };
    wire.slot(
        &SlotBinding {
            group,
            scope_sha256: catalog_key(group),
            slot_id: "portable-catalog".into(),
        },
        "portable-catalog-body",
        serde_json::to_vec(&catalog).unwrap(),
    );
    sha
}
fn engine(wire: &Wire, store: Arc<Store>, group: Uuid) -> Engine {
    let transport = wire.client(store.paths.credentials.join("drive.json"), group);
    let binding = Binding {
        group,
        root_id: "portable-root".into(),
        digest: transport.binding_digest(),
    };
    Engine::new(
        store,
        SyncPolicy {
            batch_items: 1,
            ..Default::default()
        },
        transport,
        binding,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn edit(
    store: &Arc<Store>,
    group: Uuid,
    binding: &str,
    root: &str,
    settings: serde_json::Value,
) -> Manifest {
    let scope = scope(group, binding);
    let current = store.selected_snapshot(&scope, MAX_ITEMS).unwrap();
    let previous = current.as_ref().map(|current| {
        project(
            store,
            &scope,
            &current.transaction.selected,
            &mut allowance(),
        )
        .unwrap()
    });
    let selection = match (&current, &previous) {
        (Some(current), Some(previous)) => SelectionCondition::Selected {
            aggregate_generation: current.token.aggregate_generation(),
            accepted_base_sha256: current.token.accepted_base_sha256().into(),
            selection_sha256: current.token.selection_sha256().into(),
            revision: previous.revision(),
        },
        (None, None) => SelectionCondition::Absent {},
        _ => unreachable!(),
    };
    let condition = Condition {
        group,
        mode_epoch: store.sync_mode_snapshot().unwrap().epoch.unwrap(),
        binding_sha256: binding.into(),
        store_generation: catalog::current_generation(store).unwrap(),
        local_config_sha256: digest(b"synthetic local config"),
        selection,
    };
    let patch =
        Artifact::from_value(json!({"portable_configuration_version":1,"settings":settings}))
            .unwrap();
    let operation = Uuid::new_v4();
    let record = record::prepare(
        store.actor_id,
        operation,
        &condition,
        previous.as_ref(),
        &patch,
    )
    .unwrap()
    .unwrap();
    let publication = match current {
        Some(current) => store
            .commit_selected(&current.token, operation, vec![record], BTreeMap::new())
            .unwrap(),
        None => {
            let bytes = serde_json::to_vec_pretty(&record).unwrap();
            let hash = digest(&bytes);
            store
                .initialize_selected(
                    &scope,
                    SelectionChange {
                        operation,
                        accepted_base_sha256: genesis_base(root, &scope.key_sha256),
                        selected: Manifest {
                            format: 1,
                            transaction_id: operation,
                            scope: Scope::SelectedRecords,
                            records: vec![ObjectRef {
                                sha256: hash.clone(),
                                bytes: bytes.len() as u64,
                            }],
                            record_namespaces: BTreeMap::from([(
                                hash.clone(),
                                Namespace::PortableConfiguration,
                            )]),
                            media: vec![],
                            media_coverage: vec![],
                        },
                        retained: vec![],
                    },
                    BTreeMap::from([(hash, bytes)]),
                )
                .unwrap()
        }
    };
    publication.transaction.selected
}
fn drive(engine: &mut Engine, mut condition: impl FnMut(&Engine) -> bool) {
    for _ in 0..150 {
        if condition(engine) {
            return;
        }
        engine.step().unwrap();
    }
    panic!("portable Worker postcondition missing");
}
fn applied(engine: &Engine, manifest: &Manifest) -> bool {
    engine
        .checkpoint
        .scopes
        .get(&record::key_sha256())
        .and_then(|p| p.applied.as_ref())
        .is_some_and(|applied| applied.membership == receiving::membership(manifest).unwrap())
}
fn posts(wire: &Wire) -> usize {
    wire.requests()
        .iter()
        .filter(|request| request.starts_with("POST "))
        .count()
}

#[test]
fn explicit_portable_edits_publish_and_passive_store_receives_originals_without_seeding() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let mut sender = engine(&wire, a.clone(), group_id);
    let mut receiver = engine(&wire, b.clone(), group_id);
    sender.step().unwrap();
    receiver.step().unwrap();
    sender.step().unwrap();
    receiver.step().unwrap();
    assert_eq!(posts(&wire), 0);
    assert!(a
        .selected_snapshot(&scope(group_id, &binding), MAX_ITEMS)
        .unwrap()
        .is_none());
    assert!(b
        .selected_snapshot(&scope(group_id, &binding), MAX_ITEMS)
        .unwrap()
        .is_none());
    let first = edit(
        &a,
        group_id,
        &binding,
        &root,
        json!({"product_settings":{"writer_enabled":false}}),
    );
    drive(&mut sender, |engine| applied(engine, &first));
    let before = posts(&wire);
    drive(&mut receiver, |engine| applied(engine, &first));
    assert_eq!(posts(&wire), before);
    let received = b
        .selected_snapshot(&scope(group_id, &binding), MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert!(shared::same_membership(
        &first,
        &received.transaction.selected
    ));
    let next = edit(
        &a,
        group_id,
        &binding,
        &root,
        json!({"personalization":{"text":"synthetic private shared text"}}),
    );
    drive(&mut sender, |engine| applied(engine, &next));
    let before = posts(&wire);
    drive(&mut receiver, |engine| applied(engine, &next));
    assert_eq!(posts(&wire), before);
    let current = b
        .selected_snapshot(&scope(group_id, &binding), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let projected = project(
        &b,
        &scope(group_id, &binding),
        &current.transaction.selected,
        &mut allowance(),
    )
    .unwrap();
    assert_eq!(
        projected.artifact().value()["settings"]["product_settings"]["writer_enabled"],
        false
    );
    assert_eq!(
        projected.artifact().value()["settings"]["personalization"]["text"],
        "synthetic private shared text"
    );
    assert!(shared::same_membership(
        &next,
        &current.transaction.selected
    ));
    assert!(b.selected_attempt_inventory().unwrap().is_empty());
}

#[test]
fn competing_portable_initials_keep_only_one_active_declaration_and_retain_the_other() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let before_a = edit(
        &a,
        group_id,
        &binding,
        &root,
        json!({"product_settings":{"writer_enabled":true}}),
    );
    let before_b = edit(
        &b,
        group_id,
        &binding,
        &root,
        json!({"product_settings":{"writer_enabled":false}}),
    );
    let mut sender = engine(&wire, a.clone(), group_id);
    let mut receiver = engine(&wire, b.clone(), group_id);
    for _ in 0..150 {
        sender.step().unwrap();
        receiver.step().unwrap();
        if sender
            .checkpoint
            .scopes
            .get(&record::key_sha256())
            .and_then(|p| p.applied.as_ref())
            .is_some()
            && receiver
                .checkpoint
                .scopes
                .get(&record::key_sha256())
                .and_then(|p| p.applied.as_ref())
                .is_some()
        {
            break;
        }
    }
    let sa = a
        .selected_snapshot(&scope(group_id, &binding), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let sb = b
        .selected_snapshot(&scope(group_id, &binding), MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert!(shared::same_membership(
        &sa.transaction.selected,
        &sb.transaction.selected
    ));
    for (original, current) in [(&before_a, &sa), (&before_b, &sb)] {
        if !shared::same_membership(original, &current.transaction.selected) {
            assert!(current
                .transaction
                .retained
                .iter()
                .any(|retained| shared::same_membership(original, retained)));
        }
    }
    assert_eq!(sender.view().unwrap().unwrap().entries.len(), 1);
    assert_eq!(receiver.view().unwrap().unwrap().entries.len(), 1);
}

#[test]
fn activation_before_checkpoint_replays_the_original_portable_receive_without_posts() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding);
    let a = fixture.store("a", group_id, &binding);
    let b = fixture.store("b", group_id, &binding);
    let first = edit(
        &a,
        group_id,
        &binding,
        &root,
        json!({"research_policy":null}),
    );
    let mut sender = engine(&wire, a, group_id);
    drive(&mut sender, |engine| applied(engine, &first));
    let mut receiver = engine(&wire, b.clone(), group_id);
    b.set_fault(Fault::AfterActivation).unwrap();
    let mut interrupted = false;
    for _ in 0..150 {
        if receiver.step().is_err() {
            interrupted = true;
            break;
        }
    }
    assert!(interrupted);
    let receive = receiver.checkpoint.scopes[&record::key_sha256()]
        .receive
        .as_ref()
        .unwrap()
        .operation;
    assert!(b
        .selected_receipt(&scope(group_id, &binding), receive)
        .unwrap()
        .is_some());
    drop(receiver);
    drop(b);
    let b = Arc::new(Store::open_existing_for_mode(fixture.paths("b")).unwrap());
    let before = posts(&wire);
    let mut receiver = engine(&wire, b.clone(), group_id);
    drive(&mut receiver, |engine| applied(engine, &first));
    assert_eq!(posts(&wire), before);
    assert_eq!(
        receiver.checkpoint.scopes[&record::key_sha256()]
            .applied
            .as_ref()
            .unwrap()
            .operation,
        receive
    );
    assert!(b.selected_attempt_inventory().unwrap().is_empty());
}

#[test]
fn foreign_custody_at_a_known_empty_mapping_cannot_become_an_initial_publication() {
    let fixture = Fixture::new();
    let wire = Wire::new();
    let group_id = Uuid::new_v4();
    let binding = wire
        .client(fixture.0.join("provider/drive.json"), group_id)
        .binding_digest();
    let root = group(&wire, group_id, &binding);
    let catalog = Catalog {
        schema: WIRE,
        group: group_id,
        root_sha256: root.clone(),
        base_sha256: root.clone(),
        slot: "portable-catalog".into(),
        operation: Uuid::new_v4(),
        entries: BTreeMap::from([(
            record::key_sha256(),
            Entry {
                owner: Owner::preferences(),
                genesis_slot: "known-empty-portable-genesis".into(),
            },
        )]),
        next_slot: "portable-catalog-append".into(),
    };
    wire.slot(
        &SlotBinding {
            group: group_id,
            scope_sha256: catalog_key(group_id),
            slot_id: "portable-catalog".into(),
        },
        "portable-catalog-body",
        serde_json::to_vec(&catalog).unwrap(),
    );
    let store = fixture.store("imported", group_id, &binding);
    let operation = Uuid::new_v4();
    let original = Condition {
        group: group_id,
        mode_epoch: store.sync_mode_snapshot().unwrap().epoch.unwrap(),
        binding_sha256: binding.clone(),
        store_generation: catalog::current_generation(&store).unwrap(),
        local_config_sha256: digest(b"fixture config"),
        selection: SelectionCondition::Absent {},
    };
    let artifact=Artifact::from_value(json!({"portable_configuration_version":1,"settings":{"product_settings":{"writer_enabled":true}}})).unwrap();
    let mut foreign = Uuid::new_v4();
    while foreign == store.actor_id {
        foreign = Uuid::new_v4();
    }
    let record = record::prepare(foreign, operation, &original, None, &artifact)
        .unwrap()
        .unwrap();
    let bytes = serde_json::to_vec(&record).unwrap();
    let sha = digest(&bytes);
    let scope = scope(group_id, &binding);
    // Generic storage keeps this imported evidence; it does not authorize a
    // local first publication even if a caller supplied the matching root base.
    let published = store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation,
                accepted_base_sha256: genesis_base(&root, &scope.key_sha256),
                selected: Manifest {
                    format: 1,
                    transaction_id: operation,
                    scope: Scope::SelectedRecords,
                    records: vec![ObjectRef {
                        sha256: sha.clone(),
                        bytes: bytes.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(
                        sha.clone(),
                        Namespace::PortableConfiguration,
                    )]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(sha, bytes)]),
        )
        .unwrap();
    let mut worker = engine(&wire, store.clone(), group_id);
    let mut refused = false;
    for _ in 0..20 {
        if worker.step().is_err() {
            refused = true;
            break;
        }
    }
    assert!(refused);
    assert_eq!(posts(&wire), 0);
    assert!(store.selected_attempt_inventory().unwrap().is_empty());
    assert_eq!(
        store
            .selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        published.token
    );
}
