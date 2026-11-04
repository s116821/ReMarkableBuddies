use buddy::{
    archive_maintenance::{MAX_INPUT, MAX_OUTPUT},
    config::Config,
    storage::Store,
};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;

fn command(f: &Fixture) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    cmd.current_dir(&f.root)
        .env("REMARKABLE_BUDDIES_CONFIG", f.root.join("config.json"))
        .env("OPENAI_API_KEY", "PRIVATE_ENV_SENTINEL")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
        .env("RUST_LOG", "trace");
    cmd
}
fn output(mut cmd: Command, input: Option<&[u8]>) -> Output {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = cmd.spawn().unwrap();
    let writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut pipe = child.stdin.take().unwrap();
        std::thread::spawn(move || pipe.write_all(&bytes))
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            panic!("archive CLI exceeded caller deadline")
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(writer) = writer {
        if let Err(error) = writer.join().unwrap() {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe)
        }
    }
    child.wait_with_output().unwrap()
}
fn run_bytes(f: &Fixture, operation: &str, bytes: &[u8], file: bool) -> Value {
    let mut cmd = command(f);
    cmd.args(["--archive-operation", operation, "--archive-input"]);
    if file {
        fs::write(f.root.join("input.json"), bytes).unwrap();
        cmd.arg(f.root.join("input.json"));
    } else {
        cmd.arg("-");
    }
    let out = output(cmd, if file { None } else { Some(bytes) });
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= MAX_OUTPUT && out.stdout.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["activation_performed"], false);
    assert_eq!(report["report_version"], 1);
    report
}
fn run(f: &Fixture, operation: &str, path: &Path) -> Value {
    run_bytes(
        f,
        operation,
        &serde_json::to_vec(&json!({"path":path})).unwrap(),
        false,
    )
}
fn assert_no_sentinels(path: &Path) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_sentinels(&path)
        } else {
            assert!(!String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("PRIVATE_"));
        }
    }
}

#[test]
fn real_selected_export_inspection_preserves_history_media_unrelated_data_and_exclusions() {
    for partial in [false, true] {
        let f = Fixture::new();
        let (store, scope) = f.selected(partial);
        let token = store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token;
        drop(store);
        fs::write(
            f.root.join(".env"),
            b"OPENAI_API_KEY=PRIVATE_DOTENV_SENTINEL\nREADER_BUDDY_DEBUG_DUMP=poison\n",
        )
        .unwrap();
        let before = f.inventory();
        let destination = f.root.join("archive");
        let exported = run(&f, "export-selected", &destination);
        assert_eq!(exported["status"], "ok");
        assert_eq!(exported["incomplete"], partial);
        let inspected = run(&f, "inspect", &destination);
        assert_eq!(inspected["status"], "ok");
        assert_eq!(inspected["data"], exported["data"]);
        assert_eq!(inspected["data"]["inspection"]["format"], 2);
        assert_eq!(
            inspected["data"]["inspection"]["configuration_included"],
            false
        );
        assert_eq!(inspected["data"]["inspection"]["complete_media"], !partial);
        assert!(
            inspected["data"]["inspection"]["selection_receipts"]
                .as_u64()
                .unwrap()
                >= 2
        );
        assert_eq!(f.inventory(), before);
        assert_no_sentinels(&destination);
        let archive: Value =
            serde_json::from_slice(&fs::read(destination.join("backup.json")).unwrap()).unwrap();
        assert!(archive["manifests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|manifest| manifest["record_namespaces"]
                .as_object()
                .unwrap()
                .values()
                .any(|namespace| namespace == "source")));
        let historical = Store::inspect_selected_backup(&destination, &scope, 4096)
            .unwrap()
            .unwrap();
        assert_eq!(historical.selected_records.len(), 1);
        assert_eq!(historical.retained_records.len(), 1);
        assert_eq!(historical.media.is_empty(), partial);
        let store = Store::open(f.paths()).unwrap();
        assert_eq!(
            store
                .selected_snapshot(&scope, 4096)
                .unwrap()
                .unwrap()
                .token,
            token
        );
        assert!(store.restore(&destination).is_err());
        assert_eq!(f.inventory(), before);
        assert!(store.export(&f.root.join("legacy-refusal")).is_err());
        assert!(!f.root.join("legacy-refusal/backup.json").exists());
    }
}

