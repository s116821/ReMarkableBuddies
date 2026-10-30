//! Actual host CLI on existing ordinary/selected synthetic Stores; no tablet/provider.
use buddy::storage::{digest, maintenance::Refusal, sync::mode::SyncModeChoice, Store, Uuid};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;

fn command(f: &Fixture) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    cmd.env_clear()
        .current_dir(&f.root)
        .env("REMARKABLE_BUDDIES_CONFIG", f.root.join("config.json"))
        .env("OPENAI_API_KEY", "PRIVATE_ENV_SENTINEL")
        .env(
            "OPENAI_BASE_URL",
            "http://127.0.0.1:1/private-provider-sentinel",
        )
        .env("RUST_LOG", "trace");
    cmd
}
fn output(mut command: Command, input: Option<&[u8]>) -> Output {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().unwrap();
    let writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut pipe = child.stdin.take().unwrap();
        std::thread::spawn(move || pipe.write_all(&bytes))
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("basic CLI exceeded caller deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(writer) = writer {
        if let Err(error) = writer.join().unwrap() {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
    }
    child.wait_with_output().unwrap()
}
fn run_bytes(f: &Fixture, operation: &str, input: Option<&[u8]>) -> Value {
    let mut cmd = command(f);
    cmd.args(["--basic-settings-operation", operation]);
    if input.is_some() {
        cmd.args(["--basic-settings-input", "-"]);
    }
    let out = output(cmd, input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= buddy::config::basic::MAX_IO && out.stdout.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn run(f: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(f, operation, Some(&serde_json::to_vec(input).unwrap()))
}
fn prepare_request(f: &Fixture, changes: Value) -> Value {
    let view = run_bytes(f, "inspect", None);
    assert_eq!(view["status"], "ok");
    json!({"basic_version":1,"operation":Uuid::new_v4(),"base_sha256":view["data"]["base_sha256"],"config_path_sha256":view["data"]["config_path_sha256"],"changes":changes})
}
fn prepare(f: &Fixture, changes: Value) -> Value {
    let result = run(f, "prepare", &prepare_request(f, changes));
    assert_eq!(result["outcome"], "prepared");
    json!({"intent":result["data"]["intent"]})
}

#[test]
fn actual_basic_patch_preserves_private_values_origins_noop_and_readback() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let mut config: Value =
        serde_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
    config["model"] = Value::Null;
    config["base_url"] =
        json!("https://user:PRIVATE_PASSWORD@invalid/?token=PRIVATE_QUERY#PRIVATE_FRAGMENT");
    config["model_credential"] = json!("PRIVATE_REFERENCE");
    config["debug_dump"] = Value::Null;
    config["log_level"] = Value::Null;
    config["personalization"] = json!({"text":"PRIVATE_MULTILINE\r\n \u{03b1} ","enabled":false});
    let original = serde_json::to_vec_pretty(&config).unwrap();
    fs::write(f.root.join("config.json"), &original).unwrap();
    let inventory = f.inventory();
    let unchanged = run(
        &f,
        "prepare",
        &prepare_request(&f, json!({"trigger_corner":{"action":"clear"}})),
    );
    assert_eq!(unchanged["outcome"], "no-change");
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), original);
    let intent = prepare(
        &f,
        json!({"trigger_corner":{"action":"set","value":"UL"},"log_level":{"action":"clear"},"debug_dump":{"action":"set","value":true}}),
    );
    assert_eq!(run(&f, "status", &intent)["outcome"], "unresolved");
    let applied = run(&f, "apply", &intent);
    assert_eq!(applied["outcome"], "verified");
    assert_eq!(applied["disk_readback_verified"], true);
    assert_eq!(applied["runtime_activation"], "restart-required");
    let bytes = fs::read(f.root.join("config.json")).unwrap();
    let after: Value = serde_json::from_slice(&bytes).unwrap();
    let mut before_other = config.as_object().unwrap().clone();
    let mut after_other = after.as_object().unwrap().clone();
    for key in ["trigger_corner", "log_level", "debug_dump"] {
        before_other.remove(key);
        after_other.remove(key);
    }
    assert_eq!(before_other, after_other);
    assert!(after.get("log_level").is_none());
    assert_eq!(after["trigger_corner"], "UL");
    assert_eq!(after["debug_dump"], true);
    assert_eq!(applied["data"]["target_sha256"], digest(&bytes));
    let observed = run(&f, "status", &intent);
    assert_eq!(observed["outcome"], "desired-file-observed");
    assert_eq!(observed["runtime_activation"], "unverified");
    assert_eq!(observed["disk_readback_verified"], false);
    assert_eq!(run(&f, "apply", &intent)["status"], "conflict");
    assert_eq!(f.inventory(), inventory);
}

