//! Actual early refusals only. Every Linux case stops before the real observer.
use remarkable_reader_buddy::storage::{digest, Uuid};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "buddy-conversation-inventory-cli-{}",
            Uuid::new_v4()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let p = self.0.canonicalize().unwrap();
        assert!(
            p.starts_with(std::env::temp_dir().canonicalize().unwrap())
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("buddy-conversation-inventory-cli-")
        );
        fs::remove_dir_all(p).unwrap();
    }
}
fn emit(kind: &str, data: Value) {
    use std::io::Write;
    let mut b = format!("CONVERSATION_INVENTORY_CORPUS_{kind} ").into_bytes();
    b.extend(serde_json::to_vec(&data).unwrap());
    b.push(b'\n');
    std::io::stdout().lock().write_all(&b).unwrap();
}
fn spawn(f: &Fixture, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .env_clear()
        .env("REMARKABLE_BUDDIES_CONFIG", f.0.join("missing-config.json"))
        .current_dir(&f.0)
        .arg("--inspect-conversations")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn collect(mut child: Child) -> Output {
    let end = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= end {
            child.kill().unwrap();
            let out = child.wait_with_output().unwrap();
            panic!("early inventory child exceeded bound: {}", out.status);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // stdin stayed held with no EOF until after process exit.
    child.wait_with_output().unwrap()
}
fn record(case: &str, args: &[&str], out: &Output) {
    emit(
        "RECORD",
        json!({"case":case,"argv_extra":args,"stdin_held_without_eof":true,"exit_code":out.status.code(),"stdout_utf8":String::from_utf8(out.stdout.clone()).unwrap(),"stdout_sha256":digest(&out.stdout),"stderr_utf8":String::from_utf8(out.stderr.clone()).unwrap(),"stderr_sha256":digest(&out.stderr)}),
    );
}
#[test]
fn clean_inventory_executable_provenance() {
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(o.status.success());
        String::from_utf8(o.stdout).unwrap().trim().to_owned()
    };
    assert_eq!(git(&["status", "--porcelain"]), "");
    emit(
        "PROVENANCE",
        json!({"source_head":git(&["rev-parse","HEAD"]),"source_tree":git(&["rev-parse","HEAD^{tree}"]),"executable_sha256":digest(&fs::read(env!("CARGO_BIN_EXE_reader-buddy")).unwrap()),"platform":std::env::consts::OS,"scope":"actual early inventory refusals only; no real service invocation"}),
    );
}
#[test]
fn missing_configuration_and_nonlinux_refuse_without_consuming_input() {
    let f = Fixture::new();
    let out = collect(spawn(&f, &[]));
    record("parameterless-held", &[], &out);
    assert!(
        out.status.success()
            && out.stderr.is_empty()
            && out.stdout.len() <= 8192
            && out.stdout.ends_with(b"\n")
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v.as_object().unwrap().len(), 13);
    assert_eq!(v["report_version"], 1);
    assert_eq!(
        v["status"],
        if cfg!(target_os = "linux") {
            "unavailable"
        } else {
            "unsupported"
        }
    );
    assert_eq!(v["scope"], "ordinary-current-root-candidates");
    assert_eq!(v["complete"], false);
    assert!(v["candidates"].is_null() && v["candidate_count"].is_null());
    for k in [
        "eligibility_evaluated",
        "mutation_authority",
        "native_authority",
        "physical_reclamation",
    ] {
        assert_eq!(v[k], false);
    }
    assert_eq!(v["runtime_unverified"], true);
    assert_eq!(v["local_only"], true);
    assert_eq!(v["service_observation"], "unverified");
    assert!(!String::from_utf8(out.stdout)
        .unwrap()
        .contains(&f.0.to_string_lossy().to_string()));
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}
#[test]
fn recognized_conflicts_and_ordinary_syntax_exit_before_any_runtime() {
    let f = Fixture::new();
    for extra in [
        vec!["--api-key", "PRIVATE_INVENTORY_SENTINEL"],
        vec!["--model", "PRIVATE_INVENTORY_SENTINEL"],
        vec!["--simulate", "PRIVATE_INVENTORY_SENTINEL"],
        vec![
            "--conversation-operation",
            "inspect",
            "--conversation-input=-",
        ],
        vec!["--inspect-source-cache"],
        vec!["--inspect-owned-data"],
        vec!["--log-level", "trace"],
        vec!["--debug-dump"],
    ] {
        let out = collect(spawn(&f, &extra));
        record("recognized-conflict", &extra, &out);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8(out.stderr)
            .unwrap()
            .contains("PRIVATE_INVENTORY_SENTINEL"));
    }
    for extra in [
        vec!["ordinary-unexpected"],
        vec!["--inspect-conversations=true"],
    ] {
        let out = collect(spawn(&f, &extra));
        record("ordinary-parser", &extra, &out);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}
