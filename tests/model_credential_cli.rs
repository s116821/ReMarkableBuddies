//! Actual Linux CLI over task-owned synthetic credentials. Never reads real keys.
#![cfg(target_os = "linux")]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use buddy::storage::{digest, sync::mode::SyncModeChoice, Store, Uuid};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;
const SECRET: &str = "SYNTHETIC_CREDENTIAL\r\n trailing α ";
fn emit(kind: &str, value: &Value) {
    let mut b = format!("MODEL_CREDENTIAL_CORPUS_{kind} ").into_bytes();
    b.extend(serde_json::to_vec(value).unwrap());
    b.push(b'\n');
    std::io::stdout().lock().write_all(&b).unwrap();
}
fn configure(f: &Fixture) {
    let p = f.root.join("config.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    v["model_credential"] = json!("fixture");
    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
}
fn command(f: &Fixture) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    c.env_clear()
        .current_dir(&f.root)
        .env("REMARKABLE_BUDDIES_CONFIG", f.root.join("config.json"))
        .env("OPENAI_API_KEY", "SYNTHETIC_ENV_NOT_USED")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1/not-used")
        .env("RUST_LOG", "trace");
    c
}
fn capture(
    records: &mut Vec<Value>,
    name: &str,
    f: &Fixture,
    op: &str,
    input: Option<Value>,
    status: &str,
    outcome: &str,
) -> Value {
    let mut cmd = command(f);
    cmd.args(["--model-credential-operation", op]);
    if input.is_some() {
        cmd.args(["--model-credential-input", "-"]);
    }
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let bytes = input.map(|v| serde_json::to_vec(&v).unwrap());
    let before = f.inventory();
    let config = fs::read(f.root.join("config.json")).unwrap();
    let mut child = cmd.spawn().unwrap();
    let writer = bytes.as_ref().map(|b| {
        let mut pipe = child.stdin.take().unwrap();
        let b = b.clone();
        std::thread::spawn(move || pipe.write_all(&b))
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("credential CLI exceeded caller deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(w) = writer {
        if let Err(e) = w.join().unwrap() {
            assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= 16384 && out.stdout.ends_with(b"\n"));
    let text = String::from_utf8(out.stdout.clone()).unwrap();
    assert!(!text.contains("SYNTHETIC_CREDENTIAL"));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["status"], status, "{name}");
    assert_eq!(value["outcome"], outcome, "{name}");
    assert_eq!(value["runtime_activation"], "unverified");
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), config);
    let target = f.paths().credentials.join("fixture");
    let mut after = f.inventory();
    if op == "apply" && status == "ok" {
        let mut before = before;
        before.remove(&target);
        after.remove(&target);
        assert_eq!(before, after);
    } else {
        assert_eq!(before, after);
    }
    let record = json!({"name":name,"operation":op,"stdin_base64":bytes.as_ref().map(|b|STANDARD.encode(b)),"stdin_sha256":bytes.as_ref().map(|b|digest(b)),"stdout_utf8":text,"stdout_sha256":digest(&out.stdout),"exit_code":out.status.code(),"stderr_utf8":"","config_sha256":digest(&config),"observed_credential_sha256":fs::read(&target).ok().map(|b|digest(&b)),"observed_credential_mode":fs::metadata(&target).ok().map(|m|m.permissions().mode()&0o7777)});
    emit("RECORD", &record);
    records.push(record);
    value
}
fn prepare(records: &mut Vec<Value>, name: &str, f: &Fixture, change: Value) -> Value {
    let view = capture(
        records,
        &format!("{name}-inspect"),
        f,
        "inspect",
        None,
        "ok",
        "read-only",
    );
    let p = capture(
        records,
        &format!("{name}-prepare"),
        f,
        "prepare",
        Some(
            json!({"credential_version":1,"operation":Uuid::new_v4(),"base":view["data"]["binding"],"change":change}),
        ),
        "ok",
        "prepared",
    );
    json!({"intent":p["data"]["intent"],"change":change})
}
#[test]
fn actual_synthetic_cli_modes_busy_exact_bytes_and_clean_build_corpus() {
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_reader-buddy"));
    let exe_sha = digest(&fs::read(exe).unwrap());
    let source = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(source.status.success());
    let clean = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(clean.status.success() && clean.stdout.is_empty());
    let provenance = json!({"source_checkout_sha":String::from_utf8(source.stdout).unwrap().trim(),"executable_sha256":exe_sha,"scope":"Actual Linux host CLI against only task-owned synthetic credentials; no provider/tablet/runtime or transport-loss test."});
    emit("START", &provenance);
    let mut records = Vec::new();
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    configure(&f);
    for (action, value) in [
        ("install", Some(SECRET)),
        ("replace", Some("SYNTHETIC_CREDENTIAL_REPLACEMENT\r\n α ")),
        ("remove", None),
    ] {
        if action == "replace" {
            fs::set_permissions(
                f.paths().credentials.join("fixture"),
                fs::Permissions::from_mode(0o400),
            )
            .unwrap();
        }
        let change = match value {
            Some(s) => json!({"action":action,"value":s}),
            None => json!({"action":action}),
        };
        let input = prepare(&mut records, action, &f, change);
        capture(
            &mut records,
            &format!("{action}-status-before"),
            &f,
            "status",
            Some(json!({"intent":input["intent"]})),
            "ok",
            "unresolved",
        );
        capture(
            &mut records,
            &format!("{action}-apply"),
            &f,
            "apply",
            Some(input.clone()),
            "ok",
            "verified",
        );
        capture(
            &mut records,
            &format!("{action}-status-after-verified"),
            &f,
            "status",
            Some(json!({"intent":input["intent"]})),
            "ok",
            "desired-file-observed",
        );
        if let Some(s) = value {
            assert_eq!(
                fs::read(f.paths().credentials.join("fixture")).unwrap(),
                s.as_bytes()
            );
        } else {
            assert!(!f.paths().credentials.join("fixture").exists());
        }
    }
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
                    binding_sha256: digest(b"synthetic isolated"),
                }),
                _ => Some(SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: digest(b"synthetic shared"),
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
            configure(&f);
            let name = format!("selected-{selected}-mode-{mode}");
            let input = prepare(
                &mut records,
                &name,
                &f,
                json!({"action":"install","value":SECRET}),
            );
            let owner = Store::open(f.paths()).unwrap();
            capture(
                &mut records,
                &format!("{name}-busy"),
                &f,
                "apply",
                Some(input.clone()),
                "busy",
                "not-dispatched",
            );
            capture(
                &mut records,
                &format!("{name}-status-while-busy"),
                &f,
                "status",
                Some(json!({"intent":input["intent"]})),
                "ok",
                "unresolved",
            );
            drop(owner);
            capture(
                &mut records,
                &format!("{name}-apply"),
                &f,
                "apply",
                Some(input.clone()),
                "ok",
                "verified",
            );
            capture(
                &mut records,
                &format!("{name}-status-after-verified"),
                &f,
                "status",
                Some(json!({"intent":input["intent"]})),
                "ok",
                "desired-file-observed",
            );
            assert_eq!(
                fs::read(f.paths().credentials.join("fixture")).unwrap(),
                SECRET.as_bytes()
            );
        }
    }
    assert_eq!(digest(&fs::read(exe).unwrap()), exe_sha);
    emit(
        "END",
        &json!({"provenance":provenance,"records":records.len(),"executable_sha256":exe_sha}),
    );
}
