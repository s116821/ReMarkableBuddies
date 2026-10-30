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
            panic!("model_settings CLI exceeded caller deadline");
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
    cmd.args(["--model-settings-operation", operation]);
    if input.is_some() {
        cmd.args(["--model-settings-input", "-"]);
    }
    let out = output(cmd, input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(
        out.stdout.len() <= buddy::config::model_settings::MAX_IO && out.stdout.ends_with(b"\n")
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn run(f: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(f, operation, Some(&serde_json::to_vec(input).unwrap()))
}
fn prepare_request(f: &Fixture, changes: Value) -> Value {
    let view = run_bytes(f, "inspect", None);
    assert_eq!(view["status"], "ok");
    json!({"model_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":view["data"]["base_sha256"],"config_path_sha256":view["data"]["config_path_sha256"],"changes":changes})
}
fn prepare(f: &Fixture, changes: Value) -> Value {
    let result = run(f, "prepare", &prepare_request(f, changes.clone()));
    assert_eq!(result["outcome"], "prepared");
    json!({"intent":result["data"]["intent"],"changes":changes})
}

#[test]
fn actual_private_opaque_set_clear_noop_and_patchless_status() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let old = fs::read(f.root.join("config.json")).unwrap();
    let before: Value = serde_json::from_slice(&old).unwrap();
    let changes = json!({"model":{"action":"set","value":" PRIVATE_MODEL\r\n 模型 "},"base_url":{"action":"set","value":"https://PRIVATE_USER:PRIVATE_PASS@invalid/?q=PRIVATE_QUERY#PRIVATE_FRAGMENT\r\n "}});
    let prepared = prepare(&f, changes.clone());
    assert!(prepared["intent"].get("changes").is_none());
    let status = json!({"intent":prepared["intent"]});
    assert_eq!(run(&f, "status", &status)["outcome"], "unresolved");
    let result = run(&f, "apply", &prepared);
    assert_eq!(result["outcome"], "verified");
    assert_eq!(result["disk_readback_verified"], true);
    let saved = fs::read(f.root.join("config.json")).unwrap();
    let after: Value = serde_json::from_slice(&saved).unwrap();
    for (k, v) in before.as_object().unwrap() {
        if k != "model" && k != "base_url" {
            assert_eq!(&after[k], v);
        }
    }
    assert_eq!(after["model"], changes["model"]["value"]);
    assert_eq!(after["base_url"], changes["base_url"]["value"]);
    assert_eq!(
        run(&f, "status", &status)["outcome"],
        "desired-file-observed"
    );
    assert_eq!(run(&f, "apply", &prepared)["status"], "conflict");
    let noop = run(&f, "prepare", &prepare_request(&f, changes));
    assert_eq!(noop["outcome"], "no-change");
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), saved);
    let clear = prepare(
        &f,
        json!({"model":{"action":"clear"},"base_url":{"action":"clear"}}),
    );
    assert_eq!(run(&f, "apply", &clear)["outcome"], "verified");
}
#[test]
fn actual_model_settings_existing_ordinary_and_selected_all_modes_keep_storage_byte_exact() {
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
            let intent = prepare(
                &f,
                json!({"model":{"action":"set","value":"PRIVATE_MODEL"}}),
            );
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
fn actual_args_reject_runtime_other_maintenance_and_unsafe_input() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    for extra in [
        vec!["--model", "PRIVATE_MODEL"],
        vec!["--basic-settings-operation", "inspect"],
        vec!["--restore-operation", "status", "--restore-input", "-"],
    ] {
        let mut cmd = command(&f);
        cmd.args(["--model-settings-operation", "inspect"])
            .args(extra);
        assert!(!output(cmd, None).status.success());
    }
    let mut cmd = command(&f);
    cmd.args([
        "--model-settings-operation",
        "prepare",
        "--model-settings-input",
        "some-file.json",
    ]);
    let out = output(cmd, None);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "unsupported"
    );
    for input in [
        br#"[]"#.as_slice(),
        br#"{"intent":[],"changes":{}}"#,
        br#"{"model_settings_version":1,"model_settings_version":1}"#,
    ] {
        assert_eq!(
            run_bytes(&f, "apply", Some(input))["status"],
            "invalid-input"
        );
    }
}
