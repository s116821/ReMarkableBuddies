//! Actual one-shot binary operations on isolated host Stores. No tablet/provider/service access.
use remarkable_reader_buddy::{
    memory::*,
    storage::{self, Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    profile: Uuid,
}

fn inspection_fork(store: &Store, original: &storage::Envelope, payload: Value, tombstone: bool) {
    let mut fork = original.clone();
    fork.parents = BTreeSet::from([original.revision_id]);
    fork.revision_id = Uuid::new_v4();
    fork.operation_id = Uuid::new_v4();
    fork.actor_id = Uuid::new_v4();
    fork.kind = if tombstone {
        storage::Kind::Tombstone
    } else {
        storage::Kind::Value
    };
    fork.payload = payload;
    let bytes = serde_json::to_vec(&fork).unwrap();
    let hash = storage::digest(&bytes);
    store
        .import(
            storage::Manifest {
                format: 1,
                transaction_id: Uuid::new_v4(),
                scope: storage::Scope::LocalTransaction,
                records: vec![storage::ObjectRef {
                    sha256: hash.clone(),
                    bytes: bytes.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(
                    hash.clone(),
                    storage::Namespace::SubjectMemory,
                )]),
                media: vec![],
                media_coverage: vec![],
            },
            BTreeMap::from([(hash, bytes)]),
        )
        .unwrap();
}
fn inspection_selected_pair(f: &Fixture) -> (catalog::Identity, Fact, Fact) {
    let identity = selected_config(f);
    let fact = f.fact();
    let other = f.fact();
    let initialize = |fact: &Fact| {
        f.run("initialize-aggregate",&json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"inspect-stdin aggregate"),"fact":fact}))
    };
    let first = initialize(&fact);
    let second = initialize(&other);
    assert_eq!(first["status"], "ok");
    assert_eq!(second["status"], "ok");
    assert_eq!(f.run("initialize-selection",&json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"inspect-stdin catalog"),"registrations":[first["data"]["registration"],second["data"]["registration"]]}))["status"],"ok");
    (identity, fact, other)
}