#[test]
fn actual_basic_existing_ordinary_and_selected_all_modes_keep_storage_byte_exact() {
    for selected in [false, true] {
        for mode in 0..4 {
            let f = Fixture::new();
            let store = if selected {
                f.selected(false).0
            } else {
                Arc::new(Store::open(f.paths()).unwrap())
            };
            let choice = match mode {
                0 => None,
                1 => Some(SyncModeChoice::LocalOnly),
                2 => Some(SyncModeChoice::IsolatedBackup {
                    binding_sha256: digest(b"synthetic isolated binding"),
                }),
                _ => Some(SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: digest(b"synthetic shared binding"),
                    root_id: "synthetic-root".into(),
                }),
            };
            if let Some(choice) = choice {
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        choice,
                    )
                    .unwrap();
            }
            drop(store);
            if selected {
                assert!(matches!(
                    Store::open_existing(f.paths()),
                    Err(Refusal::Unsupported)
                ));
            }
            let inventory = f.inventory();
            let intent = prepare(&f, json!({"debug_dump":{"action":"set","value":true}}));
            let applied = run(&f, "apply", &intent);
            assert_eq!(
                applied["status"], "ok",
                "selected={selected} mode={mode}: {applied}"
            );
            assert_eq!(applied["outcome"], "verified");
            assert_eq!(f.inventory(), inventory);
            assert_eq!(
                run(&f, "status", &intent)["outcome"],
                "desired-file-observed"
            );
        }
    }
}

#[test]
fn actual_basic_busy_corrupt_and_uninitialized_admission_never_repairs_or_writes() {
    let f = Fixture::new();
    let original = fs::read(f.root.join("config.json")).unwrap();
    let intent = prepare(&f, json!({"debug_dump":{"action":"set","value":true}}));
    assert!(!f.paths().data.exists());
    assert_eq!(run(&f, "apply", &intent)["status"], "unavailable");
    assert!(!f.paths().data.exists());
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), original);
    let owner = Store::open(f.paths()).unwrap();
    let inventory = f.inventory();
    let busy = run(&f, "apply", &intent);
    assert_eq!(busy["status"], "busy");
    assert_eq!(busy["outcome"], "not-dispatched");
    assert_eq!(run(&f, "status", &intent)["outcome"], "unresolved");
    assert_eq!(f.inventory(), inventory);
    drop(owner);
    let generation: Uuid =
        serde_json::from_slice(&fs::read(f.paths().data.join("CURRENT")).unwrap()).unwrap();
    fs::write(
        f.paths()
            .data
            .join("generations")
            .join(generation.to_string())
            .join("format.json"),
        b"99",
    )
    .unwrap();
    let inventory = f.inventory();
    let refused = run(&f, "apply", &intent);
    assert_eq!(refused["status"], "unsupported");
    assert_eq!(refused["outcome"], "not-dispatched");
    assert_eq!(f.inventory(), inventory);
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), original);
}

