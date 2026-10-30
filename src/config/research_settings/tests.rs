use super::*;
use crate::storage::StorePaths;
use serde_json::json;
use std::{fs, io};

struct Fixture {
    root: PathBuf,
    file: PathBuf,
    paths: StorePaths,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-research-settings-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        drop(Store::open(paths.clone()).unwrap());
        let file = root.join("config.json");
        fs::write(
            &file,
            serde_json::to_vec_pretty(&json!({"paths":paths,"model":null,
            "base_url":"https://private.invalid/?token=PRIVATE_KEEP","memory":{},
            "personalization":{"enabled":false,"text":" PRIVATE_KEEP\r\nα "}}))
            .unwrap(),
        )
        .unwrap();
        Self { root, file, paths }
    }
    fn raw(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.file).unwrap()).unwrap()
    }
    fn write(&self, raw: &Value) {
        fs::write(&self.file, serde_json::to_vec_pretty(raw).unwrap()).unwrap();
    }
    fn run(&self, operation: Operation, input: &Value) -> Report {
        run_stdin(
            operation,
            Some(self.file.clone()),
            &mut serde_json::to_vec(input).unwrap().as_slice(),
        )
    }
    fn prepare(&self, changes: &Value) -> Value {
        let current = finish(inspect(Some(self.file.clone())));
        finish(self.run(Operation::Prepare, &json!({"research_settings_version":1,"operation":Uuid::new_v4(),
            "base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":changes})))
    }
    fn intent(&self, changes: &Value) -> Value {
        let report = self.prepare(changes);
        assert_eq!(report["outcome"], "prepared");
        report["data"]["intent"].clone()
    }
    fn apply(&self, changes: &Value) -> Value {
        let intent = self.intent(changes);
        finish(self.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":changes}),
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let target = self.root.canonicalize().unwrap();
            assert_eq!(
                target.parent(),
                Some(std::env::temp_dir().canonicalize().unwrap().as_path())
            );
            assert!(target
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("buddy-research-settings-"));
            fs::remove_dir_all(target).unwrap();
        }
    }
}
fn finish(report: Report) -> Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert!(bytes.len() <= MAX_IO && bytes.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_KEEP"));
    serde_json::from_slice(&bytes).unwrap()
}
fn patch(value: &str) -> Value {
    json!({"research_policy":{"action":"set","value":value}})
}

#[test]
fn defaults_null_snapshot_and_fixed_broad_projection_stay_distinct() {
    assert!(serde_json::to_value(Config::default())
        .unwrap()
        .get("research_policy")
        .is_none());
    let f = Fixture::new();
    let original = f.raw();
    let broad = inspection::run(inspection::Operation::Inspect, Some(f.file.clone()));
    let mut out = Vec::new();
    broad.write_to(&mut out).unwrap();
    let baseline: Value = serde_json::from_slice(&out).unwrap();
    for (declared, origin, expected) in [
        (None, "default", ResearchPolicy::Required),
        (Some(Value::Null), "declared-null", ResearchPolicy::Required),
        (
            Some(json!("required")),
            "declared",
            ResearchPolicy::Required,
        ),
        (
            Some(json!("disabled")),
            "declared",
            ResearchPolicy::Disabled,
        ),
    ] {
        let mut raw = original.clone();
        if let Some(value) = declared {
            raw["research_policy"] = value;
        }
        f.write(&raw);
        let report = finish(inspect(Some(f.file.clone())));
        assert_eq!(
            report["data"]["current"]["research_policy"],
            json!({"origin":origin,"value":expected})
        );
        let config: Config = serde_json::from_value(raw).unwrap();
        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(
            encoded.get("research_policy").cloned(),
            f.raw().get("research_policy").cloned()
        );
        let snapshot = config.research_policy_snapshot().unwrap();
        assert_eq!(snapshot.policy(), expected);
        let mut changed = config.clone();
        changed.research_policy = Some(Some(ResearchPolicy::Disabled));
        assert_eq!(
            changed.research_policy_snapshot().unwrap().policy(),
            ResearchPolicy::Disabled
        );
        assert_eq!(snapshot.policy(), expected);
        let mut broad = Vec::new();
        inspection::run(inspection::Operation::Inspect, Some(f.file.clone()))
            .write_to(&mut broad)
            .unwrap();
        let current: Value = serde_json::from_slice(&broad).unwrap();
        assert_eq!(
            current["snapshot"]["fields"],
            baseline["snapshot"]["fields"]
        );
        assert_eq!(
            current["snapshot"]["unsupported"],
            baseline["snapshot"]["unsupported"]
        );
    }
}

