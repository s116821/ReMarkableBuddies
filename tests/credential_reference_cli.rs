//! Actual host CLI on existing ordinary/selected synthetic Stores; no tablet/provider.
use base64::{engine::general_purpose::STANDARD, Engine as _};
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
            panic!("credential_reference CLI exceeded caller deadline");
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
    cmd.args(["--credential-reference-operation", operation]);
    if input.is_some() {
        cmd.args(["--credential-reference-input", "-"]);
    }
    let out = output(cmd, input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(
        out.stdout.len() <= buddy::config::credential_reference::MAX_IO
            && out.stdout.ends_with(b"\n")
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["runtime_activation"], "unverified");
    let record = json!({"operation":operation,"stdin_base64":input.map(|b|STANDARD.encode(b)),"stdin_sha256":input.map(digest),"stdout_utf8":String::from_utf8(out.stdout.clone()).unwrap(),"stdout_sha256":digest(&out.stdout),"exit_code":out.status.code(),"stderr_utf8":""});
    emit("RECORD", &record);
    value
}
fn run(f: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(f, operation, Some(&serde_json::to_vec(input).unwrap()))
}
fn prepare_request(f: &Fixture, change: Value) -> Value {
    let view = run_bytes(f, "inspect", None);
    assert_eq!(view["status"], "ok");
    json!({"credential_reference_version":1,"operation":Uuid::new_v4(),"base_sha256":view["data"]["base_sha256"],"config_path_sha256":view["data"]["config_path_sha256"],"change":change})
}
fn prepare(f: &Fixture, change: Value) -> Value {
    let result = run(f, "prepare", &prepare_request(f, change.clone()));
    assert_eq!(result["outcome"], "prepared");
    json!({"intent":result["data"]["intent"],"change":change})
}

fn emit(kind: &str, value: &Value) {
    let mut bytes = format!("CREDENTIAL_REFERENCE_CORPUS_{kind} ").into_bytes();
    bytes.extend(serde_json::to_vec(value).unwrap());
    bytes.push(b'\n');
    std::io::stdout().lock().write_all(&bytes).unwrap();
}
#[test]
fn clean_candidate_provenance() {
    let source = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let clean = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(source.status.success() && clean.status.success() && clean.stdout.is_empty());
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_reader-buddy"));
    emit(
        "PROVENANCE",
        &json!({"source_checkout_sha":String::from_utf8(source.stdout).unwrap().trim(),"executable_sha256":digest(&fs::read(exe).unwrap()),"scope":"Actual host CLI against task-owned synthetic Config and Stores; no real credential/provider/tablet/runtime or transport-loss test."}),
    );
}
#[test]
fn actual_private_set_clear_origins_case_and_exact_noops() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let path = f.root.join("config.json");
    let original = fs::read(&path).unwrap();
    let clear = json!({"action":"clear"});
    let absent = run(&f, "prepare", &prepare_request(&f, clear.clone()));
    assert_eq!(absent["outcome"], "no-change");
    assert!(absent["data"].get("intent").is_none());
    assert_eq!(fs::read(&path).unwrap(), original);
    let before: Value = serde_json::from_slice(&original).unwrap();
    let change = json!({"action":"set","value":"PRIVATE_Ref-api_01"});
    let input = prepare(&f, change.clone());
    assert!(input["intent"].get("change").is_none());
    let status = json!({"intent":input["intent"]});
    assert_eq!(run(&f, "status", &status)["outcome"], "unresolved");
    let inventory = f.inventory();
    let applied = run(&f, "apply", &input);
    assert_eq!(applied["outcome"], "verified");
    assert_eq!(applied["disk_readback_verified"], true);
    assert_eq!(f.inventory(), inventory);
    let saved = fs::read(&path).unwrap();
    let after: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(after["model_credential"], change["value"]);
    for (key, value) in before.as_object().unwrap() {
        assert_eq!(&after[key], value);
    }
    assert_eq!(
        run(&f, "status", &status)["outcome"],
        "desired-file-observed"
    );
    assert_eq!(run(&f, "apply", &input)["status"], "conflict");
    let mut escaped = String::from_utf8(saved)
        .unwrap()
        .replace("PRIVATE_Ref-api_01", r"\u0050RIVATE_Ref-api_01");
    escaped.push_str(" \n");
    fs::write(&path, escaped.as_bytes()).unwrap();
    assert_eq!(
        run(&f, "prepare", &prepare_request(&f, change))["outcome"],
        "no-change"
    );
    assert_eq!(fs::read(&path).unwrap(), escaped.as_bytes());
    let case = prepare(&f, json!({"action":"set","value":"PRIVATE_ref-api_01"}));
    assert_eq!(run(&f, "apply", &case)["outcome"], "verified");
    let input = prepare(&f, clear.clone());
    assert_eq!(run(&f, "apply", &input)["outcome"], "verified");
    assert_eq!(
        run_bytes(&f, "inspect", None)["data"]["current"]["model_credential"],
        json!({"origin":"default","present":false})
    );
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["model_credential"] = Value::Null;
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        run_bytes(&f, "inspect", None)["data"]["current"]["model_credential"],
        json!({"origin":"declared-null","present":false})
    );
    let input = prepare(&f, clear);
    assert_eq!(run(&f, "apply", &input)["outcome"], "verified");
}
#[test]
fn actual_credential_reference_existing_ordinary_and_selected_all_modes_keep_storage_byte_exact() {
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
            let intent = prepare(&f, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
            let applied = run(&f, "apply", &intent);
            assert_eq!(
                applied["status"], "ok",
                "selected={selected} mode={mode}: {applied}"
            );
            assert_eq!(applied["outcome"], "verified");
            assert_eq!(f.inventory(), inventory);
            assert_eq!(
                run(&f, "status", &json!({"intent":intent["intent"]}))["outcome"],
                "desired-file-observed"
            );
        }
    }
}