#[test]
fn actual_inspect_stdin_local_heads_tombstones_empty_and_opaque_are_file_compatible() {
    let f = Fixture::new();
    let fact = f.fact();
    assert_eq!(
        f.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact: fact.clone()
            }
        )["status"],
        "ok"
    );
    let input = serde_json::to_vec(&fact.key).unwrap();
    let initial = f.stdin_inspect("local-value", &input);
    assert_eq!(initial, f.run("inspect", &fact.key));
    assert_eq!(initial["status"], "ok");
    assert_eq!(initial["data"][0]["payload"]["value"], fact.value);
    assert_eq!(
        initial["data"][0]["record_id"],
        fact.key.record_id().unwrap().to_string()
    );
    let mut absent = fact.key.clone();
    absent.fact = Uuid::new_v4();
    let empty = f.stdin_inspect("local-empty", &serde_json::to_vec(&absent).unwrap());
    assert_eq!(empty["status"], "ok");
    assert_eq!(empty["data"], json!([]));
    let held = Store::open(f.paths()).unwrap();
    assert_eq!(f.stdin_inspect("busy", &input)["status"], "busy");
    drop(held);
    let original: storage::Envelope = serde_json::from_value(initial["data"][0].clone()).unwrap();
    let store = Store::open(f.paths()).unwrap();
    inspection_fork(&store, &original, original.payload.clone(), false);
    inspection_fork(&store, &original, Value::Null, true);
    drop(store);
    let conflict = f.stdin_inspect("local-conflict", &input);
    assert_eq!(conflict, f.run("inspect", &fact.key));
    assert_eq!(conflict["status"], "ok");
    assert_eq!(conflict["data"].as_array().unwrap().len(), 2);
    assert!(conflict["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|head| head["kind"] == "tombstone" && head["payload"].is_null()));
    let store = Store::open(f.paths()).unwrap();
    inspection_fork(
        &store,
        &original,
        json!({"unsupported_future_payload":true}),
        false,
    );
    drop(store);
    let opaque = f.stdin_inspect("local-opaque", &input);
    assert_eq!(opaque, f.run("inspect", &fact.key));
    assert_eq!(opaque["data"].as_array().unwrap().len(), 3);
    assert!(opaque["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|head| head["payload"]["unsupported_future_payload"] == true));
    let expected = opaque["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|head| Uuid::parse_str(head["revision_id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        f.run(
            "forget",
            &Forget {
                operation: Uuid::new_v4(),
                expected,
                key: fact.key.clone()
            }
        )["status"],
        "ok"
    );
    let tombstone = f.stdin_inspect("local-tombstone", &input);
    assert_eq!(tombstone, f.run("inspect", &fact.key));
    assert_eq!(tombstone["data"].as_array().unwrap().len(), 1);
    assert_eq!(tombstone["data"][0]["kind"], "tombstone");
    assert!(tombstone["data"][0]["payload"].is_null());
    let missing = Fixture::new();
    let key = missing.fact().key;
    assert_eq!(
        missing.stdin_inspect("unavailable", &serde_json::to_vec(&key).unwrap())["status"],
        "unavailable"
    );
    assert!(!missing.paths().data.exists());
}

#[test]
fn actual_inspect_stdin_strict_bounds_and_file_global_compatibility_precede_store() {
    let f = Fixture::new();
    let key = f.fact().key;
    let mut bytes = serde_json::to_vec(&key).unwrap();
    bytes.resize(storage::MAX_RECORD, b' ');
    assert_eq!(f.stdin_inspect("", &bytes)["status"], "unavailable");
    bytes.push(b' ');
    assert_eq!(f.stdin_inspect("", &bytes)["status"], "invalid-input");
    for bytes in [b"{truncated".as_slice(), b"\xff".as_slice()] {
        assert_eq!(f.stdin_inspect("", bytes)["status"], "invalid-input");
    }
    let extra = json!({"profile":f.profile,"scope":{"kind":"global","unknown":"PRIVATE_SCOPE_SENTINEL"},"fact":key.fact});
    assert_eq!(f.run("inspect", &extra)["status"], "unavailable");
    assert_eq!(
        f.stdin_inspect("invalid", &serde_json::to_vec(&extra).unwrap())["status"],
        "invalid-input"
    );
    for value in [
        json!({"profile":Uuid::new_v4(),"scope":{"kind":"global"},"fact":key.fact}),
        json!({"profile":f.profile,"scope":{"kind":"document","document":Uuid::nil()},"fact":key.fact}),
    ] {
        assert_eq!(
            f.stdin_inspect("", &serde_json::to_vec(&value).unwrap())["status"],
            "invalid-input"
        );
    }
    assert!(
        !f.paths().data.exists() && !f.paths().cache.exists() && !f.paths().credentials.exists()
    );
}

#[test]
fn actual_inspect_stdin_selected_value_empty_tombstone_and_whole_view_refusals() {
    let f = Fixture::new();
    let (identity, fact, _) = inspection_selected_pair(&f);
    let input = serde_json::to_vec(&fact.key).unwrap();
    let value = f.stdin_inspect("selected-value", &input);
    assert_eq!(value, f.run("inspect", &fact.key));
    assert_eq!(value["status"], "ok");
    let mut absent = fact.key.clone();
    absent.fact = Uuid::new_v4();
    let empty = f.stdin_inspect("selected-empty", &serde_json::to_vec(&absent).unwrap());
    assert_eq!(empty["status"], "ok");
    assert_eq!(empty["data"], json!([]));
    let expected = value["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|head| Uuid::parse_str(head["revision_id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        f.run(
            "forget",
            &Forget {
                operation: Uuid::new_v4(),
                expected,
                key: fact.key.clone()
            }
        )["status"],
        "ok"
    );
    let tombstone = f.stdin_inspect("selected-tombstone", &input);
    assert_eq!(tombstone, f.run("inspect", &fact.key));
    assert_eq!(tombstone["data"][0]["kind"], "tombstone");
    assert!(tombstone["data"][0]["payload"].is_null());
    // Explicit generic membership can be valid custody but invalid domain projection.
    let store = Store::open(f.paths()).unwrap();
    let scope = identity
        .aggregate_scope(&catalog::Owner::GlobalFact {
            key: fact.key.clone(),
        })
        .unwrap();
    let snapshot = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    let original = snapshot
        .selected_records
        .iter()
        .find(|head| head.kind == storage::Kind::Value)
        .unwrap();
    let mut fork = original.clone();
    fork.parents = BTreeSet::from([original.revision_id]);
    fork.revision_id = Uuid::new_v4();
    fork.operation_id = Uuid::new_v4();
    let bytes = serde_json::to_vec(&fork).unwrap();
    let hash = storage::digest(&bytes);
    let mut manifest = snapshot.transaction.selected.clone();
    manifest.transaction_id = Uuid::new_v4();
    manifest.records.push(storage::ObjectRef {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
    });
    manifest
        .record_namespaces
        .insert(hash.clone(), storage::Namespace::SubjectMemory);
    let refused_publication = store
        .activate_selected(
            &snapshot.token,
            storage::selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: snapshot.transaction.accepted_base_sha256.clone(),
                selected: manifest,
                retained: snapshot.transaction.retained.clone(),
            },
            BTreeMap::from([(hash, bytes)]),
        )
        .err()
        .expect("conflicting membership must refuse");
    assert_eq!(
        refused_publication.to_string(),
        "conflicted selected record membership"
    );
    assert_eq!(
        store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token,
        snapshot.token
    );
    drop(store);
    let unchanged = f.stdin_inspect("selected-after-refused-conflict", &input);
    assert_eq!(unchanged, f.run("inspect", &fact.key));
    assert_eq!(unchanged, tombstone);
    // Independent owned fixture: only the OTHER registered aggregate is invalid.
    let f = Fixture::new();
    let (identity, fact, other) = inspection_selected_pair(&f);
    let input = serde_json::to_vec(&fact.key).unwrap();
    let store = Store::open(f.paths()).unwrap();
    let scope = identity
        .aggregate_scope(&catalog::Owner::GlobalFact {
            key: other.key.clone(),
        })
        .unwrap();
    let snapshot = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
    let mut bad = snapshot.selected_records[0].clone();
    bad.parents = BTreeSet::from([bad.revision_id]);
    bad.revision_id = Uuid::new_v4();
    bad.operation_id = Uuid::new_v4();
    bad.payload = json!({"unsupported_future_payload":true});
    store
        .commit_selected(&snapshot.token, Uuid::new_v4(), vec![bad], BTreeMap::new())
        .unwrap();
    drop(store);
    let refused = f.stdin_inspect("selected-unrelated-refusal", &input);
    assert_eq!(refused, f.run("inspect", &fact.key));
    assert_eq!(refused["status"], "unavailable");
    assert!(refused["data"].is_null());
}

#[test]
fn actual_manager_query_report_contract() {
    let query = json!({"document":null,"text":"gravity","now_ms":100,"current_claims":[],"max_matches":8,"max_context_bytes":8192});
    let local = Fixture::new();
    let fact = local.fact();
    assert_eq!(
        local.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact
            }
        )["status"],
        "ok"
    );
    let populated = local.stdin_query(&serde_json::to_vec(&query).unwrap());
    assert_eq!(
        populated["data"]["context"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut empty_query = query.clone();
    empty_query["text"] = json!("unmatchedword");
    let empty = local.stdin_query(&serde_json::to_vec(&empty_query).unwrap());
    assert_eq!(empty["data"]["query_status"], "complete-empty");
    let held = Store::open(local.paths()).unwrap();
    let busy = local.stdin_query(&serde_json::to_vec(&query).unwrap());
    assert_eq!(busy["status"], "busy");
    drop(held);
    let missing = Fixture::new();
    let unavailable = missing.stdin_query(&serde_json::to_vec(&query).unwrap());
    assert_eq!(unavailable["status"], "unavailable");
    let mut invalid_query = query.clone();
    invalid_query["text"] = json!("");
    let invalid = missing.stdin_query(&serde_json::to_vec(&invalid_query).unwrap());
    assert_eq!(invalid["status"], "invalid-input");
    assert!(!missing.paths().data.exists());
    let selected = Fixture::new();
    selected_config(&selected);
    let initialized = selected.run("initialize-aggregate", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"actual compatibility aggregate"),"fact":selected.fact()}));
    assert_eq!(initialized["status"], "ok");
    assert_eq!(selected.run("initialize-selection", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"actual compatibility catalog"),"registrations":[initialized["data"]["registration"]]}))["status"], "ok");
    let selected_report = selected.stdin_query(&serde_json::to_vec(&query).unwrap());
    assert_eq!(
        selected_report["data"]["context"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let reports = json!({"local":populated,"selected":selected_report,"empty":empty,"busy":busy,"unavailable":unavailable,"invalid":invalid});
    for report in reports.as_object().unwrap().values() {
        assert_eq!(report["report_version"], 1);
        assert!(serde_json::to_vec(report).unwrap().len() < 128 * 1024);
    }
    // Optional evidence export for an independently pinned consumer parser.
    if let Some(path) = std::env::var_os("HOST_MEMORY_REPORT_PATH") {
        let path = PathBuf::from(path);
        assert!(path.is_absolute());
        fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
    }
}
impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: std::env::temp_dir().join(format!("buddy-memory-cli-{}", Uuid::new_v4())),
            profile: Uuid::new_v4(),
        };
        fs::create_dir_all(&fixture.root).unwrap();
        fs::write(fixture.root.join("config.json"), serde_json::to_vec(&json!({"paths":fixture.paths(),"memory":{"profile":fixture.profile,"enabled":false}})).unwrap()).unwrap();
        fixture
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.root.join("data"),
            cache: self.root.join("cache"),
            credentials: self.root.join("credentials"),
        }
    }
    fn fact(&self) -> Fact {
        Fact {
            key: Key {
                profile: self.profile,
                scope: Scope::Global,
                fact: Uuid::new_v4(),
            },
            claim: "physics units".into(),
            value: "Use SI gravity units.\nα and β stay Unicode.".into(),
            subject: "physics".into(),
            tags: vec![],
            provenance: Provenance {
                basis: Basis::UserConfirmed,
                description: "Explicit host fixture assertion".into(),
                conversation: None,
                turn: None,
                source: None,
            },
            confidence_basis_points: 8000,
            observed_ms: 10,
            expires_ms: None,
        }
    }
    fn command(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .current_dir(&self.root)
            .args(["--memory-operation", operation, "--memory-input"])
            .arg(self.root.join("input.json"))
            .env("REMARKABLE_BUDDIES_CONFIG", self.root.join("config.json"))
            .env("OPENAI_API_KEY", "private-provider-sentinel")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("RUST_LOG", "trace")
            .env("READER_BUDDY_DEBUG_DUMP", "invalid-unused-setting");
        command
    }
    fn stdin_query(&self, input: &[u8]) -> Value {
        let command = self.command("query");
        let args: Vec<_> = command.get_args().map(|a| a.to_os_string()).collect();
        let mut replacement = Command::new(command.get_program());
        replacement
            .current_dir(&self.root)
            .args(&args[..args.len() - 1])
            .arg("-");
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                replacement.env(key, value);
            }
        }
        let mut child = replacement
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut writer = child.stdin.take().unwrap();
        writer.write_all(input).unwrap();
        drop(writer); // EOF is part of the explicit stdin contract.
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() <= maintenance::MAX_RESPONSE);
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["report_version"], 1);
        assert_eq!(report["operation"], "query");
        report
    }
    fn run(&self, operation: &str, input: &impl serde::Serialize) -> Value {
        fs::write(
            self.root.join("input.json"),
            serde_json::to_vec(input).unwrap(),
        )
        .unwrap();
        let output = self.command(operation).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["report_version"], 1);
        report
    }
    fn stdin_inspect(&self, label: &str, input: &[u8]) -> Value {
        let command = self.command("inspect");
        let args: Vec<_> = command.get_args().map(|a| a.to_os_string()).collect();
        let mut replacement = Command::new(command.get_program());
        replacement
            .current_dir(&self.root)
            .args(&args[..args.len() - 1])
            .arg("-");
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                replacement.env(key, value);
            }
        }
        let mut child = replacement
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take((maintenance::MAX_RESPONSE + 1) as u64)
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        });
        let err_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.take(4096).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let bytes = input.to_vec();
        let sender = std::thread::spawn(move || pipe.write_all(&bytes));
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                let _ = sender.join();
                let _ = out_reader.join();
                let _ = err_reader.join();
                panic!("inspection exceeded caller deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Err(error) = sender.join().unwrap() {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        let status = child.wait().unwrap();
        let stdout = out_reader.join().unwrap();
        let stderr = err_reader.join().unwrap();
        assert!(status.success());
        assert!(stderr.is_empty());
        assert!(stdout.len() <= maintenance::MAX_RESPONSE);
        assert!(stdout.ends_with(b"\n"));
        let report: Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(report["report_version"], 1);
        assert_eq!(report["operation"], "inspect");
        if !label.is_empty() {
            if let Some(directory) = std::env::var_os("HOST_MEMORY_INSPECTION_REPORT_DIR") {
                let directory = PathBuf::from(directory);
                assert!(directory.is_absolute());
                fs::create_dir_all(&directory).unwrap();
                assert!(label.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
                fs::write(directory.join(format!("{label}.json")), &stdout).unwrap();
                fs::write(directory.join(format!("{label}.request.json")), input).unwrap();
            }
        }
        report
    }
    fn stdin_prepared(&self, operation: &str, label: &str, input: &[u8]) -> Value {
        let command = self.command(operation);
        let args: Vec<_> = command.get_args().map(|a| a.to_os_string()).collect();
        let mut replacement = Command::new(command.get_program());
        replacement
            .current_dir(&self.root)
            .args(&args[..args.len() - 1])
            .arg("-");
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                replacement.env(key, value);
            }
        }
        let mut child = replacement
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take((maintenance::MAX_RESPONSE + 1) as u64)
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        });
        let err_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.take(4096).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let bytes = input.to_vec();
        let sender = std::thread::spawn(move || pipe.write_all(&bytes));
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                let _ = sender.join();
                let _ = out_reader.join();
                let _ = err_reader.join();
                panic!("inspection exceeded caller deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Err(error) = sender.join().unwrap() {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        let status = child.wait().unwrap();
        let stdout = out_reader.join().unwrap();
        let stderr = err_reader.join().unwrap();
        assert!(status.success());
        assert!(stderr.is_empty());
        assert!(stdout.len() <= maintenance::MAX_RESPONSE);
        assert!(stdout.ends_with(b"\n"));
        let report: Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(report["report_version"], 1);
        assert_eq!(report["operation"], operation);
        if !label.is_empty() {
            if let Some(directory) = std::env::var_os("HOST_MEMORY_PREPARED_REPORT_DIR") {
                let directory = PathBuf::from(directory);
                assert!(directory.is_absolute());
                fs::create_dir_all(&directory).unwrap();
                assert!(label.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
                fs::write(directory.join(format!("{label}.json")), &stdout).unwrap();
                fs::write(directory.join(format!("{label}.request.json")), input).unwrap();
            }
        }
        report
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn selected_config_only(fixture: &Fixture) -> catalog::Identity {
    let identity = catalog::Identity {
        group: Uuid::new_v4(),
        profile: fixture.profile,
        binding_sha256: storage::digest(b"explicit host fixture binding, not a cloud account"),
    };
    fs::write(fixture.root.join("config.json"), serde_json::to_vec(&json!({
        "paths": fixture.paths(), "memory": {"enabled":false,"profile":fixture.profile,"selection":identity}
    })).unwrap()).unwrap();
    identity
}
fn selected_config(fixture: &Fixture) -> catalog::Identity {
    let identity = selected_config_only(fixture);
    let store = Store::open(fixture.paths()).unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            storage::sync::mode::SyncModeChoice::SharedGroup {
                group: identity.group,
                binding_sha256: identity.binding_sha256.clone(),
                root_id: "host-memory-fixture".into(),
            },
        )
        .unwrap();
    drop(store);
    identity
}

#[test]
fn actual_selected_cli_explicit_empty_catalog_and_default_local_setup_refusal() {
    let local = Fixture::new();
    let input = json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"local setup"),"registrations":[]});
    assert_eq!(
        local.run("initialize-selection", &input)["status"],
        "unsupported"
    );
    assert!(!local.paths().data.exists());
    let fixture = Fixture::new();
    selected_config(&fixture);
    let first = fixture.run("initialize-selection", &input);
    assert_eq!(first["status"], "ok");
    assert_eq!(first["data"]["acknowledgment"], "published");
    let retry = fixture.run("initialize-selection", &input);
    assert_eq!(retry["data"]["transaction"], first["data"]["transaction"]);
    assert_eq!(retry["data"]["acknowledgment"], "replayed");
    let query = fixture.run("query", &Query::new("gravity".into(), 100));
    assert_eq!(query["data"]["query_status"], "complete-empty");
    let missing = fixture.fact();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: Default::default(),
                fact: missing
            }
        )["status"],
        "unavailable"
    );
}

