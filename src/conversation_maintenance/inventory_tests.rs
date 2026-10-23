// Synthetic Store/observer controls only. Never query a real unit.
fn empty_fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "buddy-conversation-maintenance-v1-{}",
        Uuid::new_v4()
    ));
    fs::create_dir(&root).unwrap();
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    let mut config = Config {
        paths: paths.clone(),
        ..Config::default()
    };
    config.sync.enabled = false;
    let path = root.join("config.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    drop(Store::open(paths.clone()).unwrap());
    Fixture {
        root,
        config: path,
        paths,
        conversation: Uuid::new_v4(),
    }
}
fn discover(f: &Fixture) -> InventoryReport {
    engine::inventory(Some(f.config.clone()), Arc::new(Inactive))
}
fn encoded(report: InventoryReport) -> serde_json::Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert!(bytes.len() <= 8192 && bytes.ends_with(b"\n"));
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 13);
    assert_eq!(value["scope"], "ordinary-current-root-candidates");
    for key in [
        "eligibility_evaluated",
        "mutation_authority",
        "native_authority",
        "physical_reclamation",
    ] {
        assert_eq!(value[key], false);
    }
    assert_eq!(value["runtime_unverified"], true);
    assert_eq!(value["local_only"], true);
    if value["status"] == "complete" {
        assert_eq!(value["complete"], true);
        assert_eq!(value["service_observation"], "inactive");
        assert_eq!(
            value["candidate_count"],
            value["candidates"].as_array().unwrap().len()
        );
    } else {
        assert_eq!(value["complete"], false);
        assert!(value["candidates"].is_null() && value["candidate_count"].is_null());
        assert_eq!(value["service_observation"], "unverified");
    }
    value
}
fn add_roots(f: &Fixture, ids: &[Uuid]) {
    let ledger = Ledger::new(Arc::new(Store::open(f.paths.clone()).unwrap()));
    for id in ids {
        ledger.create(*id, Uuid::new_v4(), 1).unwrap();
    }
}
fn delete_roots(f: &Fixture, ids: &[Uuid]) {
    let ledger = Ledger::new(Arc::new(Store::open(f.paths.clone()).unwrap()));
    for id in ids {
        ledger
            .delete(
                *id,
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, *id).unwrap(),
                None,
            )
            .unwrap();
    }
}
fn import_branch(store: &Store, e: Envelope) {
    let b = serde_json::to_vec(&e).unwrap();
    let sha = digest(&b);
    store
        .import(
            Manifest {
                format: 1,
                transaction_id: Uuid::new_v4(),
                scope: Scope::LocalTransaction,
                records: vec![ObjectRef {
                    sha256: sha.clone(),
                    bytes: b.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(sha.clone(), e.namespace)]),
                media: vec![],
                media_coverage: vec![],
            },
            BTreeMap::from([(sha, b)]),
        )
        .unwrap();
}
#[test]
fn inventory_empty_sorted_current_deleted_and_sixteen_seventeen_are_complete_or_refuse() {
    let f = empty_fixture();
    let before = f.state();
    let v = encoded(discover(&f));
    assert_eq!(v["candidates"], serde_json::json!([]));
    assert_eq!(before, f.state());
    let ids: Vec<_> = (1..=18).rev().map(Uuid::from_u128).collect();
    add_roots(&f, &ids[..16]);
    let before = f.state();
    let report = discover(&f);
    let reads = report.config_read_budget().unwrap();
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 2);
    let v = encoded(report);
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(v["candidate_count"], 16);
    assert_eq!(before, f.state());
    let observed: Vec<_> = v["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect();
    let mut wanted: Vec<_> = ids[..16].iter().map(Uuid::to_string).collect();
    wanted.sort();
    assert_eq!(observed, wanted);
    add_roots(&f, &ids[16..17]);
    let before = f.state();
    assert_eq!(encoded(discover(&f))["status"], "too-large");
    assert_eq!(before, f.state());
    add_roots(&f, &ids[17..]);
    delete_roots(&f, &ids[..3]);
    let before = f.state();
    let v = encoded(discover(&f));
    assert_eq!(v["candidate_count"], 15);
    assert_eq!(before, f.state());
    delete_roots(&f, &ids[3..]);
    let before = f.state();
    let v = encoded(discover(&f));
    assert_eq!(v["candidates"], serde_json::json!([]));
    assert_eq!(v["candidate_count"], 0);
    assert_eq!(before, f.state());
}
#[test]
fn inventory_pending_and_conflicted_descendants_never_become_cleanup_authority() {
    for conflict in [false, true] {
        let f = Fixture::new();
        append_pair(&f, conflict);
        if conflict {
            let store = Store::open(f.paths.clone()).unwrap();
            let mut turns = store
                .snapshot_heads_matching(&[Namespace::Conversation], 4096, |e| {
                    e.payload["record_kind"] == "turn" && e.payload["record"]["role"] == "user"
                })
                .unwrap();
            let mut e = turns.pop().unwrap();
            e.revision_id = Uuid::new_v4();
            e.operation_id = Uuid::new_v4();
            e.payload["record"]["text"] = serde_json::json!("PRIVATE_INVENTORY_TURN");
            import_branch(&store, e);
        }
        let before = f.state();
        let v = encoded(discover(&f));
        assert_eq!(
            v["candidates"],
            serde_json::json!([f.conversation.to_string()])
        );
        assert!(!v.to_string().contains("PRIVATE_INVENTORY"));
        assert_eq!(before, f.state());
        let r = f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        );
        assert_eq!(r.status, Status::Conflict);
        assert!(r.intent.is_none());
        drop(r);
        assert_eq!(before, f.state());
    }
}

