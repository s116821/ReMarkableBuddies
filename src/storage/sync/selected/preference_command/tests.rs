use super::*;
use crate::storage::{compound_generation, drive::shared_fixture::Wire, StorePaths};
use serde_json::json;

struct Fixture {
    root: PathBuf,
    store: Arc<Store>,
    config: PathBuf,
    group: Uuid,
    binding: String,
}
impl Fixture {
    fn new(wire: &Wire) -> Self {
        Self::in_group(wire, Uuid::new_v4())
    }
    fn in_group(wire: &Wire, group: Uuid) -> Self {
        let root =
            std::env::temp_dir().join(format!("buddy-preference-command-{}", Uuid::new_v4()));
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let binding = wire
            .client(root.join("provider/drive.json"), group)
            .binding_digest();
        let store = Store::open(paths.clone()).unwrap();
        let request = compound_generation::Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: catalog::current_generation(&store).unwrap(),
            group,
            binding_sha256: binding.clone(),
            expected_mode: serde_json::from_value(
                serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap(),
            )
            .unwrap(),
        };
        drop(store);
        let store = Arc::new(Store::prepare_shared_notebook(paths.clone(), request).unwrap());
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                mode::SyncModeChoice::SharedGroup {
                    group,
                    binding_sha256: binding.clone(),
                    root_id: "command-root".into(),
                },
            )
            .unwrap();
        let config = root.join("config.json");
        files::atomic_json(&config,&json!({"paths":paths,"product_settings":{"writer_enabled":true},"personalization":{"text":"LOCAL PRIVATE"}})).unwrap();
        Self {
            root,
            store,
            config,
            group,
            binding,
        }
    }
    fn bootstrap(&self, wire: &Wire) {
        let root = Root {
            schema: WIRE,
            group: self.group,
            protocol: "buddy-shared-fixed-slot-v1".into(),
            binding_sha256: self.binding.clone(),
            profile: Uuid::new_v4(),
            catalog_slot: "command-catalog".into(),
        };
        let bytes = serde_json::to_vec(&root).unwrap();
        let sha = digest(&bytes);
        wire.slot(
            &SlotBinding {
                group: self.group,
                scope_sha256: root_key(self.group),
                slot_id: "command-root".into(),
            },
            "command-root-body",
            bytes,
        );
        let catalog = Catalog {
            schema: WIRE,
            group: self.group,
            root_sha256: sha.clone(),
            base_sha256: sha,
            slot: "command-catalog".into(),
            operation: Uuid::new_v4(),
            entries: BTreeMap::new(),
            next_slot: "command-tail".into(),
        };
        wire.slot(
            &SlotBinding {
                group: self.group,
                scope_sha256: catalog_key(self.group),
                slot_id: "command-catalog".into(),
            },
            "command-catalog-body",
            serde_json::to_vec(&catalog).unwrap(),
        );
    }
    fn command(&self, wire: &Wire, request: Request, allow_network: bool) -> Result<Report> {
        let mut reads = budget();
        let capture = Capture::read(self.config.clone(), &mut reads)?;
        admitted(
            self.store.clone(),
            capture,
            request,
            Binding {
                group: self.group,
                root_id: "command-root".into(),
                digest: self.binding.clone(),
            },
            Instant::now() + Duration::from_secs(60),
            &mut reads,
            || {
                ensure!(allow_network, "unexpected provider access");
                Ok(wire.client(self.store.paths.credentials.join("drive.json"), self.group))
            },
        )
    }
    fn inspect(&self, wire: &Wire, allow_network: bool) -> Condition {
        let report = self
            .command(wire, Request::Inspect(false), allow_network)
            .unwrap();
        assert_eq!(report.status, "ok");
        assert!(report
            .view
            .as_ref()
            .unwrap()
            .get("personalization_text")
            .is_none());
        report.guard.clone().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn patch(value: serde_json::Value) -> Artifact {
    Artifact::from_value(json!({"portable_configuration_version":1,"settings":value})).unwrap()
}
fn posts(wire: &Wire) -> usize {
    wire.requests()
        .iter()
        .filter(|r| r.starts_with("POST "))
        .count()
}

#[test]
fn real_admission_first_absence_selected_offline_and_original_status_after_mode_departure() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let before = posts(&wire);
    let guard = fixture.inspect(&wire, true);
    assert!(matches!(guard.selection, SelectionCondition::Absent {}));
    assert_eq!(posts(&wire), before);
    let operation = Uuid::new_v4();
    let edit = patch(
        json!({"product_settings":{"writer_enabled":false},"personalization":{"enabled":true,"text":"SHARED PRIVATE"}}),
    );
    let hash = edit.sha256();
    let report = fixture
        .command(&wire, Request::Edit(operation, guard.clone(), edit), true)
        .unwrap();
    assert_eq!(report.disposition, "local-committed");
    drop(report);
    let before = wire.requests().len();
    let current = fixture.inspect(&wire, false);
    assert!(matches!(
        current.selection,
        SelectionCondition::Selected { .. }
    ));
    assert_eq!(wire.requests().len(), before);
    let reveal = fixture
        .command(&wire, Request::Inspect(true), false)
        .unwrap();
    assert_eq!(
        reveal.view.as_ref().unwrap()["personalization_text"],
        "SHARED PRIVATE"
    );
    drop(reveal);
    let noop = fixture
        .command(
            &wire,
            Request::Edit(Uuid::new_v4(), current.clone(), patch(json!({}))),
            false,
        )
        .unwrap();
    assert_eq!(noop.disposition, "no-change");
    drop(noop);
    fixture
        .store
        .set_sync_mode(
            &fixture.store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    let query = Query {
        shared_preferences_version: 1,
        operation,
        guard: guard.clone(),
        patch_sha256: hash.clone(),
    };
    assert!(original_status(&fixture.store, &query, &mut budget()).unwrap());
    let wrong = Query {
        shared_preferences_version: 1,
        operation,
        guard,
        patch_sha256: digest(b"another patch"),
    };
    assert!(original_status(&fixture.store, &wrong, &mut budget()).is_err());
    assert_eq!(posts(&wire), 0);
}

#[test]
fn absent_guard_rechecks_bootstrap_and_config_and_never_seeds_from_missing_root() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    let report = fixture
        .command(&wire, Request::Inspect(false), true)
        .unwrap();
    assert_eq!(report.status, "joining");
    assert!(report.guard.is_none());
    drop(report);
    assert!(fixture
        .store
        .selected_snapshot(
            &SelectionScope {
                group: fixture.group,
                key_sha256: record::key_sha256(),
                binding_sha256: fixture.binding.clone()
            },
            MAX_ITEMS
        )
        .unwrap()
        .is_none());
    fixture.bootstrap(&wire);
    let guard = fixture.inspect(&wire, true);
    files::atomic_json(
        &fixture.config,
        &json!({"paths":fixture.store.paths,"personalization":{"text":"changed"}}),
    )
    .unwrap();
    let refused = fixture
        .command(
            &wire,
            Request::Edit(
                Uuid::new_v4(),
                guard,
                patch(json!({"research_policy":null})),
            ),
            false,
        )
        .unwrap();
    assert_eq!(refused.status, "conflict");
    drop(refused);
    assert_eq!(posts(&wire), 0);
}