#[test]
fn actual_selected_cli_orphans_registration_adoption_retry_and_forgetting_preserve_other_keys() {
    let fixture = Fixture::new();
    selected_config(&fixture);
    let first = fixture.fact();
    let create = json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"accepted host descriptor"),"fact":first});
    let initialized = fixture.run("initialize-aggregate", &create);
    assert_eq!(initialized["status"], "ok");
    assert_eq!(
        fixture.run("query", &Query::new("gravity".into(), 100))["status"],
        "unavailable"
    );
    assert_eq!(
        fixture.run("initialize-aggregate", &create)["data"]["acknowledgment"],
        "replayed"
    );
    let root = json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"catalog descriptor"),
        "registrations":[initialized["data"]["registration"].clone()]});
    assert_eq!(fixture.run("initialize-selection", &root)["status"], "ok");
    let mut second = fixture.fact();
    second.claim = "gravity convention".into();
    second.value = "Use kelvin in gravity calculations".into();
    let orphan = fixture.run("initialize-aggregate", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"second aggregate"),"fact":second}));
    assert_eq!(orphan["status"], "ok");
    assert_eq!(
        fixture.run("query", &Query::new("gravity".into(), 100))["data"]["context"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let register =
        json!({"operation":Uuid::new_v4(),"registration":orphan["data"]["registration"].clone()});
    let published = fixture.run("register-selection", &register);
    assert_eq!(published["data"]["acknowledgment"], "published");
    let retry = fixture.run("register-selection", &register);
    assert_eq!(retry["data"]["acknowledgment"], "replayed");
    assert_eq!(
        retry["data"]["transaction"],
        published["data"]["transaction"]
    );
    let adopted = fixture.run(
        "register-selection",
        &json!({"operation":Uuid::new_v4(),"registration":orphan["data"]["registration"].clone()}),
    );
    assert_eq!(adopted["data"]["acknowledgment"], "adopted");
    assert!(adopted["data"]["transaction"].is_null());
    let inspected = fixture.run("inspect", &first.key);
    let parents: BTreeSet<_> = inspected["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Uuid::parse_str(e["revision_id"].as_str().unwrap()).unwrap())
        .collect();
    let mut correction = first.clone();
    correction.value = "Corrected SI gravity units".into();
    let update = Mutation {
        operation: Uuid::new_v4(),
        expected: parents,
        fact: correction,
    };
    let receipt = fixture.run("put", &update);
    assert_eq!(receipt["status"], "ok");
    let current = fixture.run("inspect", &first.key);
    let parents = current["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Uuid::parse_str(e["revision_id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        fixture.run(
            "forget",
            &Forget {
                operation: Uuid::new_v4(),
                expected: parents,
                key: first.key.clone()
            }
        )["status"],
        "ok"
    );
    assert_eq!(
        fixture.run("put", &update)["data"]["transaction"],
        receipt["data"]["transaction"]
    );
    let remaining = fixture.run("query", &Query::new("gravity".into(), 100));
    assert_eq!(
        remaining["data"]["context"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        remaining["data"]["context"]["facts"][0]["fact"]["key"]["fact"],
        json!(second.key.fact)
    );
    let mut collided = update.clone();
    collided.fact.value = "Changed replay payload".into();
    assert_eq!(fixture.run("put", &collided)["status"], "unavailable");
    assert_eq!(
        fixture.run("query", &Query::new("gravity".into(), 100))["data"]["context"],
        remaining["data"]["context"]
    );
}

#[test]
fn actual_selected_cli_invalid_setup_refuses_before_store_creation() {
    let fixture = Fixture::new();
    selected_config_only(&fixture);
    let bad =
        json!({"operation":Uuid::nil(),"accepted_base_sha256":"NOT-A-DIGEST","registrations":[]});
    assert_eq!(
        fixture.run("initialize-selection", &bad)["status"],
        "invalid-input"
    );
    assert!(!fixture.paths().data.exists());
    assert!(!fixture.paths().cache.exists());
    let mut fact = fixture.fact();
    fact.key.profile = Uuid::new_v4();
    assert_eq!(fixture.run("initialize-aggregate", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"base"),"fact":fact}))["status"], "invalid-input");
    assert!(!fixture.paths().data.exists());
}

