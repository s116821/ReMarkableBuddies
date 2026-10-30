//! Original metadata-only CLI controls; synthetic local roots and parser tokens.
use remarkable_reader_buddy::storage::{digest, Uuid};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("buddy-inventory-cli-{}", Uuid::new_v4()));
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
                .to_str()
                .unwrap()
                .starts_with("buddy-inventory-cli-"));
            fs::remove_dir_all(p).unwrap();
        }
    }
}
fn emit(kind: &str, data: Value) {
    let mut b = format!("CACHE_INVENTORY_CORPUS_{kind} ").into_bytes();
    b.extend(serde_json::to_vec(&data).unwrap());
    b.push(b'\n');
    std::io::stdout().lock().write_all(&b).unwrap();
}
fn spawn(f: &Fixture, config: &Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .env_clear()
        .env("REMARKABLE_BUDDIES_CONFIG", config)
        .current_dir(&f.0)
        .arg("--inspect-source-cache")
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
            let o = c.wait_with_output().unwrap();
            panic!("owned inventory child exceeded bound: {}", o.status);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    c.wait_with_output().unwrap()
}
fn record(case: &str, args: &[&str], o: &Output) {
    emit(
        "RECORD",
        json!({"case":case,"argv_extra":args,"stdin_held_without_eof":true,"exit_code":o.status.code(),"stdout_utf8":String::from_utf8(o.stdout.clone()).unwrap(),"stdout_sha256":digest(&o.stdout),"stderr_utf8":String::from_utf8(o.stderr.clone()).unwrap(),"stderr_sha256":digest(&o.stderr)}),
    );
}
fn report(case: &str, o: &Output) -> Value {
    record(case, &[], o);
    assert!(o.status.success());
    assert!(o.stderr.is_empty());
    assert!(o.stdout.len() <= 8192);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_object().unwrap().len(), 13);
    assert_eq!(v["runtime_unverified"], true);
    for key in [
        "cleanup_authority",
        "entry_payloads_read",
        "entry_payloads_validated",
        "source_freshness_verified",
    ] {
        assert_eq!(v[key], false);
    }
    v
}
#[test]
fn clean_executable_provenance() {
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(o.status.success());
        String::from_utf8(o.stdout).unwrap().trim().to_string()
    };
    assert_eq!(git(&["status", "--porcelain"]), "");
    emit(
        "PROVENANCE",
        json!({"source_head":git(&["rev-parse","HEAD"]),"source_tree":git(&["rev-parse","HEAD^{tree}"]),"executable_sha256":digest(&fs::read(env!("CARGO_BIN_EXE_reader-buddy")).unwrap()),"platform":std::env::consts::OS}),
    );
}
#[test]
fn parameterless_does_not_consume_held_stdin_or_start_runtime() {
    let f = Fixture::new();
    let v = report(
        "missing-config",
        &collect(spawn(&f, &f.0.join("never-read.json"), &[])),
    );
    assert_eq!(
        v["status"],
        if cfg!(target_os = "linux") {
            "unavailable"
        } else {
            "unsupported"
        }
    );
    assert!(v["entries"].is_null());
    assert!(fs::read_dir(&f.0).unwrap().next().is_none());
}
#[test]
fn recognized_conflicts_hide_values_but_ordinary_syntax_can_echo() {
    let f = Fixture::new();
    let config = f.0.join("never-read.json");
    for flag in ["--api-key", "--model", "--simulate"] {
        let extra = [flag, "private-fixture-sentinel"];
        let o = collect(spawn(&f, &config, &extra));
        record("recognized-conflict", &extra, &o);
        assert_eq!(o.status.code(), Some(2));
        assert!(o.stdout.is_empty());
        let text = String::from_utf8(o.stderr).unwrap();
        assert!(!text.contains("private-fixture-sentinel"));
    }
    for extra in [
        vec!["unexpected-fixture-token"],
        vec!["--inspect-source-cache=unexpected-fixture-value"],
        vec!["--unexpected-fixture-option"],
    ] {
        let o = collect(spawn(&f, &config, &extra));
        record("ordinary-syntax-diagnostic", &extra, &o);
        assert_eq!(o.status.code(), Some(2));
        assert!(o.stdout.is_empty());
        assert!(String::from_utf8(o.stderr)
            .unwrap()
            .contains("unexpected-fixture"));
    }
    assert!(fs::read_dir(&f.0).unwrap().next().is_none());
}
#[cfg(target_os = "linux")]
#[test]
fn actual_owned_metadata_inventory_then_cleanup_revalidates_corrupt_slot() {
    use remarkable_reader_buddy::{
        config::Config,
        retrieval::{disk::DiskCache, NormalizedDocument, SourceFormat, SourceUnit},
        storage::{Store, StorePaths},
    };
    let f = Fixture::new();
    let paths = StorePaths {
        data: f.0.join("data"),
        cache: f.0.join("cache"),
        credentials: f.0.join("credentials"),
    };
    let doc = Uuid::new_v4();
    let store = Store::open(paths.clone()).unwrap();
    let cache = DiskCache::open(&store).unwrap();
    cache
        .install(NormalizedDocument {
            document: doc,
            format: SourceFormat::Pdf,
            source_sha256: digest(b"synthetic"),
            extractor_revision: "fixture".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "private source sentinel".into(),
            }],
        })
        .unwrap();
    drop(cache);
    drop(store);
    let config = f.0.join("config.json");
    fs::write(
        &config,
        serde_json::to_vec(&Config {
            paths: paths.clone(),
            ..Config::default()
        })
        .unwrap(),
    )
    .unwrap();
    let ns = paths.cache.join("normalized-sources-v1");
    let slot = ns.join(format!("{doc}.json"));
    for (case, bytes) in [
        ("corrupt-payload", b"not json".as_slice()),
        ("zero-length", b"".as_slice()),
    ] {
        fs::write(&slot, bytes).unwrap();
        let v = report(case, &collect(spawn(&f, &config, &[])));
        assert_eq!(v["status"], "ok");
        assert_eq!(
            v["entries"],
            json!([{"document":doc,"apparent_bytes":bytes.len()}])
        );
        assert_eq!(v["total_apparent_bytes"], bytes.len() as u64);
        assert_eq!(fs::read(&slot).unwrap(), bytes);
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .env_clear()
        .env("REMARKABLE_BUDDIES_CONFIG", &config)
        .current_dir(&f.0)
        .args([
            "--source-cache-operation",
            "prepare",
            "--source-cache-input",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let body =
        serde_json::to_vec(&json!({"request_version":1,"operation":Uuid::new_v4(),"document":doc}))
            .unwrap();
    child.stdin.take().unwrap().write_all(&body).unwrap();
    let o = collect(child);
    emit(
        "RECORD",
        json!({"case":"separate-cleanup-revalidates","argv_extra":["--source-cache-operation","prepare","--source-cache-input","-"],"stdin_held_without_eof":false,"stdin_body_utf8":String::from_utf8(body).unwrap(),"exit_code":o.status.code(),"stdout_utf8":String::from_utf8(o.stdout.clone()).unwrap(),"stdout_sha256":digest(&o.stdout),"stderr_utf8":String::from_utf8(o.stderr.clone()).unwrap(),"stderr_sha256":digest(&o.stderr)}),
    );
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "unsupported");
    assert!(v["intent"].is_null());
    assert!(slot.exists());
    fs::remove_file(&slot).unwrap();
    let v = report("empty-owned-namespace", &collect(spawn(&f, &config, &[])));
    assert_eq!(v["status"], "ok");
    assert_eq!(v["entries"], json!([]));
    assert_eq!(v["total_apparent_bytes"], 0);
    fs::write(ns.join("unknown"), b"sentinel").unwrap();
    let v = report("unknown-owned-entry", &collect(spawn(&f, &config, &[])));
    assert_eq!(v["status"], "unsupported");
    assert!(v["entries"].is_null());
    assert_eq!(fs::read(ns.join("unknown")).unwrap(), b"sentinel");
}
