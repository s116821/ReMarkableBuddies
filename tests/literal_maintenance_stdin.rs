//! Strict existing stdin-only entry points; empty environment, no Config or Store.
use buddy::storage::{digest, Uuid};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Lane {
    name: &'static str,
    operation: &'static str,
    input: &'static str,
    refusal: &'static str,
}
const LANES: [Lane; 5] = [
    Lane {
        name: "basic",
        operation: "--basic-settings-operation",
        input: "--basic-settings-input",
        refusal: "unsupported",
    },
    Lane {
        name: "model",
        operation: "--model-settings-operation",
        input: "--model-settings-input",
        refusal: "unsupported",
    },
    Lane {
        name: "memory-settings",
        operation: "--memory-settings-operation",
        input: "--memory-settings-input",
        refusal: "unsupported",
    },
    Lane {
        name: "ordinary-export",
        operation: "--ordinary-export-operation",
        input: "--ordinary-export-input",
        refusal: "unsupported",
    },
    Lane {
        name: "ordinary-restore",
        operation: "--restore-operation",
        input: "--restore-input",
        refusal: "invalid-input",
    },
];
const NONLITERAL: [&str; 8] = [
    "not-stdin.json",
    "./-",
    "-/",
    "-/.",
    "-\\",
    "-//",
    " -",
    "- ",
];

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("literal-stdin-{}", Uuid::new_v4()));
        assert!(path.is_absolute());
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn empty(&self) {
        assert!(fs::read_dir(&self.0).unwrap().next().is_none());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.empty();
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            fs::remove_dir(&self.0).unwrap();
        }
    }
}
fn emit(kind: &str, data: &Value) {
    let mut bytes = format!("LITERAL_MAINTENANCE_CORPUS_{kind} ").into_bytes();
    bytes.extend(serde_json::to_vec(data).unwrap());
    bytes.push(b'\n');
    std::io::stdout().lock().write_all(&bytes).unwrap();
}
fn spawn(fixture: &Fixture, lane: Lane, selector: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .env_clear()
        .current_dir(&fixture.0)
        .args([lane.operation, "prepare"])
        .arg(format!("{}={selector}", lane.input))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn collect(mut child: Child) -> (Output, bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    (child.wait_with_output().unwrap(), timed_out)
}
fn record(lane: Lane, selector: &str, output: &Output, timed_out: bool, body: Option<&str>) {
    emit(
        "RECORD",
        &json!({
            "lane":lane.name, "selector":selector, "equals_syntax":true,
            "stdin_body_utf8":body, "timed_out":timed_out,
            "exit_code":output.status.code(),
            "stdout_utf8":String::from_utf8(output.stdout.clone()).unwrap(),
            "stdout_sha256":digest(&output.stdout),
            "stderr_utf8":String::from_utf8(output.stderr.clone()).unwrap(),
            "stderr_sha256":digest(&output.stderr),
        }),
    );
}
fn report(output: &Output) -> Value {
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 16_384 && output.stdout.ends_with(b"\n"));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["report_version"], 1);
    assert_eq!(report["operation"], "prepare");
    assert_eq!(report["runtime_activation"], "unverified");
    assert_eq!(report["disk_readback_verified"], false);
    assert_eq!(report["outcome"], "not-dispatched");
    assert!(report["data"].is_null());
    report
}
fn reject_without_eof(lane: Lane) {
    let fixture = Fixture::new();
    for selector in NONLITERAL {
        let mut child = spawn(&fixture, lane, selector);
        let held_stdin = child.stdin.take().unwrap();
        // No bytes and no EOF: refusal must not attempt to consume stdin.
        let (output, timed_out) = collect(child);
        drop(held_stdin);
        record(lane, selector, &output, timed_out, None);
        assert!(
            !timed_out,
            "{} accepted nonliteral {selector:?} and waited on stdin",
            lane.name
        );
        assert_eq!(report(&output)["status"], lane.refusal);
        fixture.empty();
    }
}
#[test]
fn basic_rejects_nonliteral_before_read() {
    reject_without_eof(LANES[0]);
}
#[test]
fn model_rejects_nonliteral_before_read() {
    reject_without_eof(LANES[1]);
}
#[test]
fn memory_settings_rejects_nonliteral_before_read() {
    reject_without_eof(LANES[2]);
}
#[test]
fn ordinary_export_rejects_nonliteral_before_read() {
    reject_without_eof(LANES[3]);
}
#[test]
fn ordinary_restore_rejects_nonliteral_before_read() {
    reject_without_eof(LANES[4]);
}

#[test]
fn literal_dash_still_consumes_through_eof() {
    let fixture = Fixture::new();
    for lane in LANES {
        let mut child = spawn(&fixture, lane, "-");
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().unwrap().is_none(),
            "literal dash bypassed EOF"
        );
        drop(pipe);
        let (output, timed_out) = collect(child);
        record(lane, "-", &output, timed_out, Some("{}"));
        assert!(!timed_out);
        let value = report(&output);
        let expected = if lane.name == "ordinary-restore" {
            "unavailable"
        } else {
            "invalid-input"
        };
        assert_eq!(value["status"], expected);
        fixture.empty();
    }
}
struct NoRead;
impl Read for NoRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("nonliteral selector attempted a reader call");
    }
}
#[test]
fn export_module_rejects_before_reader_or_config() {
    use buddy::ordinary_export_maintenance::{self as entry, Operation, Status};
    for selector in NONLITERAL {
        let result = entry::run(Operation::Prepare, Path::new(selector), None, &mut NoRead);
        assert_eq!(result.status, Status::Unsupported);
    }
}
#[test]
fn restore_module_keeps_invalid_input_without_reader_or_config() {
    use buddy::restore_maintenance::{self as entry, Operation, Status};
    for selector in NONLITERAL {
        let result = entry::run(Operation::Prepare, Path::new(selector), None, &mut NoRead);
        assert!(matches!(result.status, Status::InvalidInput));
    }
}
#[test]
fn clean_host_executable_provenance() {
    let source = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let clean = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(source.status.success() && clean.status.success() && clean.stdout.is_empty());
    let exe = Path::new(env!("CARGO_BIN_EXE_reader-buddy"));
    emit(
        "PROVENANCE",
        &json!({
            "source_checkout_sha":String::from_utf8(source.stdout).unwrap().trim(),
            "executable_sha256":digest(&fs::read(exe).unwrap()),
            "scope":"Fresh held-stdin tests of five existing strict stdin-only lanes. Empty child environment and owned empty working directories; no Config file selected/read and no Store, credential, provider or tablet work."
        }),
    );
}