#[test]
fn actual_cli_put_query_inspect_cas_forget_and_exact_retry_are_offline() {
    let fixture = Fixture::new();
    let fact = fixture.fact();
    let mutation = Mutation {
        operation: Uuid::new_v4(),
        expected: BTreeSet::new(),
        fact: fact.clone(),
    };
    let first = fixture.run("put", &mutation);
    assert_eq!(first["status"], "ok");
    assert_eq!(fixture.run("put", &mutation)["data"], first["data"]);
    let queried = fixture.run("query", &Query::new("physics gravity units".into(), 100));
    assert_eq!(queried["status"], "ok");
    assert_eq!(
        queried["data"]["context"]["facts"][0]["fact"]["value"],
        fact.value
    );
    let inspection = fixture.run("inspect", &fact.key);
    let revision: Uuid =
        serde_json::from_value(inspection["data"][0]["revision_id"].clone()).unwrap();
    assert_eq!(inspection["status"], "ok");
    let stale = Mutation {
        operation: Uuid::new_v4(),
        ..mutation.clone()
    };
    let conflict = fixture.run("put", &stale);
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["data"]["current_heads"], json!([revision]));
    let forgetting = Forget {
        operation: Uuid::new_v4(),
        expected: BTreeSet::from([revision]),
        key: fact.key.clone(),
    };
    let forgotten = fixture.run("forget", &forgetting);
    assert_eq!(forgotten["status"], "ok");
    assert_eq!(
        fixture.run("forget", &forgetting)["data"],
        forgotten["data"]
    );
    let query = fixture.run("query", &Query::new("physics gravity units".into(), 100));
    assert_eq!(query["data"]["query_status"], "complete-empty");
    assert_eq!(
        fixture.run("inspect", &fact.key)["data"][0]["kind"],
        "tombstone"
    );
    assert!(!fs::read_to_string(fixture.root.join("config.json"))
        .unwrap()
        .contains("private-provider"));
}