#[test]
fn actual_original_conditions_busy_and_pure_reconciliation() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let input = prepare(&f, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
    let path = f.root.join("config.json");
    let before = fs::read(&path).unwrap();
    let inventory = f.inventory();
    let mut changed = input.clone();
    changed["change"]["value"] = json!("PRIVATE_OTHER");
    assert_eq!(run(&f, "apply", &changed)["status"], "invalid-input");
    for field in [
        "base_sha256",
        "config_path_sha256",
        "target_sha256",
        "changes_sha256",
    ] {
        let mut bad = input.clone();
        bad["intent"][field] = json!("0".repeat(64));
        assert_ne!(run(&f, "apply", &bad)["status"], "ok");
    }
    let owner = Store::open(f.paths()).unwrap();
    assert_eq!(run(&f, "apply", &input)["status"], "busy");
    assert_eq!(run_bytes(&f, "inspect", None)["status"], "ok");
    assert_eq!(
        run(&f, "status", &json!({"intent":input["intent"]}))["outcome"],
        "unresolved"
    );
    assert_eq!(f.inventory(), inventory);
    drop(owner);
    let mut new = before.clone();
    new.push(b' ');
    fs::write(&path, &new).unwrap();
    assert_eq!(run(&f, "apply", &input)["status"], "conflict");
    assert_eq!(fs::read(&path).unwrap(), new);
    // Original status is observation-only and does not need the Store or any root.
    let pure = Fixture::new();
    let input = prepare(&pure, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
    assert_eq!(run(&pure, "apply", &input)["status"], "unavailable");
    assert_eq!(
        run(&pure, "status", &json!({"intent":input["intent"]}))["outcome"],
        "unresolved"
    );
    assert!(
        !pure.paths().data.exists()
            && !pure.paths().cache.exists()
            && !pure.paths().credentials.exists()
    );
}
#[test]
fn actual_strict_records_versions_caps_and_invalid_whole_original() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let request = prepare_request(&f, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
    for change in [
        json!({"action":"clear","value":"PRIVATE_EXTRA"}),
        json!({"action":"clear","extra":true}),
        json!({"action":"set","value":null}),
        json!({"action":"set","value":"x","extra":true}),
        json!(["clear"]),
        json!({"action":"set","value":"bad.name"}),
        json!({"action":"set","value":"x".repeat(81)}),
    ] {
        let mut bad = request.clone();
        bad["change"] = change;
        assert_eq!(run(&f, "prepare", &bad)["status"], "invalid-input");
    }
    let raw = serde_json::to_string(&request).unwrap();
    for bad in [
        raw.replacen('{', "{\"credential_reference_version\":1,", 1),
        raw.replace(
            "\"action\":\"set\"",
            "\"action\":\"set\",\"action\":\"clear\"",
        ),
        raw.clone() + " {}",
        raw.replace("\"change\":{", "\"change\":{\"unknown\":0,"),
    ] {
        assert_eq!(
            run_bytes(&f, "prepare", Some(bad.as_bytes()))["status"],
            "invalid-input"
        );
    }
    let mut padded = serde_json::to_vec(&request).unwrap();
    padded.resize(16384, b' ');
    assert_eq!(
        run_bytes(&f, "prepare", Some(&padded))["outcome"],
        "prepared"
    );
    padded.push(b' ');
    assert_eq!(
        run_bytes(&f, "prepare", Some(&padded))["status"],
        "invalid-input"
    );
    for (key, value, status) in [
        ("credential_reference_version", json!(2), "unsupported"),
        ("operation", json!(Uuid::nil()), "invalid-input"),
        ("base_sha256", json!("A".repeat(64)), "invalid-input"),
    ] {
        let mut bad = request.clone();
        bad[key] = value;
        assert_eq!(run(&f, "prepare", &bad)["status"], status);
    }
    let prepared = prepare(&f, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
    let mut extra = json!({"intent":prepared["intent"],"change":{"action":"clear"}});
    assert_eq!(run(&f, "status", &extra)["status"], "invalid-input");
    extra.as_object_mut().unwrap().remove("change");
    extra["intent"]["unknown"] = json!(true);
    assert_eq!(run(&f, "status", &extra)["status"], "invalid-input");
    let path = f.root.join("config.json");
    for bytes in [
        br#"{"model_credential":"bad.name"}"#.as_slice(),
        br#"{"model_credential":7}"#,
        br#"{"model_credential":null,"model_credential":null}"#,
        br#"{"unknown":true}"#,
        br#"{"memory":[]}"#,
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(run_bytes(&f, "inspect", None)["status"], "invalid-input");
        assert_eq!(run(&f, "prepare", &request)["status"], "invalid-input");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(&path, vec![b' '; 1048577]).unwrap();
    assert_eq!(run_bytes(&f, "inspect", None)["status"], "unavailable");
}
#[test]
fn actual_flags_reject_every_other_lane_and_input_fallback() {
    let f = Fixture::new();
    let mut combos = vec![
        vec!["--simulate", "missing.json"],
        vec!["--inspect-owned-data"],
        vec!["--api-key", "PRIVATE_VALUE"],
        vec!["--model", "PRIVATE_VALUE"],
        vec!["--base-url", "PRIVATE_VALUE"],
        vec!["--trigger-corner", "UR"],
        vec!["--log-level", "trace"],
        vec!["--debug-dump"],
        vec!["--export-note"],
        vec!["--personalization-operation", "inspect"],
        vec!["--configuration-operation", "inspect"],
    ];
    for (op, input, value) in [
        (
            "--model-credential-operation",
            "--model-credential-input",
            "inspect",
        ),
        (
            "--memory-settings-operation",
            "--memory-settings-input",
            "inspect",
        ),
        (
            "--model-settings-operation",
            "--model-settings-input",
            "inspect",
        ),
        (
            "--basic-settings-operation",
            "--basic-settings-input",
            "inspect",
        ),
        ("--restore-operation", "--restore-input", "status"),
        (
            "--ordinary-export-operation",
            "--ordinary-export-input",
            "status",
        ),
        ("--memory-operation", "--memory-input", "inspect"),
        ("--sync-mode-operation", "--sync-mode-input", "inspect"),
        ("--archive-operation", "--archive-input", "inspect"),
    ] {
        combos.push(vec![op, value, input, "-"]);
    }
    for extra in combos {
        let mut cmd = command(&f);
        cmd.args(["--credential-reference-operation", "inspect"])
            .args(extra);
        let out = output(cmd, None);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE_VALUE"));
    }
    for selector in [
        None,
        Some("some-file.json"),
        Some("./-"),
        Some(".\\-"),
        Some("-/"),
        Some("-\\"),
        Some("-/."),
    ] {
        let mut cmd = command(&f);
        cmd.args(["--credential-reference-operation", "prepare"]);
        if let Some(s) = selector {
            cmd.arg(format!("--credential-reference-input={s}"));
        }
        let out = output(cmd, None);
        assert!(out.status.success());
        let r: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(r["status"], "unsupported");
    }
    let mut cmd = command(&f);
    cmd.args([
        "--credential-reference-operation",
        "inspect",
        "--credential-reference-input",
        "-",
    ]);
    let out = output(cmd, None);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
}
#[test]
fn actual_inspect_does_not_wait_for_stdin_and_prepare_requires_eof() {
    let f = Fixture::new();
    let mut cmd = command(&f);
    cmd.args(["--credential-reference-operation", "inspect"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let held = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("inspect waited on held stdin");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = child.wait_with_output().unwrap();
    drop(held);
    assert!(out.status.success() && out.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "ok"
    );
    let request = prepare_request(&f, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
    let mut cmd = command(&f);
    cmd.args([
        "--credential-reference-operation",
        "prepare",
        "--credential-reference-input",
        "-",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    drop(pipe);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("prepare failed to complete after EOF");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["outcome"],
        "prepared"
    );
    assert!(!f.paths().data.exists());
}
#[cfg(unix)]
#[test]
fn actual_pure_readers_ignore_credential_root_alias_but_apply_keeps_safety() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let target = f.root.join("root-alias");
    std::os::unix::fs::symlink(f.paths().credentials, &target).unwrap();
    let path = f.root.join("config.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["paths"]["credentials"] = json!(target);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let inventory = f.inventory();
    let input = prepare(&f, json!({"action":"set","value":"PRIVATE_REFERENCE"}));
    assert_eq!(
        run(&f, "status", &json!({"intent":input["intent"]}))["outcome"],
        "unresolved"
    );
    assert_eq!(run(&f, "apply", &input)["status"], "unsupported");
    assert_eq!(f.inventory(), inventory);
}
