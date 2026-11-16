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
fn collect(child: Child) -> (Output, bool) {
    collect_before(child, Duration::from_secs(8))
}
fn collect_before(mut child: Child, timeout: Duration) -> (Output, bool) {
    let deadline = Instant::now() + timeout;
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

fn stage_child(fixture: &Fixture, operation: &str, selector: &str, config: Option<&Path>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command.env_clear().current_dir(&fixture.0);
    if let Some(config) = config {
        command.env("REMARKABLE_BUDDIES_CONFIG", config);
    }
    command
        .args(["--archive-operation", operation])
        .arg(format!("--archive-input={selector}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn stage_report(output: Output, timed_out: bool, operation: &str) -> Value {
    assert!(!timed_out, "{operation} exceeded the harness deadline");
    assert!(output.status.success(), "{operation}: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 16_384 && output.stdout.ends_with(b"\n"));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["report_version"], 1);
    assert_eq!(report["operation"], operation);
    assert_eq!(report["activation_performed"], false);
    report
}

#[test]
fn isolated_stage_selectors_require_literal_stdin_before_reading() {
    let fixture = Fixture::new();
    for operation in [
        "receive-isolated-stage",
        "check-isolated-stage",
        "release-isolated-stage",
    ] {
        for selector in NONLITERAL {
            let mut child = stage_child(&fixture, operation, selector, None);
            let held = child.stdin.take().unwrap();
            let (output, timed_out) = collect(child);
            drop(held);
            assert_eq!(
                stage_report(output, timed_out, operation)["status"],
                "invalid-input"
            );
            fixture.empty();
        }
        if !cfg!(target_os = "linux") {
            let mut child = stage_child(&fixture, operation, "-", None);
            let held = child.stdin.take().unwrap();
            let (output, timed_out) = collect(child);
            drop(held);
            assert_eq!(
                stage_report(output, timed_out, operation)["status"],
                "unsupported"
            );
            fixture.empty();
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_stage_check_consumes_literal_input_until_eof() {
    let fixture = Fixture::new();
    let operation = "check-isolated-stage";
    let mut child = stage_child(&fixture, operation, "-", None);
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(b"{}").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    drop(pipe);
    let (output, timed_out) = collect(child);
    assert_eq!(
        stage_report(output, timed_out, operation)["status"],
        "invalid-input"
    );
    fixture.empty();
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_stage_receive_times_out_a_silent_pipe_before_any_mutation() {
    let fixture = Fixture::new();
    let operation = "receive-isolated-stage";
    let mut child = stage_child(&fixture, operation, "-", None);
    let held = child.stdin.take().unwrap();
    let (output, timed_out) = collect_before(child, Duration::from_secs(17));
    drop(held);
    assert_eq!(
        stage_report(output, timed_out, operation)["status"],
        "invalid-input"
    );
    fixture.empty();
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_stage_receive_rejects_bad_header_and_boot_without_waiting_for_eof() {
    let fixture = Fixture::new();
    let operation = "receive-isolated-stage";
    let stage = Uuid::new_v4().to_string();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let sha = "0".repeat(64);
    let control = json!({
        "version":1, "stage_id":stage, "boot_id":boot.trim(), "marker_sha256":sha,
        "inventory_sha256":sha, "object_count":0, "payload_bytes":3, "budget_ms":1000
    })
    .to_string();
    let duplicate = format!(
        "{},\"object_count\":0}}",
        control.strip_suffix('}').unwrap()
    );
    let wrong_boot = control.replace(boot.trim(), "00000000-0000-0000-0000-000000000000");
    let fraction = control.replace("\"object_count\":0", "\"object_count\":0.0");
    for invalid in [duplicate, wrong_boot, fraction, " ".repeat(4096)] {
        let mut child = stage_child(&fixture, operation, "-", None);
        let mut held = child.stdin.take().unwrap();
        writeln!(held, "{invalid}").unwrap();
        let (output, timed_out) = collect(child);
        drop(held);
        assert_eq!(
            stage_report(output, timed_out, operation)["status"],
            "invalid-input"
        );
        fixture.empty();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_stage_receive_body_deadline_retains_uncertainty_with_writer_still_open() {
    let fixture = Fixture::new();
    let staging = fixture.0.join(".remarkable-buddies-restore-stage-v1");
    let config = stage_config(&fixture);

    let operation = "receive-isolated-stage";
    let mut child = stage_child(&fixture, operation, "-", Some(&config));
    let mut held = child.stdin.take().unwrap();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let stage = Uuid::new_v4().to_string();
    let sha = "0".repeat(64);
    let header = json!({
        "version":1, "stage_id":stage, "boot_id":boot.trim(), "marker_sha256":sha,
        "inventory_sha256":sha, "object_count":0, "payload_bytes":4, "budget_ms":1500
    });
    writeln!(held, "{header}").unwrap();
    held.write_all(&2u64.to_be_bytes()).unwrap();
    held.write_all(&[0; 32]).unwrap();
    held.write_all(b"a").unwrap();
    let partial = staging
        .join(format!("restore-v1-{stage}"))
        .join("data/backup.json");
    let observe_until = Instant::now() + Duration::from_secs(1);
    while !partial.is_file() && Instant::now() < observe_until {
        std::thread::sleep(Duration::from_millis(5));
    }
    let observed_partial = partial.is_file();
    let (output, timed_out) = collect(child);
    drop(held);
    let report = stage_report(output, timed_out, operation);
    assert!(observed_partial);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["data"]["settlement"], "unconfirmed");
    assert!(report["data"]["receipt"].is_null());
    assert!(partial.is_file());
    std::fs::remove_dir_all(staging.join(format!("restore-v1-{stage}"))).unwrap();
    remove_stage_fixture(&fixture, &staging, &config);
}

#[cfg(target_os = "linux")]
fn stage_config(fixture: &Fixture) -> PathBuf {
    let saved = fixture.0.join("saved");
    fs::create_dir(&saved).unwrap();
    let config = saved.join("config.json");
    fs::write(
        &config,
        json!({"paths": {
            "data":fixture.0.join("data"),
            "cache":fixture.0.join("cache"),
            "credentials":fixture.0.join("credentials")
        }})
        .to_string(),
    )
    .unwrap();
    config
}

#[cfg(target_os = "linux")]
fn remove_stage_fixture(fixture: &Fixture, staging: &Path, config: &Path) {
    fs::remove_file(staging.join("owner")).unwrap();
    fs::remove_dir(staging).unwrap();
    fs::remove_file(config).unwrap();
    fs::remove_dir(config.parent().unwrap()).unwrap();
    fixture.empty();
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_stage_real_cli_receives_checks_and_releases_producer_package() {
    use sha2::{Digest, Sha256};

    // Exact synthetic bytes from the complete case of
    // archive_maintenance::tests::emit_isolated_producer_fixtures.
    // Producer commit efb05505bec17e086f940148442216f5bcf96f6c.
    // Producer tree a4903c9787d7134fdd6d67c897f4e3cb8a1a21e5.
    // Producer receipt SHA-256 e90a495bb2e3a8c914a301b47b96d93b5946c9644bee4c3be8b06ede7c32f8b3.
    const MEMBERS: [(&str, &[u8], &str); 5] = [
        (
            "data/backup.json",
            include_bytes!("fixtures/isolated-stage-v1/data/backup.json"),
            "a02c944050a650bf20276847a9499ed4f50456f6a9f3abb72baf505b582d6cc6",
        ),
        (
            "data/objects/44d088869fcf926920c0f9b67e1d4fd2a2fcb339b436b912407b0688f56b820e",
            include_bytes!("fixtures/isolated-stage-v1/data/objects/44d088869fcf926920c0f9b67e1d4fd2a2fcb339b436b912407b0688f56b820e"),
            "44d088869fcf926920c0f9b67e1d4fd2a2fcb339b436b912407b0688f56b820e",
        ),
        (
            "data/objects/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            include_bytes!("fixtures/isolated-stage-v1/data/objects/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "portable-configuration.json",
            include_bytes!("fixtures/isolated-stage-v1/portable-configuration.json"),
            "7b184d21a20280377c8b9d7be89ed5b12eec7730480647fe33e81a47b290f161",
        ),
        (
            "isolated-backup.json",
            include_bytes!("fixtures/isolated-stage-v1/isolated-backup.json"),
            "636903dfb8c62ec0e7883de98527a1749721bf6dbe9bcab562a566b18e8a2ee8",
        ),
    ];
    let fixture = Fixture::new();
    let config = stage_config(&fixture);
    let config_bytes = fs::read(&config).unwrap();
    let exchange = |operation, input: &[u8]| {
        let mut child = stage_child(&fixture, operation, "-", Some(&config));
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(input).unwrap();
        drop(pipe);
        let (output, timed_out) = collect(child);
        let report = stage_report(output, timed_out, operation);
        assert_eq!(report["status"], "ok", "{operation}");
        assert_eq!(report["incomplete"], false);
        report
    };
    let stage = Uuid::new_v4().to_string();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let boot = boot.trim();
    let marker_sha256 = MEMBERS[4].2;
    let object_count = (MEMBERS.len() - 3) as u32;
    let mut inventory = Sha256::new();
    inventory.update(b"remarkable-buddies-restore-stage-inventory-v1\0");
    inventory.update(object_count.to_be_bytes());
    let mut payload_bytes = 0;
    let mut body = Vec::new();
    for (name, bytes, sha) in MEMBERS {
        let hash = Sha256::digest(bytes);
        assert_eq!(format!("{hash:x}"), sha, "{name}");
        let len = bytes.len() as u64;
        inventory.update(len.to_be_bytes());
        inventory.update(hash);
        body.extend(len.to_be_bytes());
        body.extend(hash);
        body.extend_from_slice(bytes);
        payload_bytes += len;
    }
    let inventory_sha256 = format!("{:x}", inventory.finalize());
    let header = json!({
        "version":1, "stage_id":stage, "boot_id":boot, "marker_sha256":marker_sha256,
        "inventory_sha256":inventory_sha256, "object_count":object_count,
        "payload_bytes":payload_bytes, "budget_ms":12000
    });
    let mut upload = format!("{header}\n").into_bytes();
    upload.extend(body);
    let received = exchange("receive-isolated-stage", &upload);
    let receipt = &received["data"]["receipt"];
    let staging = fixture.0.join(".remarkable-buddies-restore-stage-v1");
    let root = staging.join(format!("restore-v1-{stage}"));
    let custody = receipt["custody_sha256"].as_str().unwrap();
    assert!(buddy::storage::valid_digest(custody));
    assert_eq!(
        received["data"],
        json!({"settlement":"sealed","receipt":{
            "version":1, "stage_id":stage, "boot_id":boot, "marker_sha256":marker_sha256,
            "inventory_sha256":inventory_sha256, "root":root, "custody_sha256":custody
        }})
    );
    for (name, bytes, _) in MEMBERS {
        assert_eq!(
            fs::read(root.join(name)).unwrap().as_slice(),
            bytes,
            "{name}"
        );
    }
    let request = serde_json::to_vec(receipt).unwrap();
    let checked = exchange("check-isolated-stage", &request);
    assert_eq!(
        checked["data"],
        json!({"settlement":"verified","receipt":receipt})
    );
    assert!(root.is_dir());
    let released = exchange("release-isolated-stage", &request);
    assert_eq!(
        released["data"],
        json!({"settlement":"released","receipt":receipt})
    );
    assert!(!root.exists());
    assert_eq!(fs::read(&config).unwrap(), config_bytes);
    remove_stage_fixture(&fixture, &staging, &config);
}
