//! Actual one-shot subprocesses over owned synthetic Stores; no tablet/provider.
use buddy::storage::{digest, sync::mode::SyncModeChoice, Envelope, Kind, Namespace, Store, Uuid};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;
fn record(store: &Store) -> Envelope {
    Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"text":"PRIVATE_ARCHIVE_RECORD"}),
        media_descriptors: vec![],
    }
}
fn archive(source: &Fixture) -> PathBuf {
    let store = Store::open(source.paths()).unwrap();
    store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
    let path = source.root.join("archive");
    store.export(&path).unwrap();
    path
}
fn command(target: &Fixture, operation: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command
        .env_clear()
        .current_dir(&target.root)
        .env("REMARKABLE_BUDDIES_CONFIG", target.root.join("config.json"))
        .env("OPENAI_API_KEY", "PRIVATE_ENV_SECRET")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1/PRIVATE_PROVIDER")
        .env("RUST_LOG", "trace")
        .args(["--restore-operation", operation, "--restore-input", "-"]);
    command
}
fn output(mut command: Command, input: &[u8]) -> Output {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut pipe = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || pipe.write_all(&input));
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("restore child exceeded caller deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Err(e) = writer.join().unwrap() {
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
    }
    child.wait_with_output().unwrap()
}
fn run_bytes(target: &Fixture, operation: &str, input: &[u8]) -> Value {
    let out = output(command(target, operation), input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= buddy::restore_maintenance::MAX_IO && out.stdout.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 7);
    assert_eq!(value["runtime_activation"], "unverified");
    value
}
fn run(target: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(target, operation, &serde_json::to_vec(input).unwrap())
}
fn prepare(target: &Fixture, source: &Path) -> Value {
    let p = run(
        target,
        "prepare",
        &json!({"restore_version":1,"operation":Uuid::new_v4(),"source":source}),
    );
    assert_eq!(p["outcome"], "prepared", "{p}");
    json!({"source":source,"intent":p["data"]["intent"]})
}
#[test]
fn actual_restore_additive_readback_source_config_privacy_and_archive_absent_status() {
    let source = Fixture::new();
    let path = archive(&source);
    let source_before = fs::read(path.join("backup.json")).unwrap();
    for local in [false, true] {
        let target = Fixture::new();
        let store = Store::open(target.paths()).unwrap();
        let existing = record(&store);
        store
            .commit(vec![existing.clone()], BTreeMap::new())
            .unwrap();
        if local {
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    SyncModeChoice::LocalOnly,
                )
                .unwrap();
        }
        fs::write(target.paths().cache.join("keep"), b"PRIVATE_CACHE").unwrap();
        fs::write(
            target.paths().credentials.join("keep"),
            b"PRIVATE_CREDENTIAL",
        )
        .unwrap();
        fs::write(target.root.join("original.pdf"), b"PRIVATE_NATIVE_DOCUMENT").unwrap();
        let old: Uuid =
            serde_json::from_slice(&fs::read(target.paths().data.join("CURRENT")).unwrap())
                .unwrap();
        let config = fs::read(target.root.join("config.json")).unwrap();
        let before = target.inventory();
        drop(store);
        let intent = prepare(&target, &path);
        assert_eq!(target.inventory(), before);
        assert_eq!(run(&target, "status", &intent)["outcome"], "unresolved");
        let applied = run(&target, "apply", &intent);
        assert_eq!(applied["outcome"], "restored");
        assert_eq!(applied["disk_readback_verified"], true);
        assert_eq!(applied["data"]["configuration_applied"], false);
        assert_eq!(
            run(&target, "status", &intent)["outcome"],
            "desired-generation-observed"
        );
        assert_eq!(run(&target, "apply", &intent)["status"], "conflict");
        let after = target.inventory();
        for (name, bytes) in before {
            if name.file_name().unwrap() != "CURRENT" {
                // The shared fixture represents store.lock by its length, so
                // compare the same normalized inventory on both sides.
                assert_eq!(after.get(&name), Some(&bytes));
            }
        }
        assert!(target
            .paths()
            .data
            .join("generations")
            .join(old.to_string())
            .exists());
        assert_eq!(fs::read(target.root.join("config.json")).unwrap(), config);
        assert_eq!(
            fs::read(target.root.join("original.pdf")).unwrap(),
            b"PRIVATE_NATIVE_DOCUMENT"
        );
        let store = Store::open_existing(target.paths()).unwrap();
        assert_eq!(
            store
                .heads(existing.namespace, existing.record_id)
                .unwrap()
                .len(),
            1
        );
        drop(store);
    }
    assert_eq!(fs::read(path.join("backup.json")).unwrap(), source_before);
    let target = Fixture::new();
    drop(Store::open(target.paths()).unwrap());
    let intent = prepare(&target, &path);
    assert_eq!(run(&target, "apply", &intent)["outcome"], "restored");
    assert_eq!(path.parent(), Some(source.root.as_path()));
    fs::remove_dir_all(&path).unwrap();
    assert_eq!(
        run(&target, "status", &intent)["outcome"],
        "desired-generation-observed"
    );
    assert_eq!(run(&target, "apply", &intent)["outcome"], "not-dispatched");
}
#[test]
fn actual_restore_original_history_source_config_and_target_directory_conditions() {
    let source = Fixture::new();
    let path = archive(&source);
    let target = Fixture::new();
    drop(Store::open(target.paths()).unwrap());
    let intent = prepare(&target, &path);
    let store = Store::open_existing(target.paths()).unwrap();
    store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
    drop(store);
    let before = target.inventory();
    assert_eq!(run(&target, "apply", &intent)["status"], "conflict");
    assert_eq!(target.inventory(), before);
    let intent = prepare(&target, &path);
    let root = target
        .paths()
        .data
        .join("generations")
        .join(intent["intent"]["target_generation"].as_str().unwrap());
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep"), b"retained stage").unwrap();
    let before = target.inventory();
    assert_eq!(run(&target, "apply", &intent)["status"], "conflict");
    assert_eq!(target.inventory(), before);
    let intent = prepare(&target, &path);
    let mut original = fs::read(path.join("backup.json")).unwrap();
    original.push(b' ');
    fs::write(path.join("backup.json"), &original).unwrap();
    let before = target.inventory();
    assert_eq!(run(&target, "apply", &intent)["status"], "conflict");
    assert_eq!(target.inventory(), before);
    let intent = prepare(&target, &path);
    let config_path = target.root.join("config.json");
    let mut config = fs::read(&config_path).unwrap();
    config.push(b' ');
    fs::write(&config_path, &config).unwrap();
    assert_eq!(run(&target, "apply", &intent)["status"], "conflict");
    assert_eq!(run(&target, "status", &intent)["status"], "conflict");
}
#[test]
fn actual_restore_busy_uninitialized_selected_and_nonlocal_modes_never_mutate() {
    let source = Fixture::new();
    let path = archive(&source);
    let request = json!({"restore_version":1,"operation":Uuid::new_v4(),"source":path});
    let target = Fixture::new();
    assert_eq!(run(&target, "prepare", &request)["status"], "unavailable");
    assert!(!target.paths().data.exists());
    let store = Store::open(target.paths()).unwrap();
    let before = target.inventory();
    assert_eq!(run(&target, "prepare", &request)["status"], "busy");
    assert_eq!(target.inventory(), before);
    drop(store);
    for choice in [
        SyncModeChoice::IsolatedBackup {
            binding_sha256: digest(b"binding"),
        },
        SyncModeChoice::SharedGroup {
            group: Uuid::new_v4(),
            binding_sha256: digest(b"binding"),
            root_id: "fixture-root".into(),
        },
    ] {
        let target = Fixture::new();
        let store = Store::open(target.paths()).unwrap();
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                choice,
            )
            .unwrap();
        drop(store);
        let before = target.inventory();
        assert_eq!(run(&target, "prepare", &request)["status"], "unsupported");
        assert_eq!(target.inventory(), before);
    }
    let selected = Fixture::new();
    let (store, _) = selected.selected(false);
    drop(store);
    let before = selected.inventory();
    assert_eq!(run(&selected, "prepare", &request)["status"], "unsupported");
    assert_eq!(selected.inventory(), before);
    let selected_source = Fixture::new();
    let (store, _) = selected_source.selected(false);
    let selected_path = selected_source.root.join("selected-archive");
    store.export_selected(&selected_path).unwrap();
    drop(store);
    let before = target.inventory();
    assert_eq!(
        run(
            &target,
            "prepare",
            &json!({"restore_version":1,"operation":Uuid::new_v4(),"source":selected_path})
        )["status"],
        "unsupported"
    );
    assert_eq!(target.inventory(), before);
}
#[test]
fn actual_restore_strict_shapes_nullable_presence_caps_and_cli_inventory() {
    let source = Fixture::new();
    let path = archive(&source);
    let target = Fixture::new();
    drop(Store::open(target.paths()).unwrap());
    let intent = prepare(&target, &path);
    let before = target.inventory();
    let mut missing = intent.clone();
    missing["intent"]
        .as_object_mut()
        .unwrap()
        .remove("staged_configuration_sha256");
    for raw in [
        b"[]".to_vec(),
        b"{\"source\":\"x\",\"intent\":[]}".to_vec(),
        serde_json::to_vec(&missing).unwrap(),
        vec![b' '; buddy::restore_maintenance::MAX_IO + 1],
        b"{} {}".to_vec(),
        b"{\"intent\":{},\"intent\":{}}".to_vec(),
    ] {
        assert_eq!(run_bytes(&target, "apply", &raw)["status"], "invalid-input");
        assert_eq!(target.inventory(), before);
    }
    for extra in [
        vec!["--api-key", "PRIVATE_ARGUMENT_SECRET"],
        vec!["--simulate", "scenario"],
        vec!["--configuration-operation", "inspect"],
        vec!["--basic-settings-operation", "inspect"],
        vec!["--archive-operation", "inspect", "--archive-input", "-"],
        vec!["--log-level", "trace"],
    ] {
        let mut cmd = command(&target, "prepare");
        cmd.args(extra);
        let out = output(cmd, b"{}");
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE_ARGUMENT_SECRET"));
        assert_eq!(target.inventory(), before);
    }
    let mut wrong = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    wrong.env_clear().args(["--restore-operation", "prepare"]);
    assert!(!output(wrong, b"{}").status.success());
    let mut wrong = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    wrong
        .env_clear()
        .env("REMARKABLE_BUDDIES_CONFIG", target.root.join("config.json"))
        .args(["--restore-operation", "prepare", "--restore-input"])
        .arg(target.root.join("does-not-exist"));
    let out = output(wrong, b"{}");
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    assert_eq!(target.inventory(), before);
}

#[cfg(windows)]
#[test]
fn actual_restore_windows_case_alias_inside_owned_root_is_refused() {
    let source = Fixture::new();
    drop(Store::open(source.paths()).unwrap());
    let source_store = Store::open_existing(source.paths()).unwrap();
    source_store
        .commit(vec![record(&source_store)], BTreeMap::new())
        .unwrap();
    let target = Fixture::new();
    drop(Store::open(target.paths()).unwrap());
    let inside = target.paths().data.join("inside-archive");
    source_store.export(&inside).unwrap();
    drop(source_store);
    let alias = PathBuf::from(
        inside
            .to_str()
            .unwrap()
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect::<String>(),
    );
    assert_eq!(
        fs::read(alias.join("backup.json")).unwrap(),
        fs::read(inside.join("backup.json")).unwrap()
    );
    let before = target.inventory();
    for path in [&inside, &alias] {
        let report = run(
            &target,
            "prepare",
            &json!({"restore_version":1,"operation":Uuid::new_v4(),"source":path}),
        );
        assert_eq!(report["status"], "invalid-input");
        assert_eq!(report["outcome"], "not-dispatched");
        assert_eq!(target.inventory(), before);
    }
}
