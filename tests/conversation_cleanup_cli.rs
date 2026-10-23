//! Actual executable early-boundary controls. Never invokes service observation.
use remarkable_reader_buddy::storage::{digest, Uuid};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("buddy-conversation-cli-{}", Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let p = self.0.canonicalize().unwrap();
            assert!(p.starts_with(std::env::temp_dir().canonicalize().unwrap()));
            assert!(p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("buddy-conversation-cli-"));
            fs::remove_dir_all(p).unwrap();
        }
    }
}
fn emit(kind: &str, data: Value) {
    let mut b = format!("CONVERSATION_CLEANUP_CORPUS_{kind} ").into_bytes();
    b.extend(serde_json::to_vec(&data).unwrap());
    b.push(b'\n');
    std::io::stdout().lock().write_all(&b).unwrap();
}
fn spawn(f: &Fixture, op: &str, selector: &str, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .env_clear()
        .env("REMARKABLE_BUDDIES_CONFIG", f.0.join("missing-config.json"))
        .current_dir(&f.0)
        .args(["--conversation-operation", op])
        .arg(format!("--conversation-input={selector}"))
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn collect(mut c: Child) -> Output {
    let end = Instant::now() + Duration::from_secs(8);
    while c.try_wait().unwrap().is_none() {
        if Instant::now() >= end {
            c.kill().unwrap();
            let output = c.wait_with_output().unwrap();
            panic!("synthetic child exceeded bound: {}", output.status)
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    c.wait_with_output().unwrap()
}
fn record(case: &str, op: &str, selector: &str, body: Option<&[u8]>, extra: &[&str], out: &Output) {
    emit(
        "RECORD",
        json!({"case":case,"operation":op,"selector":selector,"stdin_held_without_eof":body.is_none(),"stdin_utf8":body.map(|b|String::from_utf8(b.to_vec()).unwrap()),"stdin_sha256":body.map(digest),"argv_extra":extra,"exit_code":out.status.code(),"stdout_utf8":String::from_utf8(out.stdout.clone()).unwrap(),"stdout_sha256":digest(&out.stdout),"stderr_utf8":String::from_utf8(out.stderr.clone()).unwrap(),"stderr_sha256":digest(&out.stderr)}),
    );
}
fn report(out: &Output) -> Value {
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= 32768);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v.as_object().unwrap().len(), 13);
    assert_eq!(v["runtime_unverified"], true);
    assert_eq!(v["physical_reclamation"], false);
    assert_eq!(v["possible_effect"], false);
    assert!(v["intent"].is_null());
    assert!(v["receipt"].is_null());
    v
}
#[test]
fn clean_executable_provenance() {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    assert_eq!(git(&["status", "--porcelain"]), "");
    emit(
        "PROVENANCE",
        json!({"source_head":git(&["rev-parse","HEAD"]),"source_tree":git(&["rev-parse","HEAD^{tree}"]),"executable_sha256":digest(&fs::read(env!("CARGO_BIN_EXE_reader-buddy")).unwrap()),"platform":std::env::consts::OS,"scope":"early refusals only; no actual service invocation"}),
    );
}
#[test]
fn aliases_and_nonlinux_never_consume_held_stdin() {
    let f = Fixture::new();
    for op in ["inspect", "prepare", "apply", "status"] {
        for selector in ["-/", "-/."] {
            let out = collect(spawn(&f, op, selector, &[]));
            record("nonliteral-held", op, selector, None, &[], &out);
            assert_eq!(report(&out)["status"], "unsupported");
        }
        #[cfg(not(target_os = "linux"))]
        {
            let out = collect(spawn(&f, op, "-", &[]));
            record("unsupported-held", op, "-", None, &[], &out);
            assert_eq!(report(&out)["status"], "unsupported");
        }
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}
#[test]
fn parser_and_missing_config_refuse_before_any_service() {
    let f = Fixture::new();
    let vectors: Value = serde_json::from_str(include_str!(
        "fixtures/conversation_cleanup_digest_vectors.json"
    ))
    .unwrap();
    let mut intent = vectors["cases"][0]["original_fields"].clone();
    intent["intent_sha256"] = vectors["cases"][0]["intent_sha256"].clone();
    for op in ["inspect", "prepare", "apply", "status"] {
        let body=serde_json::to_vec(&match op{"inspect"=>json!({"request_version":1,"conversation":"22222222-2222-4222-8222-222222222222"}),"prepare"=>json!({"request_version":1,"operation":"11111111-1111-4111-8111-111111111111","conversation":"22222222-2222-4222-8222-222222222222"}),_=>json!({"intent":intent})}).unwrap();
        let mut c = spawn(&f, op, "-", &[]);
        let write = c.stdin.take().unwrap().write_all(&body);
        if cfg!(target_os = "linux") {
            write.unwrap();
        }
        let out = collect(c);
        record("missing-config", op, "-", Some(&body), &[], &out);
        assert_eq!(
            report(&out)["status"],
            if cfg!(target_os = "linux") {
                "unavailable"
            } else {
                "unsupported"
            }
        );
    }
    for flag in ["--api-key", "--model", "--simulate"] {
        let extra = [flag, "private-synthetic-sentinel"];
        let out = collect(spawn(&f, "prepare", "-", &extra));
        record("recognized-conflict", "prepare", "-", None, &extra, &out);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8(out.stderr)
            .unwrap()
            .contains("private-synthetic-sentinel"));
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}

#[test]
fn malformed_and_oversized_bodies_never_reach_configuration_or_service() {
    let f = Fixture::new();
    let cases = vec![
        ("inspect", b"[]".to_vec()),
        ("prepare", b"{}".to_vec()),
        (
            "inspect",
            b"{\"request_version\":1,\"request_version\":1}".to_vec(),
        ),
        ("status", vec![b' '; 16385]),
        (
            "inspect",
            b"{\"request_version\":1,\"conversation\":\"00000000-0000-0000-0000-000000000000\"}"
                .to_vec(),
        ),
        ("apply", b"{\"intent\":[]}".to_vec()),
    ];
    for (op, body) in cases {
        let mut c = spawn(&f, op, "-", &[]);
        let write = c.stdin.take().unwrap().write_all(&body);
        if cfg!(target_os = "linux") {
            write.unwrap();
        }
        let out = collect(c);
        record("malformed-or-overflow", op, "-", Some(&body), &[], &out);
        assert_eq!(
            report(&out)["status"],
            if cfg!(target_os = "linux") {
                "invalid-input"
            } else {
                "unsupported"
            }
        );
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}
