//! Actual bounded CLI on synthetic host files; no tablet, Store or provider.
use remarkable_reader_buddy::storage::{digest, Uuid, MAX_RECORD};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-saved-config-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join(".env"),
            b"OPENAI_API_KEY=dotenv-secret-sentinel\n",
        )
        .unwrap();
        let config = root.join("config.json");
        Self { root, config }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .env_clear()
            .current_dir(&self.root)
            .env("REMARKABLE_BUDDIES_CONFIG", &self.config)
            .env("OPENAI_API_KEY", "environment-secret-sentinel")
            .env(
                "OPENAI_BASE_URL",
                "http://127.0.0.1:1/unreachable-secret-sentinel",
            )
            .env("RUST_LOG", "trace")
            .args(["--configuration-operation", "inspect"]);
        command
    }
    fn run(&self, bytes: &[u8]) -> (Value, Output) {
        fs::write(&self.config, bytes).unwrap();
        let output = self.command().output().unwrap();
        assert!(output.status.success(), "{:?}", output.status);
        assert!(output.stderr.is_empty());
        assert_eq!(fs::read(&self.config).unwrap(), bytes);
        let value = serde_json::from_slice(&output.stdout).unwrap();
        (value, output)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn actual_saved_config_defaults_nulls_origins_and_no_stdin_dependency() {
    let f = Fixture::new();
    fs::write(&f.config, b"{}").unwrap();
    let mut child = f
        .command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep the pipe open and empty: waiting for EOF would hang this observation.
    let stdin = child.stdin.take().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= until {
            let _ = child.kill();
            panic!("inspection read stdin");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    drop(stdin);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(report["runtime_activation"], "unverified");
    assert_eq!(report["snapshot"]["config_sha256"], digest(b"{}"));
    let fields = report["snapshot"]["fields"].as_object().unwrap();
    assert_eq!(fields.len(), 25);
    assert!(fields.values().all(|v| v["origin"] == "default"));
    assert_eq!(fields["sync.max_media_bytes"]["value"], 33554432);
    assert_eq!(fields["paths.credentials"]["value"], true);
    assert_eq!(fields["debug_dump"]["value"], Value::Null);
    let (report, _) = f.run(br#"{"model":null,"debug_dump":null,"memory":{"profile":null},"sync":{},"personalization":{"text":""}}"#);
    let fields = &report["snapshot"]["fields"];
    assert_eq!(
        fields["model"],
        json!({"origin":"declared-null","value":false})
    );
    assert_eq!(
        fields["debug_dump"],
        json!({"origin":"declared-null","value":null})
    );
    assert_eq!(fields["memory.profile"]["origin"], "declared-null");
    assert_eq!(fields["sync.enabled"]["origin"], "default");
    assert_eq!(
        fields["personalization.text"],
        json!({"origin":"declared","value":false})
    );
    assert_eq!(
        fields["personalization.text_bytes"],
        json!({"origin":"declared","value":0})
    );
}

#[test]
fn actual_saved_config_projects_private_values_without_root_or_credential_admission() {
    let f = Fixture::new();
    let credential = f.root.join("credential-sentinel");
    fs::write(&credential, b"protected-contents-sentinel").unwrap();
    // Forbidden/nonexistent/overlapping declarations would fail Config::validate.
    // The inspection must remain pure declaration/value validation instead.
    let profile = "a138b641-a4aa-4ac3-b1d1-091ba27f1470";
    let group = "b2f1cc78-a8d8-4dc1-a3ce-a40cdff053ba";
    let collection = "c173dd73-907b-44d6-a08b-402924459b2f";
    let binding = "a5".repeat(32);
    let config = json!({"memory":{"profile":profile,"selection":{"group":group,"profile":profile,"binding_sha256":binding}},"model":"model-secret-sentinel","base_url":"https://user:password-sentinel@invalid/?token=query-sentinel#fragment-sentinel","model_credential":"protected-reference-sentinel","paths":{"data":"/proc/path-secret-sentinel","cache":"/proc/path-secret-sentinel","credentials":credential},"trigger_corner":"LL","log_level":"trace","debug_dump":true,"sync":{"collection":collection,"create_new":true,"namespaces":["conversation","source","export-association","subject-memory","handwriting"],"max_media_bytes":268435456},"personalization":{"text":"personal-text-secret-sentinel\u{03b1}","enabled":true}});
    let (report, output) = f.run(&serde_json::to_vec(&config).unwrap());
    assert_eq!(report["status"], "ok");
    assert_eq!(report["snapshot"]["declared_roots_validated"], false);
    assert_eq!(report["snapshot"]["validation_basis"], "typed-values-only");
    assert_eq!(
        fs::read(&credential).unwrap(),
        b"protected-contents-sentinel"
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for sentinel in [
        "model-secret",
        "password-sentinel",
        "query-sentinel",
        "fragment-sentinel",
        "protected-reference",
        "path-secret",
        "credential-sentinel",
        "protected-contents",
        "personal-text-secret",
        "environment-secret",
        "dotenv-secret",
    ] {
        assert!(!text.contains(sentinel), "leaked {sentinel}");
    }
    for private in [profile, group, collection, binding.as_str()] {
        assert!(!text.contains(private));
    }
    let fields = &report["snapshot"]["fields"];
    assert_eq!(fields["memory.selection"]["value"], true);
    assert_eq!(
        fields["sync.namespaces"]["value"],
        json!([
            "conversation",
            "export-association",
            "handwriting",
            "source",
            "subject-memory"
        ])
    );
    assert_eq!(fields["sync.max_media_bytes"]["value"], 268435456_u64);
    for field in ["paths.data", "paths.cache", "paths.credentials"] {
        assert_eq!(fields[field], json!({"origin":"declared","value":true}));
    }
    assert_eq!(
        fields["personalization.text_bytes"]["value"],
        "personal-text-secret-sentinel\u{03b1}".len()
    );
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 3);
}

#[test]
fn actual_saved_config_refusals_are_whole_sanitized_and_bounded() {
    let f = Fixture::new();
    for (input, status) in [
        (
            br#"{"model":null,"model":null}"#.as_slice(),
            "invalid-input",
        ),
        (
            br#"{"memory":{"profile":null,"profile":null}}"#,
            "invalid-input",
        ),
        (
            br#"{"sync":{"namespaces":["source","source"]}}"#,
            "invalid-input",
        ),
        (br#"{"unknown-secret-sentinel":"secret"}"#, "invalid-input"),
        (
            br#"{"personalization":{"unknown-secret-sentinel":true}}"#,
            "invalid-input",
        ),
        (br#"{"config_schema_version":2}"#, "unsupported"),
        (
            br#"{"personalization":{"schema_version":2}}"#,
            "unsupported",
        ),
        (
            br#"{"sync":{"max_media_bytes":268435457}}"#,
            "invalid-input",
        ),
        (br#"{"memory":{"enabled":true}}"#, "invalid-input"),
        (br#"{"paths":{"data":"/unrelated"}}"#, "invalid-input"),
        (br#"{"debug_dump":"secret-sentinel"}"#, "invalid-input"),
        (br#"{"sync":{"poll_seconds":4}}"#, "invalid-input"),
        (b"\xff", "invalid-input"),
        (b"{} {}", "invalid-input"),
    ] {
        let (report, output) = f.run(input);
        assert_eq!(report["status"], status);
        assert_eq!(report["snapshot"], Value::Null);
        assert!(!String::from_utf8(output.stdout).unwrap().contains("secret"));
    }
    let (report, _) = f.run(&vec![b' '; MAX_RECORD + 1]);
    assert_eq!(report["status"], "unavailable");
    fs::remove_file(&f.config).unwrap();
    for mut command in [
        f.command(),
        {
            let mut c = f.command();
            c.env_remove("REMARKABLE_BUDDIES_CONFIG");
            c
        },
        {
            let mut c = f.command();
            c.env("REMARKABLE_BUDDIES_CONFIG", &f.root);
            c
        },
    ] {
        let output = command.output().unwrap();
        assert!(output.status.success());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "unavailable");
        assert_eq!(report["snapshot"], Value::Null);
    }
}

#[test]
fn actual_saved_config_cli_rejects_mixed_runtime_and_maintenance_before_io() {
    let f = Fixture::new();
    for args in [
        vec!["--api-key", "secret-sentinel"],
        vec!["--model", "model"],
        vec!["--base-url", "url"],
        vec!["--trigger-corner", "LL"],
        vec!["--log-level", "trace"],
        vec!["--debug-dump"],
        vec!["--simulate", "absent.json"],
        vec!["--inspect-owned-data"],
        vec!["--export-note"],
        vec!["--personalization-operation", "inspect"],
        vec!["--memory-operation", "query", "--memory-input", "-"],
        vec!["--sync-mode-operation", "inspect"],
        vec!["--archive-operation", "inspect", "--archive-input", "-"],
    ] {
        let output = f.command().args(&args).output().unwrap();
        assert!(!output.status.success(), "accepted {args:?}");
        assert!(output.stdout.is_empty());
    }
    assert!(!f.config.exists());
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn actual_saved_config_refuses_a_link_selected_file() {
    let f = Fixture::new();
    let target = f.root.join("actual.json");
    fs::write(&target, b"{}").unwrap();
    std::os::unix::fs::symlink(&target, &f.config).unwrap();
    let output = f.command().output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["snapshot"], Value::Null);
    assert_eq!(fs::read(&target).unwrap(), b"{}");
}
