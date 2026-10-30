//! Original local synthetic one-shot CLI corpus, never tablet/provider execution.
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
        let p = std::env::temp_dir().join(format!("buddy-cache-cli-{}", Uuid::new_v4()));
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
                .starts_with("buddy-cache-cli-"));
            fs::remove_dir_all(p).unwrap();
        }
    }
}
fn emit(kind: &str, data: Value) {
    let mut b = format!("SOURCE_CACHE_CORPUS_{kind} ").into_bytes();
    b.extend(serde_json::to_vec(&data).unwrap());
    b.push(b'\n');
    std::io::stdout().lock().write_all(&b).unwrap();
}
fn spawn(f: &Fixture, op: &str, selector: &str, config: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .env_clear()
        .env("REMARKABLE_BUDDIES_CONFIG", config)
        .current_dir(&f.0)
        .args(["--source-cache-operation", op])
        .arg(format!("--source-cache-input={selector}"))
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
            panic!("owned child exceeded bound: {}", output.status);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    c.wait_with_output().unwrap()
}
fn record(case: &str, op: &str, body: Option<&str>, out: &Output) -> Value {
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= 16384);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["runtime_unverified"], true);
    emit(
        "RECORD",
        json!({"case":case,"operation":op,"stdin_body_utf8":body,"exit_code":out.status.code(),"stdout_utf8":String::from_utf8(out.stdout.clone()).unwrap(),"stdout_sha256":digest(&out.stdout),"stderr_utf8":String::from_utf8(out.stderr.clone()).unwrap(),"stderr_sha256":digest(&out.stderr)}),
    );
    value
}
#[test]
fn clean_executable_provenance() {
    let git = |arg: &[&str]| {
        let o = Command::new("git")
            .args(arg)
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
fn held_stdin_nonliteral_and_platform_controls() {
    let f = Fixture::new();
    let config = f.0.join("missing.json");
    for op in ["inspect", "prepare", "apply", "status"] {
        for selector in ["./-", "-/", "-/.", "not-stdin.json"] {
            let out = collect(spawn(&f, op, selector, &config));
            assert_eq!(record(selector, op, None, &out)["status"], "unsupported");
        }
        let c = spawn(&f, op, "-", &config);
        #[cfg(target_os = "linux")]
        {
            let mut c = c;
            std::thread::sleep(Duration::from_millis(100));
            assert!(c.try_wait().unwrap().is_none());
            c.stdin.take().unwrap().write_all(b"{}").unwrap();
            let out = collect(c);
            assert_eq!(
                record("literal-eof", op, Some("{}"), &out)["status"],
                "invalid-input"
            );
        }
        #[cfg(not(target_os = "linux"))]
        {
            let out = collect(c);
            assert_eq!(
                record("platform-no-read", op, None, &out)["status"],
                "unsupported"
            );
        }
    }
    assert!(fs::read_dir(&f.0).unwrap().next().is_none());
}
#[cfg(target_os = "linux")]
#[test]
fn strict_refusal_corpus_before_config_access() {
    let f = Fixture::new();
    let config = f.0.join("missing.json");
    for (case, body) in [
        ("array", "[]"),
        ("duplicate", "{\"request_version\":1,\"request_version\":1}"),
        ("nested-array", "{\"request_version\":1,\"intent\":[]}"),
        ("extra-json", "{} {}"),
        ("missing", "{}"),
    ] {
        for op in ["inspect", "prepare", "apply", "status"] {
            let mut c = spawn(&f, op, "-", &config);
            c.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
            let out = collect(c);
            assert_eq!(
                record(case, op, Some(body), &out)["status"],
                "invalid-input"
            );
        }
    }
    let body = "x".repeat(16385);
    let mut c = spawn(&f, "prepare", "-", &config);
    c.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
    let out = collect(c);
    assert_eq!(
        record("oversized", "prepare", Some(&body), &out)["status"],
        "invalid-input"
    );
    assert!(fs::read_dir(&f.0).unwrap().next().is_none());
}
#[cfg(target_os = "linux")]
#[test]
fn synthetic_prepare_apply_and_absence_corpus() {
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
    // An ordinary Store recovery would inspect this unsupported durable commit;
    // the new header-only producer must never enumerate or parse it.
    let generation: Uuid =
        serde_json::from_slice(&fs::read(paths.data.join("CURRENT")).unwrap()).unwrap();
    let poison = paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("commits")
        .join(format!("{}.json", Uuid::new_v4()));
    fs::write(&poison, b"private invalid durable commit sentinel").unwrap();
    let call = |op: &str, value: Value| {
        let body = serde_json::to_string(&value).unwrap();
        let mut c = spawn(&f, op, "-", &config);
        c.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
        record("synthetic-owned", op, Some(&body), &collect(c))
    };
    assert_eq!(
        call("inspect", json!({"request_version":1,"document":doc}))["status"],
        "ready"
    );
    let prepared = call(
        "prepare",
        json!({"request_version":1,"document":doc,"operation":Uuid::new_v4()}),
    );
    assert_eq!(prepared["status"], "prepared");
    let confirmed = json!({"request_version":1,"intent":prepared["intent"]});
    assert_eq!(call("status", confirmed.clone())["status"], "unchanged");
    assert_eq!(call("apply", confirmed.clone())["status"], "applied");
    assert_eq!(call("status", confirmed.clone())["status"], "absent");
    assert_eq!(call("apply", confirmed)["status"], "absent");
    assert!(!paths
        .cache
        .join("normalized-sources-v1")
        .join(format!("{doc}.json"))
        .exists());
    assert_eq!(
        fs::read(poison).unwrap(),
        b"private invalid durable commit sentinel"
    );
}
