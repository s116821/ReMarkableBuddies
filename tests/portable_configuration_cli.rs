//! Real CLI over explicit owned preference files; no Store/provider/device.
use serde_json::{json, Value};
#[cfg(unix)]
use std::io::Write;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
fn run(selector: &str, path: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .arg(selector)
        .arg(path)
        .env(
            "REMARKABLE_BUDDIES_CONFIG",
            path.with_file_name("must-not-read.json"),
        )
        .env("OPENAI_API_KEY", "PRIVATE_UNUSED_CREDENTIAL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Deliberately retain an open stdin with no EOF: these selectors never read it.
    let _open_stdin = child.stdin.take();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("offline selector blocked");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 128 * 1024);
    assert!(output.stdout.ends_with(b"\n"));
    assert_eq!(output.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn export_then_inspect_without_source_roots_stdin_or_environment_fallback() {
    let dir = Temp::new();
    let source = dir.path().join("source.json");
    let artifact = dir.path().join("artifact.json");
    let settings = json!({"product_settings":{"reader_enabled":false},"personalization":{"text":"PRIVATE 複数行\n😀","enabled":false},"research_policy":"disabled"});
    let mut config = settings.clone();
    config["paths"] = json!({"data":dir.path().join("missing-data"),"cache":dir.path().join("missing-cache"),"credentials":dir.path().join("missing-credentials")});
    let bytes = serde_json::to_vec(&config).unwrap();
    fs::write(&source, &bytes).unwrap();
    let exported = report(run("--export-portable-configuration", &source));
    assert_eq!(exported["status"], "ok");
    assert_eq!(exported["artifact"]["settings"], settings);
    fs::write(
        &artifact,
        serde_json::to_vec_pretty(&exported["artifact"]).unwrap(),
    )
    .unwrap();
    fs::remove_file(&source).unwrap();
    let inspected = report(run("--inspect-portable-configuration", &artifact));
    assert_eq!(inspected["summary"], exported["summary"]);
    assert!(inspected["artifact"].is_null());
    assert!(!inspected.to_string().contains("PRIVATE"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    // Restoring the source proves the exact source input, not configured roots, is read.
    fs::write(&source, &bytes).unwrap();
    assert_eq!(
        report(run("--export-portable-configuration", &source))["artifact"],
        exported["artifact"]
    );
    assert_eq!(fs::read(source).unwrap(), bytes);
}
#[test]
fn selectors_are_exclusive_and_refusals_do_not_emit_partial_artifacts() {
    let dir = Temp::new();
    let source = dir.path().join("source.json");
    fs::write(&source, b"{}").unwrap();
    for other in [
        "--configuration-schema",
        "--inspect-owned-data-v3",
        "--inspect-selected-outbox",
        "--debug-dump",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .arg("--export-portable-configuration")
            .arg(&source)
            .arg(other)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    for bytes in [
        br#"{"personalization":{"text":"x","text":"y"}}"#.as_slice(),
        br#"{"personalization":{"schema_version":2}}"#,
        br#"{"log_level":"unknown"}"#,
    ] {
        fs::write(&source, bytes).unwrap();
        let value = report(run("--export-portable-configuration", &source));
        assert_ne!(value["status"], "ok");
        assert!(value["artifact"].is_null());
        assert!(value["summary"].is_null());
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
    assert_eq!(
        report(run("--export-portable-configuration", Path::new("-")))["status"],
        "unavailable"
    );
}
#[cfg(unix)]
#[test]
fn special_inputs_refuse_before_blocking_open_or_following_symlinks() {
    let dir = Temp::new();
    let fifo = dir.path().join("fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    for selector in [
        "--export-portable-configuration",
        "--inspect-portable-configuration",
    ] {
        assert_eq!(report(run(selector, &fifo))["status"], "unavailable");
    }
    let source = dir.path().join("source");
    fs::File::create(&source).unwrap().write_all(b"{}").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert_eq!(
        report(run("--export-portable-configuration", &link))["status"],
        "unavailable"
    );
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buddy-portable-config-{}",
            remarkable_reader_buddy::storage::Uuid::new_v4()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