#[test]
fn inspector_accepts_verified_legacy_config_presence_without_loading_config_or_opening_store() {
    let source = Fixture::new();
    let store = Store::open(source.paths()).unwrap();
    let config = Config::load(Some(source.root.join("config.json"))).unwrap();
    store.snapshot_config(&config).unwrap();
    let archive = source.root.join("legacy-archive");
    store.export(&archive).unwrap();
    drop(store);
    let f = Fixture::new();
    fs::write(f.root.join("config.json"), b"PRIVATE_INVALID_CONFIG").unwrap();
    fs::write(
        f.root.join(".env"),
        b"READER_BUDDY_DEBUG_DUMP=PRIVATE_POISON",
    )
    .unwrap();
    let report = run(&f, "inspect", &archive);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["data"]["inspection"]["format"], 1);
    assert_eq!(report["data"]["inspection"]["configuration_included"], true);
    assert!(f.inventory().is_empty());
    let missing = f.root.join("missing");
    assert_eq!(run(&f, "inspect", &missing)["status"], "unavailable");
    assert!(!missing.exists());
    assert!(f.inventory().is_empty());
}

#[test]
fn bounded_strict_file_stdin_input_and_actual_option_conflicts_refuse_before_storage() {
    let f = Fixture::new();
    let target = f.root.join("missing");
    let request = serde_json::to_vec(&json!({"path":target})).unwrap();
    let mut exact = request.clone();
    exact.resize(MAX_INPUT, b' ');
    assert_eq!(
        run_bytes(&f, "inspect", &exact, true),
        run_bytes(&f, "inspect", &exact, false)
    );
    exact.push(b' ');
    for file in [false, true] {
        assert_eq!(
            run_bytes(&f, "export-selected", &exact, file)["status"],
            "invalid-input"
        );
    }
    for request in [
        json!({"path":target,"unknown":"PRIVATE_SENTINEL"}),
        json!({"path":"relative"}),
        json!({"path":42}),
        json!({}),
        json!({"path":null}),
    ] {
        assert_eq!(
            run_bytes(
                &f,
                "export-selected",
                &serde_json::to_vec(&request).unwrap(),
                false
            )["status"],
            "invalid-input"
        );
    }
    assert_eq!(
        run_bytes(&f, "export-selected", b"{malformed", false)["status"],
        "invalid-input"
    );
    let mut cmd = command(&f);
    cmd.args([
        "--archive-operation",
        "inspect",
        "--archive-input",
        "relative.json",
    ]);
    let out = output(cmd, None);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    let mut cmd = command(&f);
    cmd.args(["--archive-operation", "inspect", "--archive-input"])
        .arg(&f.root);
    let out = output(cmd, None);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    for extra in [
        vec!["--model", "PRIVATE_MODEL"],
        vec!["--simulate", "PRIVATE_PATH"],
        vec!["--log-level", "trace"],
        vec!["--memory-operation", "query", "--memory-input", "-"],
        vec!["--sync-mode-operation", "inspect"],
        vec!["--personalization-operation", "inspect"],
        vec!["--export-note"],
        vec!["--inspect-owned-data"],
    ] {
        let mut cmd = command(&f);
        cmd.args(["--archive-operation", "inspect", "--archive-input", "-"])
            .args(extra);
        let out = output(cmd, None);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE_"));
    }
    for args in [
        vec!["--archive-operation", "inspect"],
        vec!["--archive-input", "-"],
    ] {
        let mut cmd = command(&f);
        cmd.args(args);
        assert!(!output(cmd, None).status.success());
    }
    assert!(f.inventory().is_empty());
    assert!(!target.exists());
}