#[test]
fn real_set_clear_and_noop_preserve_unrelated_values_and_bytes() {
    let f = Fixture::new();
    let original = f.raw();
    let report = f.apply(&patch("disabled"));
    assert_eq!(report["outcome"], "verified");
    assert_eq!(report["disk_readback_verified"], true);
    assert_eq!(report["runtime_activation"], "unverified");
    let mut raw = f.raw();
    assert_eq!(
        raw.as_object_mut().unwrap().remove("research_policy"),
        Some(json!("disabled"))
    );
    assert_eq!(raw, original);
    let bytes = fs::read(&f.file).unwrap();
    assert_eq!(f.prepare(&patch("disabled"))["outcome"], "no-change");
    assert_eq!(fs::read(&f.file).unwrap(), bytes);
    let clear = json!({"research_policy":{"action":"clear"}});
    assert_eq!(f.apply(&clear)["outcome"], "verified");
    assert_eq!(f.raw(), original);
    let bytes = fs::read(&f.file).unwrap();
    assert_eq!(f.prepare(&clear)["outcome"], "no-change");
    assert_eq!(fs::read(&f.file).unwrap(), bytes);
    let mut null = original;
    null["research_policy"] = Value::Null;
    f.write(&null);
    assert_eq!(f.apply(&patch("required"))["outcome"], "verified");
    assert_eq!(f.raw()["research_policy"], "required");
    assert_eq!(f.apply(&clear)["outcome"], "verified");
    assert!(f.raw().get("research_policy").is_none());
}

#[test]
fn strict_patch_binding_original_conditions_and_busy_never_repair() {
    let f = Fixture::new();
    let changes = patch("disabled");
    let intent = f.intent(&changes);
    let before = fs::read(&f.file).unwrap();
    for field in [
        "base_sha256",
        "target_sha256",
        "config_path_sha256",
        "changes_sha256",
    ] {
        let mut wrong = intent.clone();
        wrong[field] = digest(field.as_bytes()).into();
        assert_ne!(
            finish(f.run(Operation::Apply, &json!({"intent":wrong,"changes":changes})))["status"],
            "ok"
        );
        assert_eq!(fs::read(&f.file).unwrap(), before);
    }
    assert_eq!(
        finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":patch("required")})
        ))["status"],
        "invalid-input"
    );
    let owner = Store::open(f.paths.clone()).unwrap();
    assert_eq!(
        finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":changes})
        ))["status"],
        "busy"
    );
    drop(owner);
    let mut raw = f.raw();
    raw["debug_dump"] = true.into();
    f.write(&raw);
    assert_eq!(
        finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":changes})
        ))["status"],
        "conflict"
    );
    assert_eq!(f.raw(), raw);
    for invalid in [
        json!({}),
        json!({"research_policy":{"action":"clear","extra":1}}),
        json!({"research_policy":{"action":"set","value":null}}),
        json!({"research_policy":{"action":"set","value":"Disabled"}}),
        json!({"model":{"action":"clear"}}),
    ] {
        assert_eq!(f.prepare(&invalid)["status"], "invalid-input");
    }
    let raw = br#"{"research_settings_version":1,"research_settings_version":1}"#;
    assert_eq!(
        finish(run_stdin(
            Operation::Prepare,
            Some(f.file.clone()),
            &mut raw.as_slice()
        ))["status"],
        "invalid-input"
    );
    let oversized = vec![b' '; MAX_IO + 1];
    assert_eq!(
        finish(run_stdin(
            Operation::Prepare,
            Some(f.file.clone()),
            &mut oversized.as_slice()
        ))["status"],
        "invalid-input"
    );
}