#[test]
fn original_commit_status_rejects_changed_predecessor_and_patch_but_survives_later_edits() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let guard = fixture.inspect(&wire, true);
    drop(
        fixture
            .command(
                &wire,
                Request::Edit(
                    Uuid::new_v4(),
                    guard,
                    patch(json!({"research_policy":null})),
                ),
                true,
            )
            .unwrap(),
    );
    let guard = fixture.inspect(&wire, false);
    let operation = Uuid::new_v4();
    let edit = patch(json!({"product_settings":{"reader_enabled":false}}));
    let hash = edit.sha256();
    drop(
        fixture
            .command(&wire, Request::Edit(operation, guard.clone(), edit), false)
            .unwrap(),
    );
    let next = fixture.inspect(&wire, false);
    drop(
        fixture
            .command(
                &wire,
                Request::Edit(
                    Uuid::new_v4(),
                    next,
                    patch(json!({"personalization":{"enabled":false}})),
                ),
                false,
            )
            .unwrap(),
    );
    let mut query = Query {
        shared_preferences_version: 1,
        operation,
        guard,
        patch_sha256: hash,
    };
    assert!(original_status(&fixture.store, &query, &mut budget()).unwrap());
    query.guard.local_config_sha256 = digest(b"wrong original config");
    assert!(original_status(&fixture.store, &query, &mut budget()).is_err());
}