#[test]
fn missing_and_partial_state_never_initialize_from_read_forget_or_put() {
    let fixture = Fixture::new();
    let fact = fixture.fact();
    assert_eq!(fixture.run("inspect", &fact.key)["status"], "unavailable");
    assert_eq!(
        fixture.run("query", &Query::new("physics".into(), 100))["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.exists());
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::from([Uuid::new_v4()]),
                fact: fact.clone()
            }
        )["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.exists());
    let forget = Forget {
        operation: Uuid::new_v4(),
        expected: BTreeSet::from([Uuid::new_v4()]),
        key: fact.key.clone(),
    };
    assert_eq!(fixture.run("forget", &forget)["status"], "unavailable");
    assert!(!fixture.paths().data.exists());
    fs::create_dir_all(&fixture.paths().data).unwrap();
    fs::write(
        fixture.paths().data.join("unowned-sentinel"),
        b"private original",
    )
    .unwrap();
    let put = Mutation {
        operation: Uuid::new_v4(),
        expected: BTreeSet::new(),
        fact,
    };
    assert_eq!(fixture.run("put", &put)["status"], "unavailable");
    assert!(!fixture.paths().data.join("identity.json").exists());
    assert_eq!(
        fs::read(fixture.paths().data.join("unowned-sentinel")).unwrap(),
        b"private original"
    );
}

#[test]
fn pure_invalid_input_and_profile_checks_do_not_create_store_or_echo_private_values() {
    let fixture = Fixture::new();
    let mut fact = fixture.fact();
    fact.key.profile = Uuid::new_v4();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::nil(),
                expected: BTreeSet::new(),
                fact: fixture.fact()
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fixture.run(
            "forget",
            &Forget {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                key: fixture.fact().key
            }
        )["status"],
        "invalid-input"
    );
    for bytes in [
        b"private parser value".to_vec(),
        vec![b'x'; storage::MAX_RECORD + 1],
    ] {
        fs::write(fixture.root.join("input.json"), bytes).unwrap();
        let output = fixture.command("put").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains("private parser value"));
        assert!(!stdout.contains("private-provider"));
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).unwrap()["status"],
            "invalid-input"
        );
    }
    assert!(!fixture.paths().data.exists());
}

#[test]
fn busy_store_returns_report_without_service_stop_or_file_mutation() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    let identity = fs::read(fixture.paths().data.join("identity.json")).unwrap();
    let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    let report = fixture.run(
        "put",
        &Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: fixture.fact(),
        },
    );
    assert_eq!(report["status"], "busy");
    assert!(store.manifests().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.paths().data.join("identity.json")).unwrap(),
        identity
    );
    assert_eq!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        current
    );
}

#[test]
fn missing_marker_in_partly_owned_store_refuses_put_without_recovery() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    drop(store);
    fs::remove_file(fixture.paths().data.join("CURRENT")).unwrap();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact: fixture.fact()
            }
        )["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.join("CURRENT").exists());
}

#[test]
fn poison_dotenv_and_execution_overrides_cannot_enter_provider_or_runtime() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join(".env"),
        format!(
            "REMARKABLE_BUDDIES_CONFIG={}\nOPENAI_API_KEY=private-dotenv\n",
            fixture.root.join("config.json").display()
        ),
    )
    .unwrap();
    fs::write(
        fixture.root.join("input.json"),
        serde_json::to_vec(&Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: fixture.fact(),
        })
        .unwrap(),
    )
    .unwrap();
    let output = fixture
        .command("put")
        .env_remove("REMARKABLE_BUDDIES_CONFIG")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.exists());
    let output = fixture
        .command("put")
        .args(["--api-key", "private-cli-key"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-cli-key"));
    assert!(!fixture.paths().data.exists());
}

#[test]
fn selected_store_is_explicitly_unsupported_even_with_sync_disabled() {
    use storage::selection::{SelectionChange, SelectionScope};
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let memory = LocalMemory::new_local(store.clone(), fixture.profile).unwrap();
    let fact = fixture.fact();
    memory
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: fact.clone(),
        })
        .unwrap();
    drop(memory);
    let mut selected = store.manifests().unwrap().remove(0);
    selected.scope = storage::Scope::SelectedRecords;
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: storage::digest(b"fixture-scope"),
        binding_sha256: storage::digest(b"fixture-binding"),
    };
    store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"fixture-base"),
                selected,
                retained: vec![],
            },
            Default::default(),
        )
        .unwrap();
    drop(store);
    assert_eq!(fixture.run("inspect", &fact.key)["status"], "unsupported");
}

#[test]
fn partial_withheld_query_and_unavailable_recovery_never_become_healthy_empty() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let memory = LocalMemory::new_local(store.clone(), fixture.profile).unwrap();
    let first = fixture.fact();
    let mut second = first.clone();
    second.key.fact = Uuid::new_v4();
    second.value = "Conflicting cgs units".into();
    for value in [first.clone(), second] {
        memory
            .put(Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact: value,
            })
            .unwrap();
    }
    drop(memory);
    drop(store);
    let report = fixture.run("query", &Query::new("physics units".into(), 100));
    assert_eq!(report["status"], "partial");
    assert_eq!(report["incomplete"], true);
    assert!(report["data"]["context"]["facts"]
        .as_array()
        .unwrap()
        .is_empty());
    let generations = fs::read_dir(fixture.paths().data.join("generations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let commit = fs::read_dir(generations.join("commits"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(commit, b"private corrupt commit contents").unwrap();
    let report = fixture.run("query", &Query::new("physics units".into(), 100));
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["incomplete"], true);
}

#[test]
fn entire_response_cap_includes_newline_and_never_returns_partial_refresh_data() {
    use remarkable_reader_buddy::memory::maintenance::{
        Operation, Report, ReportStatus, MAX_RESPONSE,
    };
    let mut report = Report::new(Operation::Inspect, ReportStatus::Ok);
    report.data = Some(Value::String("private-value".repeat(MAX_RESPONSE / 10)));
    let bytes = report.encoded().unwrap();
    assert!(bytes.len() <= MAX_RESPONSE);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let limited: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(limited["status"], "unavailable");
    assert_eq!(limited["incomplete"], true);
    assert!(limited["data"].is_null());
}

#[test]
fn self_parent_put_and_forget_are_invalid_before_store_staging_cleanup() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    drop(store);
    let stage = fixture
        .paths()
        .data
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&stage, b"private interrupted staging evidence").unwrap();
    let operation = Uuid::new_v4();
    let revision = Uuid::new_v5(&operation, b"remarkable-buddies:subject-memory:revision:v1");
    let fact = fixture.fact();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation,
                expected: BTreeSet::from([revision]),
                fact: fact.clone()
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"private interrupted staging evidence"
    );
    assert_eq!(
        fixture.run(
            "forget",
            &Forget {
                operation,
                expected: BTreeSet::from([revision]),
                key: fact.key
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"private interrupted staging evidence"
    );
}

#[cfg(unix)]
#[test]
fn fifo_lock_is_refused_before_a_blocking_open_in_actual_cli() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    drop(store);
    let lock = fixture.paths().data.join("store.lock");
    fs::remove_file(&lock).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&lock)
        .status()
        .unwrap()
        .success());
    assert_eq!(
        fixture.run("query", &Query::new("physics units".into(), 100))["status"],
        "unavailable"
    );
}

