//! Actual host CLI on existing ordinary/selected synthetic Stores; no tablet/provider.
use buddy::storage::{digest, sync::mode::SyncModeChoice, Store, Uuid};
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
            panic!("memory settings CLI exceeded caller deadline");
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
    cmd.args(["--memory-settings-operation", operation]);
    if input.is_some() {
        cmd.args(["--memory-settings-input", "-"]);
    }
    let out = output(cmd, input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(
        out.stdout.len() <= buddy::config::memory_settings::MAX_IO && out.stdout.ends_with(b"\n")
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("123E4567-E89B-12D3-A456-426614174000"));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn run(f: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(f, operation, Some(&serde_json::to_vec(input).unwrap()))
}

const ID: &str = "123E4567-E89B-12D3-A456-426614174000";
fn patch() -> Value {
    json!({"enabled":{"action":"set","value":true},"profile":{"action":"set","value":ID}})
}
fn prepare_request(f: &Fixture, changes: Value) -> Value {
    let view = run_bytes(f, "inspect", None);
    assert_eq!(view["status"], "ok");
    json!({"memory_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":view["data"]["base_sha256"],"config_path_sha256":view["data"]["config_path_sha256"],"changes":changes})
}
fn prepare(f: &Fixture, changes: Value) -> Value {
    let r = run(f, "prepare", &prepare_request(f, changes.clone()));
    assert_eq!(r["outcome"], "prepared");
    json!({"intent":r["data"]["intent"],"changes":changes})
}
#[test]
fn actual_parent_presence_exact_uuid_private_reports_and_manual_query_survive_disable() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let original = fs::read(f.root.join("config.json")).unwrap();
    let before = f.inventory();
    assert_eq!(
        run(
            &f,
            "prepare",
            &prepare_request(
                &f,
                json!({"profile":{"action":"clear"},"enabled":{"action":"clear"}})
            )
        )["outcome"],
        "no-change"
    );
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), original);
    let i = prepare(&f, patch());
    let status = json!({"intent":i["intent"]});
    assert_eq!(run(&f, "status", &status)["outcome"], "unresolved");
    let r = run(&f, "apply", &i);
    assert_eq!(r["outcome"], "verified");
    assert_eq!(r["runtime_activation"], "unverified");
    let after: Value =
        serde_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
    assert_eq!(after["memory"]["profile"], ID);
    assert_eq!(after["memory"]["enabled"], true);
    assert_eq!(f.inventory(), before);
    let i = prepare(&f, json!({"enabled":{"action":"set","value":false}}));
    let r = run(&f, "apply", &i);
    assert_eq!(r["outcome"], "verified");
    assert_eq!(r["runtime_activation"], "unverified");
    // A separate explicit old maintenance request, never dispatched by the setter.
    let mut query = command(&f);
    query.args(["--memory-operation", "query", "--memory-input", "-"]);
    let out=output(query,Some(&serde_json::to_vec(&json!({"document":null,"text":"synthetic fixture","now_ms":0,"current_claims":[],"max_matches":8,"max_context_bytes":8192})).unwrap()));
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    let q: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(q["status"], "ok");
    assert_eq!(q["data"]["query_status"], "complete-empty");
    assert_eq!(f.inventory(), before);
    let i = prepare(
        &f,
        json!({"profile":{"action":"clear"},"enabled":{"action":"clear"}}),
    );
    assert_eq!(run(&f, "apply", &i)["outcome"], "verified");
    let raw: Value =
        serde_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
    assert_eq!(raw["memory"], json!({}));
    assert_eq!(f.inventory(), before);
    let saved = fs::read(f.root.join("config.json")).unwrap();
    assert_eq!(
        run(
            &f,
            "prepare",
            &prepare_request(&f, json!({"enabled":{"action":"clear"}}))
        )["outcome"],
        "no-change"
    );
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), saved);
}
#[test]
fn actual_existing_ordinary_selected_four_modes_preserve_all_data_and_claim_no_activation() {
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
                    binding_sha256: digest(b"fixture isolated"),
                }),
                _ => Some(SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: digest(b"fixture shared"),
                    root_id: "fixture-root".into(),
                }),
            };
            if let Some(c) = choice {
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        c,
                    )
                    .unwrap();
            }
            drop(store);
            let before = f.inventory();
            let input = prepare(&f, patch());
            let owner = Store::open(f.paths()).unwrap();
            assert_eq!(run(&f, "apply", &input)["status"], "busy");
            assert_eq!(
                run(&f, "status", &json!({"intent":input["intent"]}))["status"],
                "ok"
            );
            drop(owner);
            let r = run(&f, "apply", &input);
            assert_eq!(r["outcome"], "verified");
            assert_eq!(r["runtime_activation"], "unverified");
            assert_eq!(f.inventory(), before);
            assert_eq!(
                run(&f, "status", &json!({"intent":input["intent"]}))["outcome"],
                "desired-file-observed"
            );
        }
    }
}
#[test]
fn actual_strict_conflicts_caps_uuid_types_and_uninitialized_mutation_refuse() {
    let f = Fixture::new();
    let input = prepare(&f, patch());
    assert_eq!(run(&f, "apply", &input)["status"], "unavailable");
    assert!(!f.paths().data.exists());
    drop(Store::open(f.paths()).unwrap());
    let request = prepare_request(&f, patch());
    let mut bytes = serde_json::to_vec(&request).unwrap();
    bytes.resize(16384, b' ');
    assert_eq!(
        run_bytes(&f, "prepare", Some(&bytes))["outcome"],
        "prepared"
    );
    bytes.push(b' ');
    assert_eq!(
        run_bytes(&f, "prepare", Some(&bytes))["status"],
        "invalid-input"
    );
    for invalid in [
        json!({"profile":{"action":"set","value":null}}),
        json!({"profile":{"action":"set","value":"PRIVATE_INVALID_UUID"}}),
        json!({"enabled":{"action":"set","value":null}}),
        json!({"profile":{"action":"set","value":Uuid::nil()}}),
        json!({"selection":{"action":"clear"}}),
    ] {
        assert_eq!(
            run(&f, "prepare", &prepare_request(&f, invalid))["status"],
            "invalid-input"
        );
    }
    for raw in [
        br#"[]"#.as_slice(),
        br#"{"intent":[]}"#,
        br#"{"memory_settings_version":1,"memory_settings_version":1}"#,
    ] {
        assert_eq!(run_bytes(&f, "apply", Some(raw))["status"], "invalid-input");
    }
    for extra in [
        vec!["--memory-operation", "query", "--memory-input", "-"],
        vec!["--model-settings-operation", "inspect"],
        vec!["--model", "PRIVATE_MODEL"],
    ] {
        let mut cmd = command(&f);
        cmd.args(["--memory-settings-operation", "inspect"])
            .args(extra);
        assert!(!output(cmd, None).status.success());
    }
}
#[test]
fn actual_inspect_does_not_wait_for_open_stdin_or_probe_declared_roots() {
    let f = Fixture::new();
    let mut cmd = command(&f);
    cmd.args(["--memory-settings-operation", "inspect"])
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
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["outcome"], "read-only");
    assert!(!f.paths().data.exists());
}