#[test]
fn first_edit_rechecks_accepted_catalog_after_an_absent_inspection() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let guard = fixture.inspect(&wire, true);
    let cp = checkpoint(
        &fixture.store,
        &Binding {
            group: fixture.group,
            root_id: "command-root".into(),
            digest: fixture.binding.clone(),
        },
        guard.mode_epoch,
        &mut budget(),
    )
    .unwrap()
    .unwrap();
    let view = catalog_view(
        &Binding {
            group: fixture.group,
            root_id: "command-root".into(),
            digest: fixture.binding.clone(),
        },
        &cp,
    )
    .unwrap()
    .unwrap();
    let added = Catalog {
        schema: WIRE,
        group: fixture.group,
        root_sha256: view.root_sha,
        base_sha256: view.base,
        slot: view.slot.clone(),
        operation: Uuid::new_v4(),
        entries: BTreeMap::from([(
            record::key_sha256(),
            Entry {
                owner: Owner::preferences(),
                genesis_slot: "another-explicit-genesis".into(),
            },
        )]),
        next_slot: "new-command-tail".into(),
    };
    wire.slot(
        &SlotBinding {
            group: fixture.group,
            scope_sha256: catalog_key(fixture.group),
            slot_id: view.slot,
        },
        "competing-catalog-body",
        serde_json::to_vec(&added).unwrap(),
    );
    let operation = Uuid::new_v4();
    let report = fixture
        .command(
            &wire,
            Request::Edit(
                operation,
                guard.clone(),
                patch(json!({"research_policy":null})),
            ),
            true,
        )
        .unwrap();
    assert_eq!(report.status, "joining");
    assert_eq!(report.original_operation, Some(operation));
    drop(report);
    assert!(fixture
        .store
        .selected_receipt(&scope(&guard), operation)
        .unwrap()
        .is_none());
    assert_eq!(posts(&wire), 0);
}

#[test]
fn deadline_and_read_allowance_refuse_before_any_provider_get() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    for expired in [true, false] {
        let mut reads = SelectedReadBudget::new(64, MAX_ITEMS as u64, MAX_RECORD as u64 * 4, 0);
        let capture = Capture::read(fixture.config.clone(), &mut reads).unwrap();
        let before = wire.requests().len();
        let result = admitted(
            fixture.store.clone(),
            capture,
            Request::Inspect(false),
            Binding {
                group: fixture.group,
                root_id: "command-root".into(),
                digest: fixture.binding.clone(),
            },
            if expired {
                Instant::now() - Duration::from_secs(1)
            } else {
                Instant::now() + Duration::from_secs(60)
            },
            &mut reads,
            || {
                Ok(wire.client(
                    fixture.store.paths.credentials.join("drive.json"),
                    fixture.group,
                ))
            },
        );
        assert!(result.is_err());
        assert_eq!(wire.requests().len(), before);
    }
}

