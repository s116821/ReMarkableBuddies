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
            panic!("research settings CLI exceeded caller deadline");
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
    cmd.args(["--research-settings-operation", operation]);
    if input.is_some() {
        cmd.args(["--research-settings-input", "-"]);
    }
    let out = output(cmd, input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(
        out.stdout.len() <= buddy::config::research_settings::MAX_IO && out.stdout.ends_with(b"\n")
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("123E4567-E89B-12D3-A456-426614174000"));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn run(f: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(f, operation, Some(&serde_json::to_vec(input).unwrap()))
}

fn patch(value: &str) -> Value {
    json!({"research_policy":{"action":"set","value":value}})
}
fn prepared(f: &Fixture, changes: &Value) -> Value {
    let view = run_bytes(f, "inspect", None);
    let result = run(
        f,
        "prepare",
        &json!({"research_settings_version":1,"operation":Uuid::new_v4(),
        "base_sha256":view["data"]["base_sha256"],"config_path_sha256":view["data"]["config_path_sha256"],"changes":changes}),
    );
    assert_eq!(result["outcome"], "prepared");
    json!({"intent":result["data"]["intent"],"changes":changes})
}

#[test]
fn actual_cli_saves_reloads_clears_and_preserves_noop_bytes() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let before: Value =
        serde_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        run_bytes(&f, "inspect", None)["data"]["current"]["research_policy"],
        json!({"origin":"default","value":"required"})
    );
    let input = prepared(&f, &patch("disabled"));
    let saved = run(&f, "apply", &input);
    assert_eq!(saved["outcome"], "verified");
    assert_eq!(saved["runtime_activation"], "unverified");
    assert_eq!(saved["disk_readback_verified"], true);
    assert_eq!(
        run(&f, "status", &json!({"intent":input["intent"]}))["outcome"],
        "desired-file-observed"
    );
    let cfg = buddy::config::Config::load(Some(f.root.join("config.json"))).unwrap();
    assert_eq!(
        cfg.research_policy_snapshot().unwrap().policy(),
        buddy::conversation::receiving::memory::research::ResearchPolicy::Disabled
    );
    let bytes = fs::read(f.root.join("config.json")).unwrap();
    let view = run_bytes(&f, "inspect", None);
    let noop = run(
        &f,
        "prepare",
        &json!({"research_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":view["data"]["base_sha256"],"config_path_sha256":view["data"]["config_path_sha256"],"changes":patch("disabled")}),
    );
    assert_eq!(noop["outcome"], "no-change");
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), bytes);
    let clear = prepared(&f, &json!({"research_policy":{"action":"clear"}}));
    assert_eq!(run(&f, "apply", &clear)["outcome"], "verified");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(f.root.join("config.json")).unwrap()).unwrap(),
        before
    );
}

#[test]
fn actual_cli_retains_existing_ordinary_selected_and_all_mode_custody() {
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
            let before = f.inventory();
            let input = prepared(&f, &patch("disabled"));
            let owner = Store::open(f.paths()).unwrap();
            assert_eq!(run(&f, "apply", &input)["status"], "busy");
            drop(owner);
            let saved = run(&f, "apply", &input);
            assert_eq!(saved["outcome"], "verified");
            assert_eq!(saved["runtime_activation"], "unverified");
            assert_eq!(f.inventory(), before);
        }
    }
}

#[test]
fn actual_cli_strict_input_conflicts_and_missing_store_never_start_runtime() {
    let f = Fixture::new();
    let input = prepared(&f, &patch("disabled"));
    assert_eq!(run(&f, "apply", &input)["status"], "unavailable");
    assert!(!f.paths().data.exists());
    for other in [
        vec!["--configuration-operation", "inspect"],
        vec!["--memory-settings-operation", "inspect"],
        vec!["--model", "PRIVATE_MODEL"],
        vec!["--inspect-source-cache"],
        vec!["--simulate", "missing-fixture.json"],
    ] {
        let mut cmd = command(&f);
        cmd.args(["--research-settings-operation", "inspect"])
            .args(other);
        let out = output(cmd, None);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }
    let mut cmd = command(&f);
    cmd.args([
        "--research-settings-operation",
        "apply",
        "--research-settings-input",
        "not-a-file",
    ]);
    let out = output(cmd, None);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "unsupported"
    );
    let mut cmd = command(&f);
    cmd.args([
        "--research-settings-operation",
        "inspect",
        "--research-settings-input",
        "-",
    ]);
    let out = output(cmd, Some(b"PRIVATE_INPUT"));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    let oversized = vec![b' '; 16385];
    assert_eq!(
        run_bytes(&f, "prepare", Some(&oversized))["status"],
        "invalid-input"
    );
    assert!(!f.paths().data.exists());
}