#[test]
fn actual_basic_strict_shapes_caps_original_conditions_and_mixed_flags_refuse() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let original = fs::read(f.root.join("config.json")).unwrap();
    let inventory = f.inventory();
    let request = prepare_request(&f, json!({"debug_dump":{"action":"set","value":true}}));
    let valid = serde_json::to_vec(&request).unwrap();
    let mut exact = valid.clone();
    exact.resize(buddy::config::basic::MAX_IO, b' ');
    assert_eq!(
        run_bytes(&f, "prepare", Some(&exact))["outcome"],
        "prepared"
    );
    exact.push(b' ');
    assert_eq!(
        run_bytes(&f, "prepare", Some(&exact))["status"],
        "invalid-input"
    );
    let mut invalid = request.clone();
    invalid["changes"] = json!({"debug_dump":["clear"]});
    assert_eq!(run(&f, "prepare", &invalid)["status"], "invalid-input");
    invalid = request.clone();
    invalid["changes"] = json!({"debug_dump":{"action":"set","value":null}});
    assert_eq!(run(&f, "prepare", &invalid)["status"], "invalid-input");
    invalid = request.clone();
    invalid["changes"] = json!({"paths":{"action":"clear"}});
    assert_eq!(run(&f, "prepare", &invalid)["status"], "invalid-input");
    invalid = request.clone();
    invalid["changes"] = json!({"debug_dump":{"action":"clear","extra":true}});
    assert_eq!(run(&f, "prepare", &invalid)["status"], "invalid-input");
    let array = json!([
        request["basic_version"],
        request["operation"],
        request["base_sha256"],
        request["config_path_sha256"],
        request["changes"]
    ]);
    assert_eq!(run(&f, "prepare", &array)["status"], "invalid-input");
    invalid = request.clone();
    invalid["base_sha256"] = json!("0".repeat(64));
    assert_eq!(run(&f, "prepare", &invalid)["status"], "conflict");
    let prepared = run(&f, "prepare", &request);
    let mut intent = json!({"intent":prepared["data"]["intent"]});
    intent["intent"]["target_sha256"] = json!("0".repeat(64));
    assert_eq!(run(&f, "apply", &intent)["status"], "invalid-input");
    for flags in [
        vec!["--configuration-operation", "inspect"],
        vec!["--personalization-operation", "inspect"],
        vec!["--inspect-owned-data"],
        vec!["--model", "PRIVATE_MODEL"],
        vec!["--log-level", "trace"],
        vec!["--export-note"],
        vec!["--memory-operation", "query", "--memory-input", "-"],
        vec!["--sync-mode-operation", "inspect"],
        vec!["--archive-operation", "inspect", "--archive-input", "-"],
    ] {
        let mut cmd = command(&f);
        cmd.args(["--basic-settings-operation", "inspect"])
            .args(flags);
        let out = output(cmd, None);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }
    let mut cmd = command(&f);
    cmd.args([
        "--basic-settings-operation",
        "apply",
        "--basic-settings-input",
    ])
    .arg(f.root.join("absent.json"));
    let out = output(cmd, None);
    assert!(out.status.success());
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["status"], "unsupported");
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), original);
    assert_eq!(f.inventory(), inventory);
}

#[test]
fn actual_basic_inspect_does_not_wait_for_stdin_and_shape_fix_is_narrow() {
    let f = Fixture::new();
    let mut cmd = command(&f);
    cmd.args(["--basic-settings-operation", "inspect"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let pipe = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("inspect waited for EOF");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = child.wait_with_output().unwrap();
    drop(pipe);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["outcome"],
        "read-only"
    );
    assert!(!f.paths().data.exists());
    for raw in [
        r#"[]"#,
        r#"{"paths":["/a/b","/c/d","/e/f"]}"#,
        r#"{"memory":[]}"#,
        r#"{"sync":[]}"#,
        r#"{"personalization":[]}"#,
    ] {
        fs::write(f.root.join("config.json"), raw).unwrap();
        assert_eq!(run_bytes(&f, "inspect", None)["status"], "invalid-input");
        let mut cmd = command(&f);
        cmd.args(["--configuration-operation", "inspect"]);
        let result = output(cmd, None);
        assert!(result.status.success());
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["status"], "invalid-input");
        assert_eq!(report["snapshot"], Value::Null);
    }
    fs::write(f.root.join("config.json"),br#"{"sync":{"namespaces":["source","conversation"]},"memory":{"selection":null},"debug_dump":null}"#).unwrap();
    let mut cmd = command(&f);
    cmd.args(["--configuration-operation", "inspect"]);
    let report: Value = serde_json::from_slice(&output(cmd, None).stdout).unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(
        report["snapshot"]["fields"]["debug_dump"]["origin"],
        "declared-null"
    );
    assert!(!f.paths().data.exists());
}