#[test]
fn strict_requests_and_closed_view_preserve_privacy_and_sparse_origins() {
    for bytes in [
        br#"{"shared_preferences_version":1,"reveal_text":false,"reveal_text":true}"#.as_slice(),
        br#"{"shared_preferences_version":1,"reveal_text":false,"unexpected":"PRIVATE"}"#,
        br#"{"shared_preferences_version":2,"reveal_text":false}"#,
        b"\xff",
    ] {
        assert!(Request::decode(Operation::Inspect, bytes).is_err());
    }
    let local=LocalConfigSnapshot::parse(br#"{"model":"PRIVATE MODEL","personalization":{"text":"LOCAL PRIVATE"},"product_settings":null}"#).unwrap();
    let artifact = patch(
        json!({"product_settings":{},"personalization":{"enabled":true},"research_policy":null}),
    );
    let view = local.view(Some(&artifact), false).unwrap();
    let text = serde_json::to_string(&view).unwrap();
    assert!(!text.contains("PRIVATE"));
    assert_eq!(view["origins"]["reader_enabled"], "default");
    assert_eq!(view["origins"]["personalization_text"], "local");
    assert_eq!(view["origins"]["personalization_enabled"], "shared");
    assert_eq!(view["origins"]["research_policy"], "shared");
    assert_eq!(
        local.view(Some(&artifact), true).unwrap()["personalization_text"],
        "LOCAL PRIVATE"
    );
}

#[test]
fn actual_edit_activation_fault_and_failed_output_resolve_only_the_original_receipt() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let guard = fixture.inspect(&wire, true);
    let operation = Uuid::new_v4();
    let edit = patch(json!({"research_policy":null}));
    let hash = edit.sha256();
    fixture.store.set_fault(Fault::AfterActivation).unwrap();
    assert!(fixture
        .command(&wire, Request::Edit(operation, guard.clone(), edit), true)
        .is_err());
    let query = Query {
        shared_preferences_version: 1,
        operation,
        guard: guard.clone(),
        patch_sha256: hash,
    };
    assert!(original_status(&fixture.store, &query, &mut budget()).unwrap());
    let repeated = fixture
        .command(
            &wire,
            Request::Edit(operation, guard, patch(json!({"research_policy":null}))),
            false,
        )
        .unwrap();
    assert_eq!(repeated.status, "conflict");
    drop(repeated);
    let current = fixture.inspect(&wire, false);
    let operation = Uuid::new_v4();
    let edit = patch(json!({"product_settings":{"writer_enabled":false}}));
    let hash = edit.sha256();
    let report = fixture
        .command(
            &wire,
            Request::Edit(operation, current.clone(), edit),
            false,
        )
        .unwrap();
    struct Lost(usize);
    impl Write for Lost {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            Err(std::io::Error::other("lost output"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Lost(0);
    assert!(report.write_to(&mut output).is_err());
    assert_eq!(output.0, 1);
    assert!(original_status(
        &fixture.store,
        &Query {
            shared_preferences_version: 1,
            operation,
            guard: current,
            patch_sha256: hash
        },
        &mut budget()
    )
    .unwrap());
    assert_eq!(posts(&wire), 0);
}

#[test]
fn config_changed_during_get_is_refused_and_provider_io_holds_no_store_mutex() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let path = fixture.config.clone();
    let store = fixture.store.clone();
    let changed = Arc::new(AtomicBool::new(false));
    let flag = changed.clone();
    wire.observe_requests(move |request| {
        assert!(store.inner.try_lock().is_ok());
        if request.starts_with("GET ") && !flag.swap(true, Ordering::SeqCst) {
            files::atomic_json(
                &path,
                &json!({"paths":store.paths,"personalization":{"text":"changed during GET"}}),
            )
            .unwrap();
        }
    });
    assert!(fixture
        .command(&wire, Request::Inspect(false), true)
        .is_err());
    assert!(changed.load(Ordering::SeqCst));
    assert!(fixture
        .store
        .selected_snapshot(
            &SelectionScope {
                group: fixture.group,
                key_sha256: record::key_sha256(),
                binding_sha256: fixture.binding.clone()
            },
            MAX_ITEMS
        )
        .unwrap()
        .is_none());
    assert_eq!(posts(&wire), 0);
}

#[test]
fn original_custody_without_selected_data_is_unresolved_rather_than_fresh_absence() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let guard = fixture.inspect(&wire, true);
    let path = fixture.store.paths.data.join("sync/state.json");
    let mut state: State = files::json(&path, MAX_METADATA as u64).unwrap();
    let cp = state
        .selected
        .as_mut()
        .unwrap()
        .epochs
        .get_mut(&guard.mode_epoch)
        .unwrap();
    let original = Uuid::new_v4();
    let key = record::key_sha256();
    cp.catalog_pending = Some(CatalogPending {
        operation: Uuid::new_v5(
            &original,
            format!("shared-catalog-register:v1:{key}").as_bytes(),
        ),
        source_generation: guard.store_generation,
        registration: None,
        portable: Some(PortableRegistration {
            genesis_operation: original,
            genesis_sha256: digest(b"unresolved original"),
        }),
        genesis_slot: None,
        proposal: None,
    });
    files::atomic_json(&path, &state).unwrap();
    let before = wire.requests().len();
    let report = fixture
        .command(&wire, Request::Inspect(false), false)
        .unwrap();
    assert_eq!(report.status, "joining");
    assert!(report.guard.is_none());
    assert_eq!(wire.requests().len(), before);
}

#[test]
fn changed_config_at_output_invalidates_inspection_and_leaves_a_committed_edit_unknown() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let report = fixture
        .command(&wire, Request::Inspect(true), true)
        .unwrap();
    files::atomic_json(
        &fixture.config,
        &json!({"paths":fixture.store.paths,"personalization":{"text":"NEW PRIVATE"}}),
    )
    .unwrap();
    let mut bytes = vec![];
    report.write_to(&mut bytes).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "unavailable");
    assert!(value.get("guard").is_none());
    assert!(value.get("view").is_none());
    assert!(!String::from_utf8(bytes).unwrap().contains("PRIVATE"));
    let guard = fixture.inspect(&wire, true);
    let operation = Uuid::new_v4();
    let edit = patch(json!({"research_policy":null}));
    let hash = edit.sha256();
    let report = fixture
        .command(&wire, Request::Edit(operation, guard.clone(), edit), true)
        .unwrap();
    files::atomic_json(&fixture.config, &json!({"paths":fixture.store.paths})).unwrap();
    let mut bytes = vec![];
    report.write_to(&mut bytes).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["disposition"], "unknown");
    assert_eq!(value["original_operation"], operation.to_string());
    assert!(original_status(
        &fixture.store,
        &Query {
            shared_preferences_version: 1,
            operation,
            guard,
            patch_sha256: hash
        },
        &mut budget()
    )
    .unwrap());
    let guard = fixture.inspect(&wire, false);
    let noop = Uuid::new_v4();
    let report = fixture
        .command(&wire, Request::Edit(noop, guard, patch(json!({}))), false)
        .unwrap();
    files::atomic_json(
        &fixture.config,
        &json!({"paths":fixture.store.paths,"personalization":{"enabled":true}}),
    )
    .unwrap();
    let mut bytes = vec![];
    report.write_to(&mut bytes).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["disposition"], "no-change");
    let guard = fixture.inspect(&wire, false);
    let mut stale = guard.clone();
    stale.local_config_sha256 = digest(b"stale original config");
    let report = fixture
        .command(
            &wire,
            Request::Edit(
                Uuid::new_v4(),
                stale,
                patch(json!({"research_policy":null})),
            ),
            false,
        )
        .unwrap();
    files::atomic_json(&fixture.config, &json!({"paths":fixture.store.paths})).unwrap();
    let mut bytes = vec![];
    report.write_to(&mut bytes).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "conflict");
    assert_eq!(value["disposition"], "not-dispatched");
    assert!(fixture
        .store
        .selected_receipt(&scope(&guard), noop)
        .unwrap()
        .is_none());
}