#[test]
fn actual_cli_stdin_query_matches_file_for_local_and_selected_memory() {
    for selected in [false, true] {
        let fixture = Fixture::new();
        let fact = fixture.fact();
        if selected {
            selected_config(&fixture);
            let aggregate = fixture.run("initialize-aggregate", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"stdin aggregate"),"fact":fact}));
            assert_eq!(aggregate["status"], "ok");
            let selection = fixture.run("initialize-selection", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"stdin catalog"),"registrations":[aggregate["data"]["registration"].clone()]}));
            assert_eq!(selection["status"], "ok");
        } else {
            assert_eq!(
                fixture.run(
                    "put",
                    &Mutation {
                        operation: Uuid::new_v4(),
                        expected: BTreeSet::new(),
                        fact: fact.clone()
                    }
                )["status"],
                "ok"
            );
        }
        let query = Query::new("physics gravity units\r\nα".into(), 100);
        let file_report = fixture.run("query", &query);
        let bytes = serde_json::to_vec(&query).unwrap();
        let stdin_report = fixture.stdin_query(&bytes);
        assert_eq!(stdin_report, file_report);
        assert_eq!(stdin_report["status"], "ok");
        assert_eq!(
            stdin_report["data"]["context"]["facts"][0]["fact"]["value"],
            fact.value
        );
        assert_eq!(
            stdin_report["data"]["context"]["facts"][0]["fact"]["provenance"]["description"],
            fact.provenance.description
        );
        assert!(serde_json::to_vec(&stdin_report).unwrap().len() < 128 * 1024);
        if !selected {
            let mut padded = bytes;
            padded.resize(storage::MAX_RECORD, b' ');
            assert_eq!(fixture.stdin_query(&padded), file_report);
        }
    }
}

#[test]
fn actual_cli_stdin_invalid_query_refuses_before_store_and_does_not_echo() {
    let query = Query::new("PRIVATE_STDIN_CANARY".into(), 100);
    let valid = serde_json::to_vec(&query).unwrap();
    let mut oversized = valid.clone();
    oversized.resize(storage::MAX_RECORD + 1, b' ');
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b" {}");
    let mut bad_query = serde_json::to_value(&query).unwrap();
    bad_query["max_matches"] = json!(0);
    for bytes in [
        b"PRIVATE_STDIN_CANARY".to_vec(),
        vec![0xff, 0xfe],
        oversized,
        trailing,
        serde_json::to_vec(&bad_query).unwrap(),
    ] {
        let fixture = Fixture::new();
        let report = fixture.stdin_query(&bytes);
        assert_eq!(report["status"], "invalid-input");
        assert!(report["data"].is_null());
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("PRIVATE_STDIN_CANARY"));
        assert!(!fixture.paths().data.exists());
        assert!(!fixture.paths().cache.exists());
        assert!(!fixture.paths().credentials.exists());
    }
    let fixture = Fixture::new();
    assert_eq!(fixture.stdin_query(&valid)["status"], "unavailable");
    assert!(!fixture.paths().data.exists());
}

