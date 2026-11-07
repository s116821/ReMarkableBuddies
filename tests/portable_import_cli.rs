//! Actual private CLI/Store transport; synthetic owned files, no model or tablet.
use remarkable_reader_buddy::{config::portable::import, storage::*};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    paths: StorePaths,
    path: PathBuf,
}
impl Fixture {
    fn new(notebook: bool, local: bool) -> Self {
        let root = std::env::temp_dir().join(format!("portable-import-cli-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let store = Store::open(paths.clone()).unwrap();
        if local {
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    sync::mode::SyncModeChoice::LocalOnly,
                )
                .unwrap();
        }
        let bytes = b"retained media";
        let record = Envelope {
            envelope_version: 1,
            namespace: Namespace::SubjectMemory,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: json!({"fixture":"retained"}),
            media_descriptors: vec![Media {
                sha256: digest(bytes),
                bytes: bytes.len() as u64,
                media_type: "text/plain".into(),
            }],
        };
        store
            .commit(
                vec![record],
                BTreeMap::from([(digest(bytes), bytes.to_vec())]),
            )
            .unwrap();
        drop(store);
        if notebook {
            drop(Store::prepare_notebook(paths.clone()).unwrap());
        }
        let path = root.join("config.json");
        fs::write(&path,serde_json::to_vec_pretty(&json!({"paths":paths,"personalization":{"enabled":false,"text":"PRIVATE retained"},"model_credential":"private_key","base_url":"https://PRIVATE.invalid/v1"})).unwrap()).unwrap();
        fs::write(
            root.join(".env"),
            b"OPENAI_API_KEY=PRIVATE_POISON\nREMARKABLE_BUDDIES_CONFIG=must-not-read.json",
        )
        .unwrap();
        Self { root, paths, path }
    }
    fn command(&self, op: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        c.env_clear()
            .current_dir(&self.root)
            .env("REMARKABLE_BUDDIES_CONFIG", &self.path)
            .env("OPENAI_API_KEY", "PRIVATE_UNUSED")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1/unused")
            .env("RUST_LOG", "bad PRIVATE")
            .args(["--portable-configuration-operation", op]);
        c
    }
    fn run(&self, op: &str, request: Option<&Value>) -> Value {
        let mut c = self.command(op);
        let bytes = request.map(|v| serde_json::to_vec(v).unwrap());
        if bytes.is_some() {
            c.args(["--portable-configuration-input", "-"]);
        }
        report(bounded(c, bytes))
    }
    fn prepare(&self, a: &Value) -> Value {
        let i = self.run("inspect", None);
        self.run("prepare",Some(&json!({"portable_import_version":1,"operation":Uuid::new_v4(),"base_sha256":i["data"]["base_sha256"],"config_path_sha256":i["data"]["config_path_sha256"],"artifact":a})))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn bounded(mut c: Command, input: Option<Vec<u8>>) -> Output {
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pipe = child.stdin.take();
    // Inspect keeps stdin open through completion. Mutating selectors receive EOF.
    let mut open = None;
    let writer = if let Some(bytes) = input {
        Some(std::thread::spawn(move || {
            let mut p = pipe.unwrap();
            let _ = p.write_all(&bytes);
        }))
    } else {
        open = pipe;
        None
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(open);
    if let Some(w) = writer {
        w.join().unwrap();
    }
    child.wait_with_output().unwrap()
}
fn report(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stderr.is_empty());
    assert!(o.stdout.len() <= import::MAX_REPORT);
    assert_eq!(o.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["runtime_activation"], "unverified");
    assert!(!v.to_string().contains("PRIVATE"));
    v
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, p: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(p).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    fs::read(p).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
fn artifact() -> Value {
    json!({"portable_configuration_version":1,"settings":{"product_settings":{"writer_enabled":false},"personalization":{"text":"PRIVATE imported é\r\n日本語😀"},"research_policy":null}})
}

#[test]
fn actual_cli_conditional_roundtrip_both_store_families_and_modes_preserve_records_media() {
    for notebook in [false, true] {
        for local in [false, true] {
            let f = Fixture::new(notebook, local);
            let a = artifact();
            let before = inventory(&f.root);
            let old = fs::read(&f.path).unwrap();
            let prepared = f.prepare(&a);
            assert_eq!(prepared["outcome"], "prepared");
            assert_eq!(inventory(&f.root), before);
            let intent = prepared["data"]["intent"].clone();
            let req = json!({"intent":intent,"artifact":a});
            // A live independent owner blocks only Apply, never Status/Prepare.
            let owner = Store::open(f.paths.clone()).unwrap();
            assert_eq!(f.run("apply", Some(&req))["status"], "busy");
            assert_eq!(
                f.run("status", Some(&json!({"intent":intent})))["outcome"],
                "unresolved"
            );
            drop(owner);
            assert_eq!(fs::read(&f.path).unwrap(), old);
            let applied = f.run("apply", Some(&req));
            assert_eq!(applied["outcome"], "verified");
            assert_eq!(applied["disk_readback_verified"], true);
            let actual: Value = serde_json::from_slice(&fs::read(&f.path).unwrap()).unwrap();
            assert_eq!(
                actual["personalization"]["text"],
                a["settings"]["personalization"]["text"]
            );
            assert_eq!(actual["personalization"]["enabled"], false);
            assert_eq!(actual["model_credential"], "private_key");
            assert_eq!(actual["research_policy"], Value::Null);
            assert_eq!(
                f.run("status", Some(&json!({"intent":intent})))["outcome"],
                "desired-file-observed"
            );
            assert_eq!(f.run("apply", Some(&req))["status"], "conflict");
            let mut after = inventory(&f.root);
            after.insert(PathBuf::from("config.json"), old);
            assert_eq!(after, before);
        }
    }
}
#[test]
fn actual_cli_noop_status_and_inspect_need_no_roots_or_stdin_eof() {
    let f = Fixture::new(false, false);
    let a = artifact();
    let intent = f.prepare(&a)["data"]["intent"].clone();
    for p in [&f.paths.data, &f.paths.cache, &f.paths.credentials] {
        fs::remove_dir_all(p).unwrap();
    }
    let before = inventory(&f.root);
    assert_eq!(f.run("inspect", None)["status"], "ok");
    assert_eq!(
        f.prepare(&json!({"portable_configuration_version":1,"settings":{}}))["outcome"],
        "no-change"
    );
    assert_eq!(
        f.run("status", Some(&json!({"intent":intent})))["outcome"],
        "unresolved"
    );
    assert_eq!(inventory(&f.root), before);
    let mut command = f.command("inspect");
    command.env_remove("REMARKABLE_BUDDIES_CONFIG");
    assert_eq!(report(bounded(command, None))["status"], "unavailable");
}
#[test]
fn actual_cli_selector_input_and_version_refusals_are_bounded_and_private() {
    let f = Fixture::new(false, false);
    let before = inventory(&f.root);
    for args in [
        vec!["--debug-dump"],
        vec!["--writer-note"],
        vec!["--product-settings-operation", "inspect"],
        vec!["--configuration-schema"],
        vec!["--inspect-portable-configuration", "-"],
    ] {
        let mut c = f.command("inspect");
        c.args(args);
        let o = bounded(c, None);
        assert!(!o.status.success());
        assert!(o.stdout.is_empty());
    }
    assert_eq!(f.run("prepare", None)["status"], "unsupported");
    let mut c = f.command("prepare");
    c.args(["--portable-configuration-input", "must-not-read.json"]);
    assert_eq!(report(bounded(c, None))["status"], "unsupported");
    let mut c = f.command("inspect");
    c.args(["--portable-configuration-input", "-"]);
    assert_eq!(report(bounded(c, None))["status"], "invalid-input");
    let mut c = f.command("apply");
    c.args(["--portable-configuration-input", "-"]);
    assert_eq!(
        report(bounded(c, Some(vec![b' '; import::MAX_REQUEST + 1])))["status"],
        "invalid-input"
    );
    let mut c = f.command("apply");
    c.args(["--portable-configuration-input", "-"]);
    assert_eq!(
        report(bounded(
            c,
            Some(br#"{"intent":{},"intent":{},"artifact":{}}"#.to_vec())
        ))["status"],
        "invalid-input"
    );
    let d = f.run("inspect", None);
    assert_eq!(f.run("prepare",Some(&json!({"portable_import_version":2,"operation":Uuid::new_v4(),"base_sha256":d["data"]["base_sha256"],"config_path_sha256":d["data"]["config_path_sha256"],"artifact":artifact()})))["status"],"unsupported");
    assert_eq!(inventory(&f.root), before);
}
#[cfg(unix)]
#[test]
fn actual_cli_link_and_fifo_refuse_without_opening_streams() {
    let f = Fixture::new(false, false);
    let link = f.root.join("alias.json");
    std::os::unix::fs::symlink(&f.path, &link).unwrap();
    let mut c = f.command("inspect");
    c.env("REMARKABLE_BUDDIES_CONFIG", link);
    assert_eq!(report(bounded(c, None))["status"], "unavailable");
    let fifo = f.root.join("pipe");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut c = f.command("inspect");
    c.env("REMARKABLE_BUDDIES_CONFIG", fifo);
    assert_eq!(report(bounded(c, None))["status"], "unavailable");
}