#[test]
fn actual_command_producer_worker_second_store_and_reverse_edit_keep_original_custody() {
    let wire = Wire::new();
    let a = Fixture::new(&wire);
    a.bootstrap(&wire);
    let b = Fixture::in_group(&wire, a.group);
    assert_eq!(a.binding, b.binding);
    let guard = a.inspect(&wire, true);
    let operation = Uuid::new_v4();
    let edit = patch(json!({"product_settings":{"writer_enabled":false},"research_policy":null}));
    let hash = edit.sha256();
    drop(
        a.command(&wire, Request::Edit(operation, guard.clone(), edit), true)
            .unwrap(),
    );
    assert_eq!(posts(&wire), 0);
    let make_worker = |fixture: &Fixture| {
        Engine::new(
            fixture.store.clone(),
            SyncPolicy {
                batch_items: 1,
                ..Default::default()
            },
            wire.client(
                fixture.store.paths.credentials.join("drive.json"),
                fixture.group,
            ),
            Binding {
                group: fixture.group,
                root_id: "command-root".into(),
                digest: fixture.binding.clone(),
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    };
    let current_manifest = |fixture: &Fixture| {
        fixture
            .store
            .selected_snapshot(
                &SelectionScope {
                    group: fixture.group,
                    key_sha256: record::key_sha256(),
                    binding_sha256: fixture.binding.clone(),
                },
                MAX_ITEMS,
            )
            .unwrap()
            .map(|r| r.transaction.selected)
    };
    let applied = |engine: &Engine, manifest: &Manifest| {
        engine
            .checkpoint
            .scopes
            .get(&record::key_sha256())
            .and_then(|p| p.applied.as_ref())
            .is_some_and(|p| p.membership == receiving::membership(manifest).unwrap())
    };
    let original = current_manifest(&a).unwrap();
    let mut sender = make_worker(&a);
    for _ in 0..150 {
        if applied(&sender, &original) {
            break;
        }
        sender.step().unwrap();
    }
    assert!(applied(&sender, &original));
    let before = posts(&wire);
    let joining = b.command(&wire, Request::Inspect(false), true).unwrap();
    assert_eq!(joining.status, "joining");
    assert!(joining.guard.is_none());
    drop(joining);
    assert!(current_manifest(&b).is_none());
    assert_eq!(posts(&wire), before);
    let mut receiver = make_worker(&b);
    for _ in 0..150 {
        if applied(&receiver, &original) {
            break;
        }
        receiver.step().unwrap();
    }
    assert!(applied(&receiver, &original));
    assert_eq!(posts(&wire), before);
    assert!(shared::same_membership(
        &original,
        &current_manifest(&b).unwrap()
    ));
    let inspected = b.command(&wire, Request::Inspect(false), false).unwrap();
    assert_eq!(inspected.view.as_ref().unwrap()["writer_enabled"], false);
    assert_eq!(
        inspected.view.as_ref().unwrap()["origins"]["writer_enabled"],
        "shared"
    );
    let b_guard = inspected.guard.clone().unwrap();
    drop(inspected);
    assert!(!original_status(
        &b.store,
        &Query {
            shared_preferences_version: 1,
            operation,
            guard: guard.clone(),
            patch_sha256: hash.clone()
        },
        &mut budget()
    )
    .unwrap());
    let back_operation = Uuid::new_v4();
    let back = patch(json!({"personalization":{"enabled":true,"text":"EXPLICIT SHARED TEXT"}}));
    let back_hash = back.sha256();
    drop(
        b.command(
            &wire,
            Request::Edit(back_operation, b_guard.clone(), back),
            false,
        )
        .unwrap(),
    );
    let revised = current_manifest(&b).unwrap();
    for _ in 0..150 {
        if applied(&receiver, &revised) {
            break;
        }
        receiver.step().unwrap();
    }
    assert!(applied(&receiver, &revised));
    let before = posts(&wire);
    for _ in 0..150 {
        if applied(&sender, &revised) {
            break;
        }
        sender.step().unwrap();
    }
    assert!(applied(&sender, &revised));
    assert_eq!(posts(&wire), before);
    assert!(shared::same_membership(
        &revised,
        &current_manifest(&a).unwrap()
    ));
    let disclosed = a.command(&wire, Request::Inspect(true), false).unwrap();
    assert_eq!(
        disclosed.view.as_ref().unwrap()["personalization_text"],
        "EXPLICIT SHARED TEXT"
    );
    assert_eq!(disclosed.view.as_ref().unwrap()["writer_enabled"], false);
    drop(disclosed);
    assert!(original_status(
        &a.store,
        &Query {
            shared_preferences_version: 1,
            operation,
            guard,
            patch_sha256: hash
        },
        &mut budget()
    )
    .unwrap());
    assert!(original_status(
        &b.store,
        &Query {
            shared_preferences_version: 1,
            operation: back_operation,
            guard: b_guard,
            patch_sha256: back_hash
        },
        &mut budget()
    )
    .unwrap());
}

#[test]
fn constructor_reload_allowance_exhaustion_precedes_the_first_provider_observation() {
    let wire = Wire::new();
    let fixture = Fixture::new(&wire);
    fixture.bootstrap(&wire);
    let _ = fixture.inspect(&wire, true);
    assert!(fixture.store.paths.data.join("sync/state.json").exists());
    // Enough for caller-local reads and the first slot observation, but not
    // also Engine's independently bounded MAX_METADATA checkpoint reload.
    let mut reads = SelectedReadBudget::new(
        64,
        MAX_ITEMS as u64,
        3 * MAX_METADATA as u64 + 64 * 1024 + 4 * MAX_RECORD as u64,
        0,
    );
    let capture = Capture::read(fixture.config.clone(), &mut reads).unwrap();
    let before = wire.requests().len();
    let result = admitted(
        fixture.store.clone(),
        capture,
        Request::Inspect(false),
        Binding {
            group: fixture.group,
            root_id: "command-root".into(),
            digest: fixture.binding.clone(),
        },
        Instant::now() + Duration::from_secs(60),
        &mut reads,
        || {
            Ok(wire.client(
                fixture.store.paths.credentials.join("drive.json"),
                fixture.group,
            ))
        },
    );
    assert!(result.is_err());
    assert_eq!(
        wire.requests().len(),
        before,
        "constructor and observer must share the original allowance"
    );
    assert_eq!(posts(&wire), 0);
}