#[test]
fn export_busy_missing_partial_legacy_and_destination_refusals_preserve_bytes() {
    let f = Fixture::new();
    let destination = f.root.join("archive");
    assert_eq!(
        run(&f, "export-selected", &destination)["status"],
        "unavailable"
    );
    assert!(f.inventory().is_empty());
    let (store, _) = f.selected(false);
    let before = f.inventory();
    assert_eq!(run(&f, "export-selected", &destination)["status"], "busy");
    assert_eq!(f.inventory(), before);
    assert!(!destination.exists());
    drop(store);
    let owner = f.paths().cache.join("owner.json");
    let saved = f.paths().cache.join("saved-owner");
    fs::rename(&owner, &saved).unwrap();
    let before = f.inventory();
    assert_eq!(
        run(&f, "export-selected", &destination)["status"],
        "unavailable"
    );
    assert_eq!(f.inventory(), before);
    fs::rename(saved, owner).unwrap();
    let existing = f.root.join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("keep"), b"keep").unwrap();
    let before = f.inventory();
    for target in [
        existing.clone(),
        f.root.clone(),
        f.paths().data.join("nested"),
        f.paths().cache.join("nested"),
        f.paths().credentials.join("nested"),
    ] {
        assert_eq!(
            run(&f, "export-selected", &target)["status"],
            "invalid-input"
        );
        assert_eq!(f.inventory(), before);
    }
    assert_eq!(fs::read(existing.join("keep")).unwrap(), b"keep");
    f.save_config(true);
    assert_eq!(
        run(&f, "export-selected", &destination)["status"],
        "unsupported"
    );
    assert_eq!(f.inventory(), before);
    assert!(!destination.exists());
    let ordinary = Fixture::new();
    let store = Store::open(ordinary.paths()).unwrap();
    drop(store);
    let before = ordinary.inventory();
    let target = ordinary.root.join("archive");
    assert_eq!(
        run(&ordinary, "export-selected", &target)["status"],
        "failed"
    );
    assert_eq!(ordinary.inventory(), before);
    assert!(!target.exists());
}