#[test]
fn read_paths_use_only_pure_values_and_inspection_never_reads_input() {
    let f = Fixture::new();
    let mut raw = f.raw();
    raw["paths"] = json!({"data":"relative-unadmitted-data","cache":"relative-unadmitted-cache","credentials":"relative-private"});
    f.write(&raw);
    let config: Config = serde_json::from_value(raw).unwrap();
    assert!(config.research_policy_snapshot().is_ok());
    assert!(config.validate().is_err());
    let intent = f.intent(&patch("disabled"));
    assert_eq!(
        finish(f.run(Operation::Status, &json!({"intent":intent})))["outcome"],
        "unresolved"
    );
    assert_eq!(
        finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":patch("disabled")})
        ))["status"],
        "unsupported"
    );
    struct NeverRead;
    impl Read for NeverRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("inspection consumed input")
        }
    }
    assert_eq!(
        finish(run_stdin(
            Operation::Inspect,
            Some(f.file.clone()),
            &mut NeverRead
        ))["status"],
        "invalid-input"
    );
}

#[test]
fn possible_publication_and_prefix_failure_retain_custody_without_retry() {
    let f = Fixture::new();
    let changes = patch("disabled");
    let intent = f.intent(&changes);
    let doc = Document::load(Some(f.file.clone())).unwrap();
    let report = apply_with(
        doc,
        serde_json::from_value(intent.clone()).unwrap(),
        serde_json::from_value(changes).unwrap(),
        |store, path, base, target| {
            assert!(matches!(
                store.replace_config_document(path, base, target),
                ConfigPublication::Verified(_)
            ));
            ConfigPublication::Unknown
        },
    );
    assert_eq!(report.outcome, Outcome::Unknown);
    assert!(Store::open(f.paths.clone()).is_err());
    let observed = finish(f.run(Operation::Status, &json!({"intent":intent})));
    assert_eq!(observed["outcome"], "desired-file-observed");
    assert_eq!(report.outcome, Outcome::Unknown);
    struct Prefix(Vec<u8>);
    impl Write for Prefix {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                self.0.push(bytes[0]);
                Ok(1)
            } else {
                Err(io::Error::other("fixture output failure"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Prefix(Vec::new());
    assert!(report.write_to(&mut writer).is_err());
    assert_eq!(writer.0, b"{");
    drop(Store::open(f.paths.clone()).unwrap());
    assert_eq!(f.raw()["research_policy"], "disabled");
}

#[test]
fn patch_fingerprint_uses_original_decoded_canonical_value_and_domain() {
    let literal = br#"{"research_policy":{"action":"set","value":"disabled"}}"#;
    let escaped = br#"{"research_policy":{"value":"\u0064isabled","action":"set"}}"#;
    let expected = digest(
        &[
            b"urn:remarkable-buddies:research-settings-patch:v1:".as_slice(),
            literal,
        ]
        .concat(),
    );
    assert_eq!(patch_hash(&parse::<Changes>(literal).unwrap()), expected);
    assert_eq!(patch_hash(&parse::<Changes>(escaped).unwrap()), expected);
}

#[test]
fn existing_editor_operations_preserve_the_new_declaration() {
    use super::super::{basic, memory_settings, model_settings, personalization};
    fn render(write: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> Value {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        serde_json::from_slice(&out).unwrap()
    }
    for declaration in [
        None,
        Some(Value::Null),
        Some(json!("required")),
        Some(json!("disabled")),
    ] {
        let f = Fixture::new();
        let mut raw = f.raw();
        if let Some(value) = &declaration {
            raw["research_policy"] = value.clone();
        }
        f.write(&raw);
        for (which, version, changes) in [
            (
                0,
                "basic_version",
                json!({"debug_dump":{"action":"set","value":true}}),
            ),
            (
                1,
                "model_settings_version",
                json!({"model":{"action":"set","value":"authored-model"}}),
            ),
            (
                2,
                "memory_settings_version",
                json!({"profile":{"action":"set","value":Uuid::new_v4()}}),
            ),
        ] {
            let current = match which {
                0 => render(|out| basic::inspect(Some(f.file.clone())).write_to(out)),
                1 => render(|out| model_settings::inspect(Some(f.file.clone())).write_to(out)),
                _ => render(|out| memory_settings::inspect(Some(f.file.clone())).write_to(out)),
            };
            let mut request = json!({"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":changes});
            request[version] = 1.into();
            let bytes = serde_json::to_vec(&request).unwrap();
            let prepared = match which {
                0 => render(|out| {
                    basic::run_stdin(
                        basic::Operation::Prepare,
                        Some(f.file.clone()),
                        &mut bytes.as_slice(),
                    )
                    .write_to(out)
                }),
                1 => render(|out| {
                    model_settings::run_stdin(
                        model_settings::Operation::Prepare,
                        Some(f.file.clone()),
                        &mut bytes.as_slice(),
                    )
                    .write_to(out)
                }),
                _ => render(|out| {
                    memory_settings::run_stdin(
                        memory_settings::Operation::Prepare,
                        Some(f.file.clone()),
                        &mut bytes.as_slice(),
                    )
                    .write_to(out)
                }),
            };
            assert_eq!(prepared["outcome"], "prepared");
            let input = serde_json::to_vec(&if which == 0 {
                json!({"intent":prepared["data"]["intent"]})
            } else {
                json!({"intent":prepared["data"]["intent"],"changes":changes})
            })
            .unwrap();
            let applied = match which {
                0 => render(|out| {
                    basic::run_stdin(
                        basic::Operation::Apply,
                        Some(f.file.clone()),
                        &mut input.as_slice(),
                    )
                    .write_to(out)
                }),
                1 => render(|out| {
                    model_settings::run_stdin(
                        model_settings::Operation::Apply,
                        Some(f.file.clone()),
                        &mut input.as_slice(),
                    )
                    .write_to(out)
                }),
                _ => render(|out| {
                    memory_settings::run_stdin(
                        memory_settings::Operation::Apply,
                        Some(f.file.clone()),
                        &mut input.as_slice(),
                    )
                    .write_to(out)
                }),
            };
            assert_eq!(applied["outcome"], "verified");
            assert_eq!(f.raw().get("research_policy").cloned(), declaration);
        }
        let input=serde_json::to_vec(&json!({"contract_version":1,"expected_config_sha256":digest(&fs::read(&f.file).unwrap()),"enabled":true,"text":"authored preference"})).unwrap();
        let report = render(|out| {
            personalization::run(
                personalization::Operation::Set,
                Some(f.file.clone()),
                &mut input.as_slice(),
            )
            .write_to(out)
        });
        assert_eq!(report["status"], "ok");
        assert_eq!(f.raw().get("research_policy").cloned(), declaration);
    }
}

#[test]
fn personalization_update_preserves_an_explicit_null_policy_declaration() {
    let f = Fixture::new();
    let mut raw = f.raw();
    raw["research_policy"] = Value::Null;
    f.write(&raw);
    let input = serde_json::to_vec(&json!({"contract_version":1,
        "expected_config_sha256":digest(&fs::read(&f.file).unwrap()),
        "enabled":true,"text":"authored preference"}))
    .unwrap();
    let report = super::super::personalization::run(
        super::super::personalization::Operation::Set,
        Some(f.file.clone()),
        &mut input.as_slice(),
    );
    let mut output = Vec::new();
    report.write_to(&mut output).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap()["status"],
        "ok"
    );
    assert_eq!(f.raw().get("research_policy"), Some(&Value::Null));
}