fn prepare_forget(f: &Fixture, fact: &Fact, label: &str) -> Value {
    let report = f.stdin_prepared(
        "prepare-forget",
        label,
        &serde_json::to_vec(
            &json!({"prepared_version":1,"operation":Uuid::new_v4(),"key":fact.key}),
        )
        .unwrap(),
    );
    assert_eq!(report["status"], "ok", "{report}");
    assert_eq!(report["incomplete"], false);
    assert_eq!(
        report["data"]["preview"]["payload"],
        serde_json::to_value(fact).unwrap()
    );
    report["data"]["intent"].clone()
}
fn original_intent_call(f: &Fixture, operation: &str, label: &str, intent: &Value) -> Value {
    f.stdin_prepared(
        operation,
        label,
        &serde_json::to_vec(&json!({"intent":intent})).unwrap(),
    )
}
#[test]
fn actual_prepared_forget_local_original_receipt_survives_later_value_without_redeleting() {
    let f = Fixture::new();
    let mut fact = f.fact();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[],"fact":fact})
        )["status"],
        "ok"
    );
    let intent = prepare_forget(&f, &fact, "local-prepare");
    let absent = original_intent_call(&f, "forget-receipt", "local-receipt-absent", &intent);
    assert_eq!(absent["status"], "unavailable");
    assert_eq!(absent["data"]["outcome"], "unresolved");
    let ack = original_intent_call(&f, "forget", "local-confirm", &intent);
    assert_eq!(ack["status"], "ok", "{ack}");
    assert_eq!(ack["data"]["outcome"], "acknowledged");
    assert_eq!(ack["data"]["operation"], intent["operation"]);
    let old = f.run("inspect", &serde_json::to_value(&fact.key).unwrap());
    let tombstone = old["data"][0]["revision_id"].clone();
    assert_eq!(old["data"][0]["kind"], "tombstone");
    fact.value = "A later value must survive old acknowledgement".into();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[tombstone],"fact":fact})
        )["status"],
        "ok"
    );
    let store = Store::open(f.paths()).unwrap();
    let manifests = store.manifests().unwrap();
    drop(store);
    let receipt = original_intent_call(&f, "forget-receipt", "local-later-receipt", &intent);
    assert_eq!(receipt["data"], ack["data"]);
    assert_eq!(
        original_intent_call(&f, "forget", "local-exact-replay", &intent)["data"],
        ack["data"]
    );
    let mut forged = intent.clone();
    forged["expected"][0]["sha256"] = json!(storage::digest(b"forged original content"));
    assert_eq!(
        original_intent_call(&f, "forget-receipt", "local-tampered-receipt", &forged)["data"]
            ["outcome"],
        "unresolved"
    );
    assert_eq!(
        original_intent_call(&f, "forget", "local-tampered-confirm", &forged)["data"]["outcome"],
        "not-dispatched"
    );
    let now = f.run("inspect", &serde_json::to_value(&fact.key).unwrap());
    assert_eq!(now["data"][0]["payload"]["value"], fact.value);
    let store = Store::open(f.paths()).unwrap();
    assert_eq!(store.manifests().unwrap(), manifests);
}
#[test]
fn actual_prepared_forget_selected_predecessor_and_other_pins_bind_historical_ack() {
    let f = Fixture::new();
    let (_, mut fact, mut other) = inspection_selected_pair(&f);
    let intent = prepare_forget(&f, &fact, "selected-prepare");
    assert_eq!(
        original_intent_call(&f, "forget-receipt", "selected-receipt-absent", &intent)["data"]
            ["outcome"],
        "unresolved"
    );
    let ack = original_intent_call(&f, "forget", "selected-confirm", &intent);
    assert_eq!(ack["status"], "ok", "{ack}");
    let tombstone = f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["data"][0]
        ["revision_id"]
        .clone();
    fact.value = "Later selected value survives".into();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[tombstone],"fact":fact})
        )["status"],
        "ok"
    );
    assert_eq!(
        original_intent_call(&f, "forget-receipt", "selected-later-receipt", &intent)["data"],
        ack["data"]
    );
    assert_eq!(
        original_intent_call(&f, "forget", "selected-exact-replay", &intent)["data"],
        ack["data"]
    );
    let mut forged = intent.clone();
    forged["expected"][0]["sha256"] = json!(storage::digest(b"changed expected content"));
    assert_eq!(
        original_intent_call(&f, "forget-receipt", "selected-tampered-receipt", &forged)["data"]
            ["outcome"],
        "unresolved"
    );
    let mut forged = intent.clone();
    forged["binding"]["domain"]["pins"][0]["accepted_base_sha256"] =
        json!(storage::digest(b"changed accepted base"));
    assert_eq!(
        original_intent_call(&f, "forget-receipt", "selected-changed-pin", &forged)["data"]
            ["outcome"],
        "unresolved"
    );
    let old = f.run("inspect", &serde_json::to_value(&other.key).unwrap())["data"][0]
        ["revision_id"]
        .clone();
    other.value = "Unrelated admitted aggregate changed".into();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[old],"fact":other})
        )["status"],
        "ok"
    );
    assert_eq!(
        original_intent_call(&f, "forget-receipt", "selected-other-drift", &intent)["data"]
            ["outcome"],
        "unresolved"
    );
    assert_eq!(
        f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["data"][0]["payload"]["value"],
        fact.value
    );
}
#[test]
fn actual_prepared_forget_binding_reentry_strict_limits_and_no_initialization() {
    let f = Fixture::new();
    let fact = f.fact();
    let request = json!({"prepared_version":1,"operation":Uuid::new_v4(),"key":fact.key});
    let valid = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        f.stdin_prepared("prepare-forget", "missing-prepare", &valid)["status"],
        "unavailable"
    );
    assert!(!f.paths().data.exists());
    let mut malformed = request.clone();
    malformed["key"]["scope"]["extra"] = json!(true);
    assert_eq!(
        f.stdin_prepared(
            "prepare-forget",
            "invalid-prepare",
            &serde_json::to_vec(&malformed).unwrap()
        )["status"],
        "invalid-input"
    );
    let mut exact = valid.clone();
    exact.resize(storage::MAX_RECORD, b' ');
    assert_eq!(
        f.stdin_prepared("prepare-forget", "", &exact)["status"],
        "unavailable"
    );
    exact.push(b' ');
    assert_eq!(
        f.stdin_prepared("prepare-forget", "", &exact)["status"],
        "invalid-input"
    );
    assert!(!f.paths().data.exists());
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[],"fact":fact})
        )["status"],
        "ok"
    );
    let intent = prepare_forget(&f, &fact, "binding-prepare");
    let store = Store::open(f.paths()).unwrap();
    for choice in [
        storage::sync::mode::SyncModeChoice::LocalOnly,
        storage::sync::mode::SyncModeChoice::IsolatedBackup {
            binding_sha256: storage::digest(b"different mode"),
        },
        storage::sync::mode::SyncModeChoice::LocalOnly,
    ] {
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                choice,
            )
            .unwrap();
    }
    drop(store);
    assert_eq!(
        original_intent_call(&f, "forget", "mode-reentry-refusal", &intent)["data"]["outcome"],
        "not-dispatched"
    );
    let intent = prepare_forget(&f, &fact, "config-prepare");
    let path = f.root.join("config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["memory"]["enabled"] = json!(true);
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert_eq!(
        original_intent_call(&f, "forget", "config-refusal", &intent)["status"],
        "conflict"
    );
    assert_eq!(
        f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["data"][0]["kind"],
        "value"
    );
}
#[test]
fn actual_prepared_forget_same_heads_do_not_rebind_replaced_storage() {
    let f = Fixture::new();
    let fact = f.fact();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[],"fact":fact})
        )["status"],
        "ok"
    );
    let intent = prepare_forget(&f, &fact, "restore-prepare");
    let store = Store::open(f.paths()).unwrap();
    let backup = f.root.join("backup");
    store.export(&backup).unwrap();
    store.restore(&backup).unwrap();
    drop(store);
    assert_eq!(
        original_intent_call(&f, "forget", "restored-generation-refusal", &intent)["data"]
            ["outcome"],
        "not-dispatched"
    );
    assert_eq!(
        f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["data"][0]["kind"],
        "value"
    );
    for catalog in [false, true] {
        let f = Fixture::new();
        let (identity, fact, _) = inspection_selected_pair(&f);
        let intent = prepare_forget(
            &f,
            &fact,
            if catalog {
                "catalog-replace-prepare"
            } else {
                "target-replace-prepare"
            },
        );
        let store = Store::open(f.paths()).unwrap();
        let scope = if catalog {
            identity.scope().unwrap()
        } else {
            identity
                .aggregate_scope(&catalog::Owner::GlobalFact {
                    key: fact.key.clone(),
                })
                .unwrap()
        };
        let old = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
        store
            .activate_selected(
                &old.token,
                storage::selection::SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: storage::digest(
                        b"new acceptance with identical envelopes",
                    ),
                    selected: old.transaction.selected.clone(),
                    retained: old.transaction.retained.clone(),
                },
                BTreeMap::new(),
            )
            .unwrap();
        let new = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
        assert!(new.selected_records == old.selected_records);
        assert_ne!(
            new.token.aggregate_generation(),
            old.token.aggregate_generation()
        );
        drop(store);
        // Establish the new state is still valid domain data, not a manufactured invalid fixture.
        assert_eq!(
            f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["status"],
            "ok"
        );
        let refusal = original_intent_call(
            &f,
            "forget",
            if catalog {
                "catalog-replacement-refusal"
            } else {
                "target-replacement-refusal"
            },
            &intent,
        );
        assert_eq!(refusal["status"], "conflict", "{refusal}");
        assert_eq!(refusal["data"]["outcome"], "not-dispatched");
    }
}
#[test]
fn actual_prepared_forget_refuses_ambiguous_unsupported_and_busy_without_mutation() {
    for shape in ["absent", "tombstone", "conflict", "opaque", "numeric"] {
        let f = Fixture::new();
        let mut fact = f.fact();
        if shape == "numeric" {
            fact.observed_ms = 9_007_199_254_740_992;
        }
        assert_eq!(
            f.run(
                "put",
                &json!({"operation":Uuid::new_v4(),"expected":[],"fact":fact})
            )["status"],
            "ok"
        );
        let head = f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["data"][0].clone();
        if shape == "tombstone" {
            assert_eq!(f.run("forget",&json!({"operation":Uuid::new_v4(),"key":fact.key,"expected":[head["revision_id"]]}))["status"],"ok");
        }
        if shape == "conflict" || shape == "opaque" {
            let store = Store::open(f.paths()).unwrap();
            let original: storage::Envelope = serde_json::from_value(head).unwrap();
            inspection_fork(
                &store,
                &original,
                if shape == "opaque" {
                    json!({"unsupported":true})
                } else {
                    original.payload.clone()
                },
                false,
            );
            if shape == "conflict" {
                inspection_fork(&store, &original, original.payload.clone(), false);
            }
            drop(store);
        }
        let mut key = fact.key.clone();
        if shape == "absent" {
            key.fact = Uuid::new_v4();
        }
        let report = f.stdin_prepared(
            "prepare-forget",
            &format!("shape-{shape}"),
            &serde_json::to_vec(
                &json!({"prepared_version":1,"operation":Uuid::new_v4(),"key":key}),
            )
            .unwrap(),
        );
        assert_eq!(report["status"], "unsupported", "{shape}: {report}");
        assert_eq!(report["incomplete"], false);
        assert!(report["data"].is_null());
    }
    let f = Fixture::new();
    let fact = f.fact();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[],"fact":fact})
        )["status"],
        "ok"
    );
    let intent = prepare_forget(&f, &fact, "busy-prepare");
    let store = Store::open(f.paths()).unwrap();
    let report = original_intent_call(&f, "forget", "busy-confirm", &intent);
    assert_eq!(report["status"], "busy");
    assert_eq!(report["incomplete"], false);
    assert_eq!(report["data"]["outcome"], "not-dispatched");
    let report = original_intent_call(&f, "forget-receipt", "busy-receipt", &intent);
    assert_eq!(report["data"]["outcome"], "unresolved");
    drop(store);
    assert_eq!(
        f.run("inspect", &serde_json::to_value(&fact.key).unwrap())["data"][0]["kind"],
        "value"
    );
}
#[test]
fn actual_prepared_forget_ambiguous_local_receipt_is_unresolved_without_republication() {
    let f = Fixture::new();
    let fact = f.fact();
    assert_eq!(
        f.run(
            "put",
            &json!({"operation":Uuid::new_v4(),"expected":[],"fact":fact})
        )["status"],
        "ok"
    );
    let intent = prepare_forget(&f, &fact, "ambiguity-prepare");
    let ack = original_intent_call(&f, "forget", "ambiguity-confirm", &intent);
    assert_eq!(ack["status"], "ok");
    let original = Uuid::parse_str(ack["data"]["transaction"].as_str().unwrap()).unwrap();
    let store = Store::open(f.paths()).unwrap();
    let mut duplicate = store
        .manifests()
        .unwrap()
        .into_iter()
        .find(|m| m.transaction_id == original)
        .unwrap();
    duplicate.transaction_id = Uuid::new_v4();
    store.import(duplicate, BTreeMap::new()).unwrap();
    let before = store.manifests().unwrap();
    drop(store);
    let receipt = original_intent_call(&f, "forget-receipt", "ambiguous-local-receipt", &intent);
    assert_eq!(receipt["status"], "unavailable");
    assert_eq!(receipt["data"]["outcome"], "unresolved");
    let repeated = original_intent_call(&f, "forget", "ambiguous-confirm-refusal", &intent);
    assert_eq!(repeated["data"]["outcome"], "not-dispatched");
    let store = Store::open(f.paths()).unwrap();
    assert_eq!(store.manifests().unwrap(), before);
}
#[test]
fn recorded_prepared_corpus_preserves_original_bytes_and_logical_acknowledgements() {
    let corpus: Value = serde_json::from_str(include_str!(
        "fixtures/prepared-forget-77e92306-corpus.json"
    ))
    .unwrap();
    let manifest_raw = corpus["manifest_utf8"].as_str().unwrap();
    assert_eq!(
        storage::digest(manifest_raw.as_bytes()),
        corpus["manifest_sha256"]
    );
    let manifest: Value = serde_json::from_str(manifest_raw).unwrap();
    let reports = manifest["reports"].as_array().unwrap();
    let pairs = corpus["pairs"].as_array().unwrap();
    assert_eq!(reports.len(), 39);
    assert_eq!(pairs.len(), 39);
    let mut seen = BTreeSet::new();
    let mut acknowledgements = BTreeMap::new();
    let mut greek_previews = 0;
    for item in reports {
        let name = item["name"].as_str().unwrap();
        assert!(seen.insert(name));
        let pair = pairs.iter().find(|p| p["name"] == name).unwrap();
        let request = pair["request_utf8"].as_str().unwrap();
        let stdout = pair["stdout_utf8"].as_str().unwrap();
        assert!(stdout.ends_with('\n'));
        assert_eq!(storage::digest(request.as_bytes()), item["request_sha256"]);
        assert_eq!(storage::digest(stdout.as_bytes()), item["stdout_sha256"]);
        let request: Value = serde_json::from_str(request).unwrap();
        let report: Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(report["report_version"], 1);
        assert_eq!(report["operation"], item["operation"]);
        assert_eq!(report["status"], item["status"]);
        assert_eq!(report["incomplete"], item["incomplete"]);
        if report["status"] != "ok" {
            continue;
        }
        if report["operation"] == "prepare-forget" {
            let envelope: storage::Envelope =
                serde_json::from_value(report["data"]["preview"].clone()).unwrap();
            envelope.validate().unwrap();
            let fact: Fact = serde_json::from_value(envelope.payload.clone()).unwrap();
            fact.validate().unwrap();
            assert_eq!(fact.key.record_id().unwrap(), envelope.record_id);
            let intent = &report["data"]["intent"];
            assert_eq!(intent["operation"], request["operation"]);
            assert_eq!(intent["key"], request["key"]);
            assert_eq!(
                intent["expected"][0]["revision"],
                json!(envelope.revision_id)
            );
            assert_eq!(
                intent["expected"][0]["sha256"],
                storage::digest(&serde_json::to_vec(&envelope).unwrap())
            );
            if fact.value.contains('\u{03b1}') && fact.value.contains('\u{03b2}') {
                greek_previews += 1;
            }
        } else {
            assert_eq!(report["data"]["outcome"], "acknowledged");
            assert_eq!(report["data"]["operation"], request["intent"]["operation"]);
            let operation = Uuid::parse_str(report["data"]["operation"].as_str().unwrap()).unwrap();
            let transaction =
                Uuid::parse_str(report["data"]["transaction"].as_str().unwrap()).unwrap();
            assert!(!operation.is_nil() && !transaction.is_nil());
            if let Some(prior) = acknowledgements.insert(operation, transaction) {
                assert_eq!(prior, transaction);
            }
        }
    }
    assert_eq!(greek_previews, 9);
}