#[test]
fn inventory_other_namespaces_and_untyped_tombstones_are_not_current_root_candidates() {
    let f = empty_fixture();
    let store = Store::open(f.paths.clone()).unwrap();
    let root_id = Uuid::from_u128(99);
    // Deliberate envelope-level fixture: a Root-like Source payload is not a
    // Conversation root. Discovery does not certify other domain eligibility.
    let mut e = Envelope {
        envelope_version: 1,
        namespace: Namespace::Source,
        domain_schema_version: 1,
        record_id: root_id,
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(Record::Root(Root {
            id: root_id,
            next_sequence: 0,
            binding: None,
            created_ms: 1,
            updated_ms: 1,
        }))
        .unwrap(),
        media_descriptors: vec![],
    };
    let source = e.clone();
    e.namespace = Namespace::Conversation;
    e.record_id = Uuid::from_u128(100);
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.kind = Kind::Tombstone;
    e.payload = serde_json::Value::Null;
    store.commit(vec![source, e], BTreeMap::new()).unwrap();
    drop(store);
    let before = f.state();
    let v = encoded(discover(&f));
    assert_eq!(v["candidates"], serde_json::json!([]));
    assert_eq!(before, f.state());
}
#[test]
fn inventory_root_head_conflicts_type_reuse_and_retained_schema_refuse_without_prefix() {
    for defect in [
        "root-conflict",
        "mixed-heads",
        "current-other",
        "retained-other",
        "schema-two",
        "unknown-kind",
        "wrong-id",
    ] {
        let f = Fixture::new();
        let store = Store::open(f.paths.clone()).unwrap();
        let original = store
            .value(Namespace::Conversation, f.conversation)
            .unwrap()
            .unwrap();
        let mut e = original.clone();
        e.parents = BTreeSet::from([e.revision_id]);
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        match defect {
            "root-conflict" => {
                e.parents = original.parents.clone();
                import_branch(&store, e);
            }
            "mixed-heads" => {
                let mut tombstone = e.clone();
                tombstone.kind = Kind::Tombstone;
                tombstone.payload = serde_json::Value::Null;
                store.commit(vec![tombstone], BTreeMap::new()).unwrap();
                e.revision_id = Uuid::new_v4();
                e.operation_id = Uuid::new_v4();
                import_branch(&store, e);
            }
            "current-other" | "retained-other" => {
                let mut turn = user(f.conversation, Uuid::new_v4());
                turn.id = f.conversation;
                e.payload = serde_json::to_value(Record::Turn(turn)).unwrap();
                let parent = e.revision_id;
                store.commit(vec![e], BTreeMap::new()).unwrap();
                if defect == "retained-other" {
                    let mut root = original;
                    root.parents = BTreeSet::from([parent]);
                    root.revision_id = Uuid::new_v4();
                    root.operation_id = Uuid::new_v4();
                    store.commit(vec![root], BTreeMap::new()).unwrap();
                }
            }
            "schema-two" => {
                e.domain_schema_version = 2;
                let parent = e.revision_id;
                store.commit(vec![e], BTreeMap::new()).unwrap();
                let mut root = original;
                root.parents = BTreeSet::from([parent]);
                root.revision_id = Uuid::new_v4();
                root.operation_id = Uuid::new_v4();
                store.commit(vec![root], BTreeMap::new()).unwrap();
            }
            "unknown-kind" => {
                e.payload["record_kind"] = serde_json::json!("future-root");
                store.commit(vec![e], BTreeMap::new()).unwrap();
            }
            _ => {
                e.payload["record"]["id"] = serde_json::json!(Uuid::new_v4());
                store.commit(vec![e], BTreeMap::new()).unwrap();
            }
        }
        drop(store);
        let before = f.state();
        let v = encoded(discover(&f));
        assert_eq!(
            v["status"],
            if matches!(defect, "schema-two" | "unknown-kind" | "wrong-id") {
                "unsupported"
            } else {
                "conflict"
            },
            "{defect}"
        );
        assert_eq!(before, f.state(), "{defect}");
    }
}
#[test]
fn inventory_storage_work_exhausts_before_the_real_typed_decoder_and_never_resets() {
    use storage::conversation_cleanup::{CandidateRefusal, Session};
    let f = empty_fixture();
    let store = Store::open(f.paths.clone()).unwrap();
    let e = Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::json!({"record_kind":"future-root","record":{}}),
        media_descriptors: vec![],
    };
    store.commit(vec![e], BTreeMap::new()).unwrap();
    drop(store);
    let before = f.state();
    {
        let mut s = Session::open(f.paths.clone(), &BTreeMap::new()).unwrap();
        assert_eq!(s.candidate_roots(), Err(CandidateRefusal::Unsupported));
    }
    for byte_limit in [true, false] {
        let mut s = Session::open(f.paths.clone(), &BTreeMap::new()).unwrap();
        if byte_limit {
            s.budget.bytes = 512 * 1024 * 1024;
        } else {
            s.budget.visits = 65536;
        }
        assert_eq!(s.candidate_roots(), Err(CandidateRefusal::Unavailable));
        assert!(s.budget.bytes <= 512 * 1024 * 1024 && s.budget.visits <= 65536);
        assert!(s.budget.charge(0, 0).is_err());
        drop(s);
        assert_eq!(before, f.state());
    }
}
#[test]
fn inventory_exhausted_discovery_emits_unavailable_and_holds_custody_through_output() {
    struct CheckedOutput {
        paths: StorePaths,
        bytes: Vec<u8>,
    }
    impl std::io::Write for CheckedOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            assert!(Store::open_existing(self.paths.clone()).is_err());
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for byte_limit in [true, false] {
        let f = Fixture::new();
        let before = f.state();
        let report =
            engine::inventory_at_work_limit(Some(f.config.clone()), Arc::new(Inactive), byte_limit);
        let mut out = CheckedOutput {
            paths: f.paths.clone(),
            bytes: vec![],
        };
        report.write_to(&mut out).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out.bytes).unwrap();
        assert_eq!(value["status"], "unavailable");
        assert_eq!(value["complete"], false);
        assert!(value["candidates"].is_null() && value["candidate_count"].is_null());
        assert_eq!(value["service_observation"], "unverified");
        drop(Store::open_existing(f.paths.clone()).unwrap());
        assert_eq!(before, f.state());
    }
}
#[test]
fn inventory_real_config_change_is_still_conflict_after_discovery_exhaustion() {
    for byte_limit in [true, false] {
        let f = Fixture::new();
        let report =
            engine::inventory_at_work_limit(Some(f.config.clone()), Arc::new(Inactive), byte_limit);
        let mut bytes = fs::read(&f.config).unwrap();
        bytes.push(b' ');
        fs::write(&f.config, &bytes).unwrap();
        assert_eq!(encoded(report)["status"], "conflict");
        assert_eq!(fs::read(&f.config).unwrap(), bytes);
        drop(Store::open_existing(f.paths.clone()).unwrap());
    }
}
#[cfg(unix)]
#[test]
fn inventory_real_lock_replacement_is_still_conflict_after_discovery_exhaustion() {
    for byte_limit in [true, false] {
        let f = Fixture::new();
        let report =
            engine::inventory_at_work_limit(Some(f.config.clone()), Arc::new(Inactive), byte_limit);
        let lock = f.paths.data.join("store.lock");
        let held = lock.with_extension("synthetic-held-lock");
        fs::rename(&lock, held).unwrap();
        fs::write(&lock, []).unwrap();
        assert_eq!(encoded(report)["status"], "conflict");
    }
}
#[test]
fn inventory_does_not_need_publication_headroom_and_keeps_staging_documents_and_modes() {
    let f = Fixture::new();
    let g: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
    let commits = f
        .paths
        .data
        .join("generations")
        .join(g.to_string())
        .join("commits");
    let n = fs::read_dir(&commits).unwrap().count();
    for _ in n..4096 {
        fs::write(commits.join(format!(".stage-{}", Uuid::new_v4())), []).unwrap();
    }
    fs::write(f.root.join("original.pdf"), b"PRIVATE_INVENTORY_ORIGINAL").unwrap();
    #[cfg(unix)]
    let permissions =
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&f.config).unwrap().permissions());
    let before = f.state();
    assert_eq!(encoded(discover(&f))["candidate_count"], 1);
    assert_eq!(before, f.state());
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&f.config).unwrap().permissions()),
        permissions
    );
}
#[test]
fn inventory_orphan_original_objects_refuse_without_adopting_cleanup_receipt_authority() {
    use storage::conversation_cleanup::{Plan, Session};
    let f = Fixture::new();
    let i = f.prepared();
    let plan = Plan::new(&i).unwrap();
    let mut s = Session::open(f.paths.clone(), &plan.known()).unwrap();
    let mode = s.store.sync_mode_snapshot().unwrap();
    let guard = s.store.ordinary_mode_lease(&mode.token).unwrap();
    let snapshot = s.snapshot(f.conversation).unwrap();
    s.reserve(&i, &plan, &snapshot).unwrap();
    s.store.set_fault(Fault::AfterObjects).unwrap();
    let mut attempted = false;
    assert!(s.publish(&i, &plan, &mut attempted).is_err() && attempted);
    drop(guard);
    drop(s);
    let before = f.state();
    assert_eq!(encoded(discover(&f))["status"], "unsupported");
    assert_eq!(before, f.state());
    let r = f.confirm(Operation::Status, &i);
    assert_eq!(r.status, Status::Unresolved);
    drop(r);
    assert_eq!(before, f.state());
}
#[test]
fn inventory_held_owner_missing_structure_and_unsupported_modes_preserve_all_files() {
    let f = Fixture::new();
    let before = f.state();
    // The initializer rewrites capability/contract files. A read-only existing
    // owner models the competing lease without adding fixture mutations.
    let held = Store::open_existing(f.paths.clone()).unwrap();
    assert_eq!(encoded(discover(&f))["status"], "busy");
    drop(held);
    assert_eq!(before, f.state());
    for defect in 0..7 {
        let f = Fixture::new();
        match defect {
            0 => fs::remove_file(f.paths.data.join("store.lock")).unwrap(),
            1 => fs::write(f.paths.cache.join("owner.json"), b"invalid").unwrap(),
            2 | 4 => {
                let store = Store::open(f.paths.clone()).unwrap();
                let token = store.sync_mode_snapshot().unwrap().token;
                let choice = if defect == 2 {
                    storage::sync::mode::SyncModeChoice::IsolatedBackup {
                        binding_sha256: "a".repeat(64),
                    }
                } else {
                    storage::sync::mode::SyncModeChoice::SharedGroup {
                        group: Uuid::new_v4(),
                        binding_sha256: "b".repeat(64),
                        root_id: "synthetic-root".into(),
                    }
                };
                store.set_sync_mode(&token, Uuid::new_v4(), choice).unwrap();
            }
            5 => {
                let mut config: Config =
                    serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
                config.sync.enabled = true;
                fs::write(&f.config, serde_json::to_vec(&config).unwrap()).unwrap();
            }
            _ => {
                let g: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
                let p = f.paths.data.join("generations").join(g.to_string());
                if defect == 6 {
                    fs::write(p.join("format.json"), b"2").unwrap();
                } else {
                    fs::write(p.join("selection-feature.json"), b"{}").unwrap();
                }
            }
        }
        let before = f.state();
        let v = encoded(discover(&f));
        assert!(matches!(
            v["status"].as_str(),
            Some("unsupported" | "unavailable")
        ));
        assert_eq!(before, f.state());
    }
}
#[test]
fn inventory_explicit_local_mode_and_bounded_config_load_are_independent() {
    let f = Fixture::new();
    let store = Store::open(f.paths.clone()).unwrap();
    let mode = store.sync_mode_snapshot().unwrap();
    store
        .set_sync_mode(
            &mode.token,
            Uuid::new_v4(),
            storage::sync::mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    drop(store);
    let before = f.state();
    assert_eq!(encoded(discover(&f))["status"], "complete");
    assert_eq!(before, f.state());
    struct NeverObserve;
    impl service::Observer for NeverObserve {
        fn inactive(&self) -> std::result::Result<(), Status> {
            panic!("Config refusal reached service")
        }
    }
    let f = Fixture::new();
    fs::write(&f.config, vec![b' '; 1024 * 1024 + 1]).unwrap();
    let before = f.state();
    let v = encoded(engine::inventory(
        Some(f.config.clone()),
        Arc::new(NeverObserve),
    ));
    assert_eq!(v["status"], "unavailable");
    assert_eq!(before, f.state());
    assert_eq!(
        encoded(engine::inventory(None, Arc::new(NeverObserve)))["status"],
        "unavailable"
    );
}
#[test]
fn inventory_final_config_change_and_observer_refusal_clear_every_candidate() {
    let f = Fixture::new();
    let observer = Arc::new(ChangedOnObservation {
        config: f.config.clone(),
        lock: f.paths.data.join("store.lock"),
        at: 3,
        calls: std::sync::atomic::AtomicUsize::new(0),
        replace_lock: false,
    });
    let r = engine::inventory(Some(f.config.clone()), observer);
    let reads = r.config_read_budget().unwrap();
    let v = encoded(r);
    assert_eq!(v["status"], "conflict");
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    struct BusyAtFinal(std::sync::atomic::AtomicUsize);
    impl service::Observer for BusyAtFinal {
        fn inactive(&self) -> std::result::Result<(), Status> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 2 {
                Err(Status::Busy)
            } else {
                Ok(())
            }
        }
    }
    let f = Fixture::new();
    let before = f.state();
    let v = encoded(engine::inventory(
        Some(f.config.clone()),
        Arc::new(BusyAtFinal(std::sync::atomic::AtomicUsize::new(0))),
    ));
    assert_eq!(v["status"], "busy");
    assert_eq!(before, f.state());
}
#[cfg(unix)]
#[test]
fn inventory_final_named_lock_replacement_is_not_a_complete_empty_result() {
    let f = Fixture::new();
    let observer = Arc::new(ChangedOnObservation {
        config: f.config.clone(),
        lock: f.paths.data.join("store.lock"),
        at: 3,
        calls: std::sync::atomic::AtomicUsize::new(0),
        replace_lock: true,
    });
    assert_eq!(
        encoded(engine::inventory(Some(f.config.clone()), observer))["status"],
        "conflict"
    );
}
#[test]
fn inventory_partial_output_failure_holds_custody_for_complete_and_refused_discovery() {
    struct Broken {
        paths: StorePaths,
        calls: usize,
        prefix: Vec<u8>,
    }
    impl std::io::Write for Broken {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            assert!(Store::open_existing(self.paths.clone()).is_err());
            self.calls += 1;
            if self.calls == 1 {
                self.prefix.extend_from_slice(&b[..5]);
                Ok(5)
            } else {
                assert_eq!(self.calls, 2, "second report attempted");
                Err(std::io::Error::other("synthetic output loss"))
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for too_many in [false, true] {
        let f = Fixture::new();
        if too_many {
            let ids: Vec<_> = (1..=16).map(Uuid::from_u128).collect();
            add_roots(&f, &ids);
        }
        let before = f.state();
        let r = discover(&f);
        let reads = r.config_read_budget().unwrap();
        let mut out = Broken {
            paths: f.paths.clone(),
            calls: 0,
            prefix: vec![],
        };
        assert!(r.write_to(&mut out).is_err());
        assert_eq!(out.calls, 2);
        assert_eq!(out.prefix.len(), 5);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 0);
        drop(Store::open_existing(f.paths.clone()).unwrap());
        assert_eq!(before, f.state());
    }
}
#[cfg(not(target_os = "linux"))]
#[test]
fn inventory_public_nonlinux_boundary_never_admits_an_existing_store() {
    let f = Fixture::new();
    let before = f.state();
    assert_eq!(
        encoded(inspect_inventory(Some(f.config.clone())))["status"],
        "unsupported"
    );
    assert_eq!(before, f.state());
}
