//! Actual existing selectors on a ready retained notebook; no provider/tablet.
use buddy::storage::*;
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;

fn run(f: &Fixture, family: &str, operation: &str, input: Option<&Value>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command
        .env_clear()
        .current_dir(&f.root)
        .env("REMARKABLE_BUDDIES_CONFIG", f.root.join("config.json"))
        .env("OPENAI_API_KEY", "PRIVATE_UNUSED")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1/unused")
        .args([format!("--{family}-settings-operation"), operation.into()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.args([format!("--{family}-settings-input"), "-".into()]);
    }
    let mut child = command.spawn().unwrap();
    let mut pipe = Some(child.stdin.take().unwrap());
    let writer = input.map(|input| {
        let bytes = serde_json::to_vec(input).unwrap();
        let mut pipe = pipe.take().unwrap();
        std::thread::spawn(move || {
            pipe.write_all(&bytes).unwrap();
        })
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(writer) = writer {
        writer.join().unwrap();
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    assert!(result.stdout.len() <= 16_384 && result.stdout.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("PRIVATE"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains(PRIVATE_ID));
    serde_json::from_slice(&result.stdout).unwrap()
}

// Compare the whole fixture, including parent-directory publication staging, except
// the selected configuration file. Lock contents are platform-specific bookkeeping.
fn root_inventory(f: &Fixture) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if path.is_dir() {
                out.insert(relative, b"directory".to_vec());
                walk(root, &path, out);
            } else if relative != Path::new("config.json") {
                let bytes = if path.file_name().is_some_and(|n| n == "store.lock") {
                    fs::metadata(&path).unwrap().len().to_le_bytes().to_vec()
                } else {
                    fs::read(path).unwrap()
                };
                out.insert(relative, bytes);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&f.root, &f.root, &mut out);
    out
}

const PRIVATE_ID: &str = "123E4567-E89B-12D3-A456-426614174000";
fn changes(family: &str, clear: bool) -> Value {
    if clear {
        return match family {
            "basic" => {
                json!({"debug_dump":{"action":"clear"},"log_level":{"action":"clear"},"trigger_corner":{"action":"clear"}})
            }
            "model" => json!({"model":{"action":"clear"},"base_url":{"action":"clear"}}),
            _ => json!({"enabled":{"action":"clear"},"profile":{"action":"clear"}}),
        };
    }
    match family {
        "basic" => {
            json!({"debug_dump":{"action":"set","value":true},"log_level":{"action":"set","value":"debug"},"trigger_corner":{"action":"set","value":"UL"}})
        }
        "model" => {
            json!({"model":{"action":"set","value":" PRIVATE_MODEL\r\n 模型 "},"base_url":{"action":"set","value":"https://PRIVATE:PRIVATE@invalid/?q=PRIVATE#PRIVATE\r\n "}})
        }
        _ => {
            json!({"enabled":{"action":"set","value":true},"profile":{"action":"set","value":PRIVATE_ID}})
        }
    }
}
fn request(f: &Fixture, family: &str, changes: &Value) -> Value {
    let current = run(f, family, "inspect", None);
    let mut request = json!({"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],
        "config_path_sha256":current["data"]["config_path_sha256"],"changes":changes});
    request[match family {
        "basic" => "basic_version",
        "model" => "model_settings_version",
        _ => "memory_settings_version",
    }] = json!(1);
    request
}
fn exercise(family: &str) {
    for local in [false, true] {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        let media = b"retained notebook media";
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
                sha256: digest(media),
                bytes: media.len() as u64,
                media_type: "text/plain".into(),
            }],
        };
        store
            .commit(
                vec![record],
                BTreeMap::from([(digest(media), media.to_vec())]),
            )
            .unwrap();
        if local {
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    sync::mode::SyncModeChoice::LocalOnly,
                )
                .unwrap();
        }
        drop(store);
        drop(Store::prepare_notebook(f.paths()).unwrap());
        let path = f.root.join("config.json");
        let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        raw["personalization"] = json!({"enabled":false,"text":"PRIVATE exact\r\n α "});
        raw["model_credential"] = json!("PRIVATE_REFERENCE");
        raw["product_settings"] = Value::Null;
        raw["memory"] = json!({});
        fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        let inventory = root_inventory(&f);
        for clear in [false, true] {
            let old = fs::read(&path).unwrap();
            let before: Value = serde_json::from_slice(&old).unwrap();
            let patch = changes(family, clear);
            let prepared = run(&f, family, "prepare", Some(&request(&f, family, &patch)));
            assert_eq!(prepared["outcome"], "prepared", "{family}: {prepared}");
            let mut input = json!({"intent":prepared["data"]["intent"]});
            if family != "basic" {
                input["changes"] = patch.clone();
            }
            assert_eq!(
                run(
                    &f,
                    family,
                    "status",
                    Some(&json!({"intent":input["intent"]}))
                )["outcome"],
                "unresolved"
            );
            let owner = Store::open(f.paths()).unwrap();
            assert_eq!(run(&f, family, "apply", Some(&input))["status"], "busy");
            drop(owner);
            assert_eq!(fs::read(&path).unwrap(), old);
            // A forged target cannot mutate even after successful notebook admission.
            let mut forged = input.clone();
            forged["intent"]["target_sha256"] = json!("0".repeat(64));
            assert_eq!(
                run(&f, family, "apply", Some(&forged))["status"],
                "invalid-input"
            );
            assert_eq!(fs::read(&path).unwrap(), old);
            let applied = run(&f, family, "apply", Some(&input));
            assert_eq!(applied["outcome"], "verified");
            assert_eq!(applied["disk_readback_verified"], true);
            assert_eq!(
                applied["runtime_activation"],
                if family == "memory" {
                    "unverified"
                } else {
                    "restart-required"
                }
            );
            let saved = fs::read(&path).unwrap();
            assert_eq!(applied["data"]["target_sha256"], digest(&saved));
            let after: Value = serde_json::from_slice(&saved).unwrap();
            let keys: &[&str] = match family {
                "basic" => &["debug_dump", "log_level", "trigger_corner"],
                "model" => &["model", "base_url"],
                _ => &["memory"],
            };
            let mut other_before = before.as_object().unwrap().clone();
            let mut other_after = after.as_object().unwrap().clone();
            for key in keys {
                other_before.remove(*key);
                other_after.remove(*key);
            }
            assert_eq!(other_before, other_after);
            let target = if family == "memory" {
                &after["memory"]
            } else {
                &after
            };
            for (key, change) in patch.as_object().unwrap() {
                if clear {
                    assert!(target.get(key).is_none());
                } else {
                    assert_eq!(target[key], change["value"]);
                }
            }
            let status = run(
                &f,
                family,
                "status",
                Some(&json!({"intent":input["intent"]})),
            );
            assert_eq!(status["outcome"], "desired-file-observed");
            assert_eq!(status["runtime_activation"], "unverified");
            assert_eq!(run(&f, family, "apply", Some(&input))["status"], "conflict");
            assert_eq!(
                run(&f, family, "prepare", Some(&request(&f, family, &patch)))["outcome"],
                "no-change"
            );
            assert_eq!(fs::read(&path).unwrap(), saved);
            assert_eq!(root_inventory(&f), inventory);
        }
        // Each newly admitted caller must still refuse corrupt persisted notebook state.
        let patch = changes(family, false);
        let prepared = run(&f, family, "prepare", Some(&request(&f, family, &patch)));
        let mut input = json!({"intent":prepared["data"]["intent"]});
        if family != "basic" {
            input["changes"] = patch;
        }
        fs::write(f.paths().data.join("sync/mode.json"), b"not valid mode").unwrap();
        let held = root_inventory(&f);
        let old = fs::read(&path).unwrap();
        assert_eq!(
            run(&f, family, "apply", Some(&input))["status"],
            "unsupported"
        );
        assert_eq!(fs::read(&path).unwrap(), old);
        assert_eq!(root_inventory(&f), held);
    }
}
#[test]
fn actual_basic_notebook_declarations_roundtrip() {
    exercise("basic");
}
#[test]
fn actual_model_notebook_private_declarations_roundtrip() {
    exercise("model");
}
#[test]
fn actual_memory_notebook_private_declarations_roundtrip() {
    exercise("memory");
}

#[test]
fn actual_three_declaration_writers_keep_legacy_feedback_admission() {
    for family in ["basic", "model", "memory"] {
        let f = Fixture::new();
        drop(buddy::unassociated_feedback::Feedback::prepare(f.paths()).unwrap());
        let inventory = root_inventory(&f);
        let patch = changes(family, false);
        let prepared = run(&f, family, "prepare", Some(&request(&f, family, &patch)));
        assert_eq!(prepared["outcome"], "prepared");
        let mut input = json!({"intent":prepared["data"]["intent"]});
        if family != "basic" {
            input["changes"] = patch;
        }
        let applied = run(&f, family, "apply", Some(&input));
        assert_eq!(applied["outcome"], "verified");
        assert_eq!(root_inventory(&f), inventory);
    }
}