#[test]
fn complete_archive_corruption_and_unknown_format_never_become_empty_success() {
    let f = Fixture::new();
    let (store, _) = f.selected(false);
    let target = f.root.join("archive");
    store.export_selected(&target).unwrap();
    drop(store);
    let marker = target.join("backup.json");
    let original = fs::read(&marker).unwrap();
    let mut value: Value = serde_json::from_slice(&original).unwrap();
    value["format"] = json!(999);
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = fs::read(&marker).unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    assert_eq!(fs::read(&marker).unwrap(), before);
    fs::write(&marker, &original).unwrap();
    let object = fs::read_dir(target.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = fs::read(&object).unwrap();
    let missing = target.join("temporarily-missing-object");
    fs::rename(&object, &missing).unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    fs::rename(&missing, &object).unwrap();
    fs::write(&object, b"PRIVATE_CORRUPT_OBJECT").unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    assert_eq!(fs::read(&object).unwrap(), b"PRIVATE_CORRUPT_OBJECT");
    fs::write(object, bytes).unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "ok");
    assert_eq!(fs::read(marker).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn fifo_input_archive_marker_and_store_lock_refuse_without_blocking() {
    let f = Fixture::new();
    let fifo = f.root.join("input-fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut cmd = command(&f);
    cmd.args(["--archive-operation", "inspect", "--archive-input"])
        .arg(&fifo);
    let out = output(cmd, None);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    let target = f.root.join("archive");
    fs::create_dir(&target).unwrap();
    assert!(Command::new("mkfifo")
        .arg(target.join("backup.json"))
        .status()
        .unwrap()
        .success());
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    let data = f.paths().data;
    fs::create_dir(&data).unwrap();
    assert!(Command::new("mkfifo")
        .arg(data.join("store.lock"))
        .status()
        .unwrap()
        .success());
    assert_eq!(
        run(&f, "export-selected", &f.root.join("destination"))["status"],
        "unsupported"
    );
    assert!(!f.root.join("destination").exists());
    let regular = f.root.join("regular-input.json");
    fs::write(
        &regular,
        serde_json::to_vec(&json!({"path":target})).unwrap(),
    )
    .unwrap();
    let linked = f.root.join("linked-input.json");
    std::os::unix::fs::symlink(&regular, &linked).unwrap();
    let mut cmd = command(&f);
    cmd.args(["--archive-operation", "inspect", "--archive-input"])
        .arg(&linked);
    assert_eq!(
        serde_json::from_slice::<Value>(&output(cmd, None).stdout).unwrap()["status"],
        "invalid-input"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn notebook_archive_cli_exports_product_generated_records_and_inspects_detached() {
    let f = Fixture::new();
    let maintained =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    let mut scenario: Value = serde_json::from_slice(
        &fs::read(maintained.join("writer-notebook-create-replay.json")).unwrap(),
    )
    .unwrap();
    scenario["output"] = json!("output");
    scenario["retain_store_fixture"] = json!(true);
    fs::copy(
        maintained.join("writer-notebook-source.json"),
        f.root.join("writer-notebook-source.json"),
    )
    .unwrap();
    let input = f.root.join("scenario.json");
    fs::write(&input, serde_json::to_vec(&scenario).unwrap()).unwrap();
    let mut simulate = command(&f);
    simulate.arg("--simulate").arg(&input);
    let out = output(simulate, None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let scenario_report: Value =
        serde_json::from_slice(&fs::read(f.root.join("output/report.json")).unwrap()).unwrap();
    assert_eq!(scenario_report["model_calls"], 3);
    assert_eq!(scenario_report["store_fixture_saved"], true);

    // The simulator deliberately exports data only. Explicit fixture preparation
    // validates/rebinds that data before any archive command is exercised.
    let paths = buddy::storage::StorePaths {
        data: f.root.join("output/store-fixture/data"),
        ..f.paths()
    };
    let config = f.root.join("config.json");
    let store = Store::prepare_notebook(paths.clone()).unwrap();
    let manifests = store.manifests().unwrap();
    let conversation = serde_json::from_value(scenario["conversation"].clone()).unwrap();
    let ledger = buddy::conversation::Ledger::new(std::sync::Arc::new(store));
    let records = ledger.inspect(conversation, false).unwrap();
    assert!(records
        .iter()
        .any(|r| matches!(r, buddy::conversation::Record::ArchivedNormalizedSource(_))));
    assert!(records
        .iter()
        .any(|r| matches!(r, buddy::conversation::Record::NotebookPlan(_))));
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, buddy::conversation::Record::NotebookStage(_)))
            .count(),
        3
    );
    drop(ledger);
    fs::write(
        &config,
        serde_json::to_vec(&json!({"paths":paths})).unwrap(),
    )
    .unwrap();
    let current = fs::read(paths.data.join("CURRENT")).unwrap();
    let generation: String = serde_json::from_slice(&current).unwrap();
    let source = paths.data.join("generations").join(&generation);
    let read_inventory = |directory: &Path| {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_owned(),
                    fs::read(path).unwrap(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let objects = read_inventory(&source.join("objects"));
    let commits = read_inventory(&source.join("commits"));
    let archive = f.root.join("notebook-archive");
    let exported = run(&f, "export-notebook", &archive);
    assert_eq!(exported["status"], "ok");
    let summary = &exported["data"]["inspection"];
    assert_eq!(summary["format"], 4);
    assert_eq!(summary["complete_media"], true);
    assert_eq!(summary["selected_scopes"], 0);
    assert_eq!(summary["selection_receipts"], 0);
    assert_eq!(summary["manifests"], manifests.len());
    let marker = fs::read(archive.join("backup.json")).unwrap();
    let backup: Value = serde_json::from_slice(&marker).unwrap();
    assert_eq!(backup["origin_generation"], generation);
    let identity: Value =
        serde_json::from_slice(&fs::read(paths.data.join("identity.json")).unwrap()).unwrap();
    assert_eq!(backup["origin_actor"], identity["actor"]);
    assert_eq!(
        backup["manifests"],
        serde_json::to_value(manifests).unwrap()
    );
    let archived = read_inventory(&archive.join("objects"));
    for (name, bytes) in &archived {
        assert_eq!(objects.get(name), Some(bytes));
    }
    assert_eq!(
        archived.len(),
        summary["objects"].as_u64().unwrap() as usize
    );
    assert_eq!(
        run(&f, "export-notebook", &archive)["status"],
        "invalid-input"
    );
    assert_eq!(fs::read(archive.join("backup.json")).unwrap(), marker);
    fs::write(config, b"{poison config").unwrap();
    let detached = run(&f, "inspect-notebook", &archive);
    assert_eq!(detached["status"], "ok");
    assert_eq!(detached["data"], exported["data"]);
    assert_eq!(run(&f, "inspect", &archive)["status"], "unsupported");
    fs::write(archive.join("objects/undeclared"), b"extra").unwrap();
    assert_eq!(
        run(&f, "inspect-notebook", &archive)["status"],
        "unsupported"
    );
    fs::remove_file(archive.join("objects/undeclared")).unwrap();
    let (name, bytes) = archived.into_iter().next().unwrap();
    let object = archive.join("objects").join(name);
    fs::remove_file(&object).unwrap();
    assert_eq!(
        run(&f, "inspect-notebook", &archive)["status"],
        "unsupported"
    );
    let mut damaged = bytes;
    damaged[0] ^= 1;
    fs::write(object, damaged).unwrap();
    assert_eq!(
        run(&f, "inspect-notebook", &archive)["status"],
        "unsupported"
    );
    assert_eq!(fs::read(paths.data.join("CURRENT")).unwrap(), current);
    assert_eq!(read_inventory(&source.join("objects")), objects);
    assert_eq!(read_inventory(&source.join("commits")), commits);
}

#[test]
fn notebook_archive_cli_refuses_busy_wrong_format_and_incomplete_admission() {
    let f = Fixture::new();
    let path = f.root.join("destination");
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unavailable");
    assert!(!f.paths().data.exists());
    let legacy = Store::open(f.paths()).unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "busy");
    let legacy_archive = f.root.join("legacy");
    legacy.export(&legacy_archive).unwrap();
    use buddy::storage::{sync::mode::SyncModeChoice, Uuid};
    let mode = f.paths().data.join("sync/mode.json");
    legacy
        .set_sync_mode(
            &legacy.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::IsolatedBackup {
                binding_sha256: buddy::storage::digest(b"archive fixture"),
            },
        )
        .unwrap();
    let nonlocal_mode = fs::read(&mode).unwrap();
    legacy
        .set_sync_mode(
            &legacy.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::LocalOnly,
        )
        .unwrap();
    let local_mode = fs::read(&mode).unwrap();
    drop(legacy);
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    assert_eq!(
        run(&f, "inspect-notebook", &legacy_archive)["status"],
        "unsupported"
    );
    let notebook = Store::prepare_notebook(f.paths()).unwrap();
    let current: String =
        serde_json::from_slice(&fs::read(f.paths().data.join("CURRENT")).unwrap()).unwrap();
    drop(notebook);
    let root = f.paths().data.join("generations").join(current);
    let capabilities = f.paths().data.join("capabilities.json");
    let original = fs::read(&capabilities).unwrap();
    fs::remove_file(&capabilities).unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::write(&capabilities, original).unwrap();
    fs::write(&mode, b"{").unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::write(&mode, nonlocal_mode).unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::write(&mode, local_mode).unwrap();
    let history = root.join("selection-history");
    fs::create_dir(&history).unwrap();
    fs::write(history.join("retained"), b"negative fixture").unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::remove_file(history.join("retained")).unwrap();
    let local_archive = f.root.join("local-only");
    let exported = run(&f, "export-notebook", &local_archive);
    assert_eq!(exported["status"], "ok");
    let inspected = run(&f, "inspect-notebook", &local_archive);
    assert_eq!(inspected["status"], "ok");
    assert_eq!(inspected["data"], exported["data"]);
    fs::write(root.join("format.json"), b"1").unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    assert!(!path.exists());
}
