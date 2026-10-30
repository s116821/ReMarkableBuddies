//! Actual host CLI on existing ordinary/selected synthetic Stores; no tablet/provider.
use buddy::storage::{digest, sync::mode::SyncModeChoice, Store, Uuid};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    process::{Command, Output, Stdio},
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
            panic!("ordinary export CLI exceeded caller deadline");
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
    cmd.args(["--ordinary-export-operation", operation]);
    if input.is_some() {
        cmd.args(["--ordinary-export-input", "-"]);
    }
    let out = output(cmd, input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(
        out.stdout.len() <= buddy::ordinary_export_maintenance::MAX_IO
            && out.stdout.ends_with(b"\n")
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn run(f: &Fixture, operation: &str, input: &Value) -> Value {
    run_bytes(f, operation, Some(&serde_json::to_vec(input).unwrap()))
}

fn prepare(f: &Fixture, dest: &std::path::Path) -> Value {
    let r = run(
        f,
        "prepare",
        &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest}),
    );
    assert_eq!(r["outcome"], "prepared");
    json!({"destination":dest,"intent":r["data"]["intent"]})
}
#[test]
fn actual_private_snapshot_preservation_verified_export_and_changed_path_restore_round_trip() {
    let f = Fixture::new();
    let store = Store::open(f.paths()).unwrap();
    let config:buddy::config::Config=serde_json::from_value(json!({"paths":f.paths(),"model":"PRIVATE_MODEL\r\n 模型 ","base_url":"https://PRIVATE_USER:PRIVATE_PASS@invalid/?q=PRIVATE_QUERY#PRIVATE_FRAGMENT","model_credential":"PRIVATE_REFERENCE","personalization":{"text":"PRIVATE_TEXT\r\n α "}})).unwrap();
    store.snapshot_config(&config).unwrap();
    fs::write(f.paths().cache.join("keep-cache"), b"PRIVATE_CACHE").unwrap();
    fs::write(
        f.paths().credentials.join("keep-credential"),
        b"PRIVATE_CREDENTIAL",
    )
    .unwrap();
    fs::write(f.root.join("original.pdf"), b"PRIVATE_NATIVE_ORIGINAL").unwrap();
    drop(store);
    let before = f.inventory();
    let dest = f.root.join("export");
    let input = prepare(&f, &dest);
    assert_eq!(run(&f, "status", &input)["outcome"], "unresolved");
    assert_eq!(run(&f, "apply", &input)["outcome"], "verified");
    assert_eq!(f.inventory(), before);
    let marker: Value =
        serde_json::from_slice(&fs::read(dest.join("backup.json")).unwrap()).unwrap();
    assert_eq!(marker["format"], 1);
    assert_eq!(marker["config_snapshot"]["model"], config.model.unwrap());
    assert_eq!(
        fs::read(f.root.join("original.pdf")).unwrap(),
        b"PRIVATE_NATIVE_ORIGINAL"
    );
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 2);
    let owner = Store::open_existing(f.paths()).unwrap();
    assert_eq!(
        run(&f, "status", &input)["outcome"],
        "desired-archive-observed"
    );
    drop(owner);
    let target = Fixture::new();
    drop(Store::open(target.paths()).unwrap());
    let invoke = |operation: &str, value: &Value| {
        let mut c = command(&target);
        c.args(["--restore-operation", operation, "--restore-input", "-"]);
        let out = output(c, Some(&serde_json::to_vec(value).unwrap()));
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let r = invoke(
        "prepare",
        &json!({"restore_version":1,"operation":Uuid::new_v4(),"source":dest}),
    );
    assert_eq!(r["outcome"], "prepared");
    let r = invoke(
        "apply",
        &json!({"source":dest,"intent":r["data"]["intent"]}),
    );
    assert_eq!(r["outcome"], "restored");
    assert_eq!(f.inventory(), before);
    fs::remove_file(f.root.join("config.json")).unwrap();
    fs::rename(f.paths().data, f.root.join("moved-data")).unwrap();
    assert_eq!(
        run(&f, "status", &input)["outcome"],
        "desired-archive-observed"
    );
}
#[test]
fn actual_ordinary_modes_busy_selected_and_shared_refusals_keep_source_exact() {
    for mode in 0..4 {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        let choice = match mode {
            0 => None,
            1 => Some(SyncModeChoice::LocalOnly),
            2 => Some(SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"fixture"),
            }),
            _ => Some(SyncModeChoice::SharedGroup {
                group: Uuid::new_v4(),
                binding_sha256: digest(b"fixture"),
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
        let dest = f.root.join("export");
        let req = json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest});
        let p = run(&f, "prepare", &req);
        if mode < 2 {
            assert_eq!(p["outcome"], "prepared");
            let input = json!({"destination":dest,"intent":p["data"]["intent"]});
            let owner = Store::open_existing(f.paths()).unwrap();
            assert_eq!(run(&f, "apply", &input)["status"], "busy");
            drop(owner);
            assert_eq!(run(&f, "apply", &input)["outcome"], "verified");
        } else {
            assert_eq!(p["status"], "unsupported");
            assert!(!dest.exists());
        }
        assert_eq!(f.inventory(), before);
    }
    let f = Fixture::new();
    drop(f.selected(false));
    let before = f.inventory();
    let dest = f.root.join("selected-refused");
    assert_eq!(
        run(
            &f,
            "prepare",
            &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest})
        )["status"],
        "unsupported"
    );
    assert!(!dest.exists());
    assert_eq!(f.inventory(), before);
}
#[test]
fn actual_strict_input_flags_caps_existing_parent_and_no_initialization() {
    let f = Fixture::new();
    let req =
        json!({"export_version":1,"operation":Uuid::new_v4(),"destination":f.root.join("export")});
    assert_ne!(run(&f, "prepare", &req)["status"], "ok");
    assert!(!f.paths().data.exists());
    assert!(!f.root.join("export").exists());
    drop(Store::open(f.paths()).unwrap());
    let mut bytes = serde_json::to_vec(&req).unwrap();
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
    for raw in [
        br#"[]"#.as_slice(),
        br#"{"intent":[]}"#,
        br#"{"export_version":1,"export_version":1}"#,
    ] {
        assert_eq!(run_bytes(&f, "apply", Some(raw))["status"], "invalid-input");
    }
    for extra in [
        vec!["--model", "PRIVATE_MODEL"],
        vec!["--archive-operation", "inspect", "--archive-input", "-"],
        vec!["--model-settings-operation", "inspect"],
    ] {
        let mut cmd = command(&f);
        cmd.args([
            "--ordinary-export-operation",
            "prepare",
            "--ordinary-export-input",
            "-",
        ])
        .args(extra);
        assert!(!output(cmd, Some(&serde_json::to_vec(&req).unwrap()))
            .status
            .success());
    }
    let mut cmd = command(&f);
    cmd.args([
        "--ordinary-export-operation",
        "prepare",
        "--ordinary-export-input",
        "not-a-file.json",
    ]);
    let out = output(cmd, None);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "unsupported"
    );
}
#[cfg(unix)]
#[test]
fn actual_symlink_destination_parent_is_refused_without_creation() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    let alias = f.root.join("alias");
    std::os::unix::fs::symlink(f.paths().data, &alias).unwrap();
    let dest = alias.join("new");
    assert_eq!(
        run(
            &f,
            "prepare",
            &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest})
        )["status"],
        "invalid-input"
    );
    assert!(!dest.exists());
}
#[cfg(windows)]
#[test]
fn actual_windows_case_and_junction_aliases_refuse_without_creation() {
    let f = Fixture::new();
    drop(Store::open(f.paths()).unwrap());
    for root in [f.paths().data, f.paths().cache, f.paths().credentials] {
        let spelling = root
            .to_str()
            .unwrap()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c
                }
            })
            .collect::<String>();
        let dest = std::path::PathBuf::from(spelling).join("alias-new");
        assert_eq!(
            run(
                &f,
                "prepare",
                &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest})
            )["status"],
            "invalid-input"
        );
        assert!(!dest.exists());
    }
    let alias = f.root.join("junction");
    let out = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&alias)
        .arg(f.paths().data)
        .output()
        .unwrap();
    assert!(out.status.success(), "junction fixture setup failed");
    let dest = alias.join("new");
    assert_eq!(
        run(
            &f,
            "prepare",
            &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest})
        )["status"],
        "invalid-input"
    );
    assert!(!dest.exists());
}
