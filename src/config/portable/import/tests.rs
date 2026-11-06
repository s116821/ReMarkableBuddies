use super::*;
use crate::{
    config::notebook_preferences_tests::{inventory, seed},
    storage::{sync::mode::SyncModeChoice, ConfigFault, StorePaths},
};
use std::{fs, io, path::Path};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    paths: StorePaths,
}
impl Fixture {
    fn new(notebook: bool, local: bool) -> Self {
        Self::with_mode(notebook, local.then_some(SyncModeChoice::LocalOnly))
    }
    fn with_mode(notebook: bool, mode: Option<SyncModeChoice>) -> Self {
        let root = std::env::temp_dir().join(format!("portable-import-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let store = Store::open(paths.clone()).unwrap();
        let retained_mode = mode.map(|mode| {
            let ordinary = matches!(mode, SyncModeChoice::LocalOnly);
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    mode,
                )
                .unwrap();
            let bytes = fs::read(paths.data.join("sync/mode.json")).unwrap();
            if !ordinary && notebook {
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        SyncModeChoice::LocalOnly,
                    )
                    .unwrap();
            }
            (ordinary, bytes)
        });
        drop(store);
        if notebook {
            seed(&paths);
            if let Some((false, bytes)) = retained_mode {
                fs::write(paths.data.join("sync/mode.json"), bytes).unwrap();
            }
        }
        let path = root.join("config.json");
        fs::write(&path, serde_json::to_vec_pretty(&json!({"paths":paths,"model":"private-model",
            "base_url":"https://PRIVATE.invalid/v1","model_credential":"private_key",
            "personalization":{"enabled":false,"text":"PRIVATE retained\r\nα"},
            "product_settings":{"reader_enabled":false,"writer_enabled":true},"research_policy":"disabled"})).unwrap()).unwrap();
        Self { root, path, paths }
    }
    fn run(&self, op: Operation, request: &Value) -> Report {
        run_stdin(
            op,
            Some(self.path.clone()),
            &mut serde_json::to_vec(request).unwrap().as_slice(),
        )
    }
    fn prepared(&self, artifact: &Value) -> Value {
        let d = finish(inspect(Some(self.path.clone())));
        finish(self.run(Operation::Prepare,&json!({"portable_import_version":1,"operation":Uuid::new_v4(),
            "base_sha256":d["data"]["base_sha256"],"config_path_sha256":d["data"]["config_path_sha256"],"artifact":artifact})))
    }
    fn generation(&self) -> PathBuf {
        let id: Uuid =
            serde_json::from_slice(&fs::read(self.paths.data.join("CURRENT")).unwrap()).unwrap();
        self.paths.data.join("generations").join(id.to_string())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn finish(report: Report) -> Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert!(bytes.len() <= MAX_REPORT);
    assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
    let raw = String::from_utf8(bytes.clone()).unwrap();
    for private in ["PRIVATE", "retained", "private_key", "private-model"] {
        assert!(!raw.contains(private));
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["runtime_activation"], "unverified");
    assert_eq!(value.as_object().unwrap().len(), 7);
    value
}
fn portable(settings: Value) -> Value {
    json!({"portable_configuration_version":1,"settings":settings})
}
fn changed() -> Value {
    portable(
        json!({"personalization":{"enabled":true,"text":"PRIVATE imported é\r\n日本語\t😀"},"product_settings":{"reader_enabled":true},"research_policy":null}),
    )
}

#[test]
fn actual_store_and_media_notebook_overlay_preserves_excluded_and_omitted_bytes() {
    for notebook in [false, true] {
        for local in [false, true] {
            let f = Fixture::new(notebook, local);
            let artifact = changed();
            let old: Value = serde_json::from_slice(&fs::read(&f.path).unwrap()).unwrap();
            let before = inventory(&f.root);
            let prepared = f.prepared(&artifact);
            assert_eq!(prepared["outcome"], "prepared");
            assert_eq!(
                prepared["data"]["changed_settings"],
                json!(["personalization", "product_settings", "research_policy"])
            );
            assert_eq!(inventory(&f.root), before);
            let intent = prepared["data"]["intent"].clone();
            assert_eq!(intent.as_object().unwrap().len(), 6);
            let report = finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"artifact":artifact}),
            ));
            assert_eq!(report["outcome"], "verified");
            assert_eq!(report["disk_readback_verified"], true);
            let new: Value = serde_json::from_slice(&fs::read(&f.path).unwrap()).unwrap();
            assert_eq!(
                new["product_settings"],
                json!({"reader_enabled":true,"writer_enabled":true})
            );
            assert_eq!(
                new["personalization"],
                artifact["settings"]["personalization"]
            );
            assert!(new.as_object().unwrap().contains_key("research_policy"));
            assert!(new["research_policy"].is_null());
            for key in ["paths", "model", "base_url", "model_credential"] {
                assert_eq!(new[key], old[key]);
            }
            let mut after = inventory(&f.root);
            after.insert(
                PathBuf::from("config.json"),
                before[Path::new("config.json")].clone(),
            );
            assert_eq!(after, before);
            assert_eq!(
                finish(f.run(Operation::Status, &json!({"intent":intent})))["outcome"],
                "desired-file-observed"
            );
        }
    }
}

#[test]
fn declaration_overlay_empty_null_sparse_and_noop_never_materializes_defaults() {
    let f = Fixture::new(false, false);
    for (original, settings, expected) in [
        (json!({}), json!({}), json!({})),
        (
            json!({"product_settings":null}),
            json!({"product_settings":{}}),
            json!({"product_settings":{}}),
        ),
        (
            json!({}),
            json!({"product_settings":null}),
            json!({"product_settings":null}),
        ),
        (
            json!({"product_settings":{"reader_enabled":true,"writer_enabled":false}}),
            json!({"product_settings":{}}),
            json!({"product_settings":{"reader_enabled":true,"writer_enabled":false}}),
        ),
        (
            json!({"personalization":{"enabled":true,"text":"PRIVATE kept"}}),
            json!({"personalization":{"enabled":false}}),
            json!({"personalization":{"enabled":false,"text":"PRIVATE kept"}}),
        ),
        (
            json!({"personalization":{}}),
            json!({"personalization":{}}),
            json!({"personalization":{}}),
        ),
    ] {
        let bytes = serde_json::to_vec_pretty(&original).unwrap();
        fs::write(&f.path, &bytes).unwrap();
        let doc = Document::load(Some(f.path.clone())).unwrap();
        let a = portable(settings);
        artifact(&a).unwrap();
        let next = target(&doc, &a).unwrap();
        assert_eq!(next.raw, expected);
        let report = f.prepared(&a);
        assert_eq!(
            report["outcome"],
            if original == expected {
                "no-change"
            } else {
                "prepared"
            }
        );
        assert_eq!(fs::read(&f.path).unwrap(), bytes);
        if original == expected {
            assert!(report["data"].get("intent").is_none());
        }
    }
}

#[test]
fn no_op_inspection_prepare_status_do_not_require_or_open_roots() {
    let f = Fixture::new(false, false);
    let artifact = changed();
    let intent = f.prepared(&artifact)["data"]["intent"].clone();
    let owner = Store::open_existing(f.paths.clone()).unwrap();
    assert_eq!(f.prepared(&portable(json!({})))["outcome"], "no-change");
    assert_eq!(finish(inspect(Some(f.path.clone())))["status"], "ok");
    assert_eq!(
        finish(f.run(Operation::Status, &json!({"intent":intent})))["outcome"],
        "unresolved"
    );
    assert_eq!(
        finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"artifact":artifact})
        ))["status"],
        "busy"
    );
    drop(owner);
    for root in [&f.paths.data, &f.paths.cache, &f.paths.credentials] {
        fs::remove_dir_all(root).unwrap();
    }
    assert_eq!(f.prepared(&portable(json!({})))["outcome"], "no-change");
    assert_eq!(
        finish(f.run(Operation::Status, &json!({"intent":intent})))["status"],
        "ok"
    );
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 1);
}

#[test]
fn exact_original_artifact_path_target_and_stale_conditions_refuse_before_store() {
    let f = Fixture::new(true, false);
    let a = changed();
    let intent = f.prepared(&a)["data"]["intent"].clone();
    let before = inventory(&f.root);
    let owner = Store::open_existing_for_portable_preferences(f.paths.clone()).unwrap();
    for key in [
        "base_sha256",
        "target_sha256",
        "artifact_sha256",
        "config_path_sha256",
    ] {
        let mut i = intent.clone();
        i[key] = json!("0".repeat(64));
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":i,"artifact":a})))["status"],
            "conflict"
        );
    }
    assert_eq!(
        finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"artifact":portable(json!({}))})
        ))["status"],
        "conflict"
    );
    let alternate = f.root.join("other.json");
    fs::copy(&f.path, &alternate).unwrap();
    assert_eq!(
        finish(run_stdin(
            Operation::Apply,
            Some(alternate.clone()),
            &mut serde_json::to_vec(&json!({"intent":intent,"artifact":a}))
                .unwrap()
                .as_slice()
        ))["status"],
        "conflict"
    );
    assert_eq!(
        finish(run_stdin(
            Operation::Status,
            Some(alternate.clone()),
            &mut serde_json::to_vec(&json!({"intent":intent}))
                .unwrap()
                .as_slice()
        ))["status"],
        "conflict"
    );
    fs::remove_file(alternate).unwrap();
    drop(owner);
    assert_eq!(inventory(&f.root), before);
    let mut newer = fs::read(&f.path).unwrap();
    newer.push(b' ');
    fs::write(&f.path, &newer).unwrap();
    assert_eq!(
        finish(f.run(Operation::Apply, &json!({"intent":intent,"artifact":a})))["status"],
        "conflict"
    );
    assert_eq!(fs::read(&f.path).unwrap(), newer);
}

#[test]
fn strict_requests_artifacts_originals_and_versions_refuse_without_disclosure() {
    let f = Fixture::new(false, false);
    for mut raw in [br#"{}"#.as_slice(),br#"[]"#,br#"{}{}"#,br#"{"intent":null}"#,br#"{"intent":{},"intent":{}}"#,br#"{"intent":{},"artifact":{"settings":{"personalization":{"text":"PRIVATE","text":"duplicate"}}}}"#] {
        assert_eq!(finish(run_stdin(Operation::Apply,Some(f.path.clone()),&mut raw))["status"],"invalid-input");
    }
    let a = changed();
    let good = f.prepared(&a)["data"]["intent"].clone();
    for key in [
        "portable_import_version",
        "operation",
        "base_sha256",
        "target_sha256",
        "artifact_sha256",
        "config_path_sha256",
        "extra",
    ] {
        let mut intent = good.clone();
        intent[key] = json!(null);
        assert_eq!(
            finish(f.run(Operation::Status, &json!({"intent":intent})))["status"],
            "invalid-input"
        );
    }
    for v in [json!(2), json!(1.0), json!(true)] {
        let mut intent = good.clone();
        intent["portable_import_version"] = v.clone();
        assert_eq!(
            finish(f.run(Operation::Status, &json!({"intent":intent})))["status"],
            if v == json!(2) {
                "unsupported"
            } else {
                "invalid-input"
            }
        );
    }
    for settings in [
        json!({"personalization":null}),
        json!({"product_settings":{"unknown":false}}),
        json!({"paths":{}}),
        json!({"research_policy":[]}),
    ] {
        assert_eq!(f.prepared(&portable(settings))["status"], "invalid-input");
    }
    for (bytes, status) in [
        (br#"{"model":""}"#.as_slice(), "invalid-input"),
        (br#"{"config_schema_version":2}"#, "unsupported"),
        (
            br#"{"personalization":{"schema_version":2}}"#,
            "unsupported",
        ),
        (
            br#"{"log_level":"info","log_level":"debug"}"#,
            "invalid-input",
        ),
    ] {
        fs::write(&f.path, bytes).unwrap();
        assert_eq!(finish(inspect(Some(f.path.clone())))["status"], status);
    }
}

#[test]
fn inclusive_request_artifact_config_and_target_bounds_are_independent() {
    let f = Fixture::new(false, false);
    let a = changed();
    let intent = f.prepared(&a)["data"]["intent"].clone();
    let mut request = serde_json::to_vec(&json!({"intent":intent})).unwrap();
    request.resize(MAX_REQUEST, b' ');
    assert_eq!(
        finish(run_stdin(
            Operation::Status,
            Some(f.path.clone()),
            &mut request.as_slice()
        ))["status"],
        "ok"
    );
    request.push(b' ');
    assert_eq!(
        finish(run_stdin(
            Operation::Status,
            Some(f.path.clone()),
            &mut request.as_slice()
        ))["status"],
        "invalid-input"
    );
    // Large legal whitespace is framing only: compact artifact limits do not use it.
    let d = finish(inspect(Some(f.path.clone())));
    let req = json!({"portable_import_version":1,"operation":Uuid::new_v4(),"base_sha256":d["data"]["base_sha256"],"config_path_sha256":d["data"]["config_path_sha256"],"artifact":portable(json!({}))});
    let mut raw = serde_json::to_vec(&req).unwrap();
    raw.resize(MAX_REQUEST, b' ');
    assert_eq!(
        finish(run_stdin(
            Operation::Prepare,
            Some(f.path.clone()),
            &mut raw.as_slice()
        ))["outcome"],
        "no-change"
    );
    let huge = portable(json!({"personalization":{"text":"x".repeat(super::super::MAX_IO)}}));
    assert!(matches!(artifact(&huge), Err(Status::InvalidInput)));
    let mut raw = fs::read(&f.path).unwrap();
    raw.resize(MAX_RECORD, b' ');
    fs::write(&f.path, &raw).unwrap();
    assert_eq!(finish(inspect(Some(f.path.clone())))["status"], "ok");
    raw.push(b' ');
    fs::write(&f.path, raw).unwrap();
    assert_eq!(
        finish(inspect(Some(f.path.clone())))["status"],
        "unavailable"
    );
    // Valid long excluded root strings remain raw values; target cap is enforced before Store access.
    let long = "x".repeat((MAX_RECORD - 512) / 3);
    let config = json!({"paths":{"data":long,"cache":long,"credentials":long}});
    fs::write(&f.path, serde_json::to_vec(&config).unwrap()).unwrap();
    let oversized = portable(json!({"personalization":{"text":"a".repeat(2048)}}));
    assert_eq!(f.prepared(&oversized)["status"], "invalid-input");
}

#[test]
fn unsupported_legacy_notebook_readiness_history_mode_refuses_without_repair() {
    for notebook in [false, true] {
        let cases = if notebook { 0..13 } else { 13..17 };
        for case in cases {
            let mode = match case {
                11 | 15 => Some(SyncModeChoice::IsolatedBackup {
                    binding_sha256: digest(b"binding"),
                }),
                12 | 16 => Some(SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: digest(b"binding"),
                    root_id: "fixture".into(),
                }),
                _ => None,
            };
            let f = Fixture::with_mode(notebook, mode);
            let a = changed();
            let intent = f.prepared(&a)["data"]["intent"].clone();
            let generation = f.generation();
            match case {
                0 => fs::remove_file(f.paths.data.join("capabilities.json")).unwrap(),
                1 => fs::write(f.paths.data.join("contract-v3.json"), b"bad").unwrap(),
                2 => {
                    let p = generation.join("notebook-feature.json");
                    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    v["actor"] = json!(Uuid::new_v4());
                    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
                3 => fs::write(generation.join("format.json"), b"1").unwrap(),
                4 | 5 => {
                    let p = generation.join(if case == 4 {
                        "selections"
                    } else {
                        "selection-history"
                    });
                    fs::create_dir_all(&p).unwrap();
                    fs::write(p.join("residue"), b"retained").unwrap();
                }
                6 => fs::write(generation.join("selection-history"), b"not-directory").unwrap(),
                7 => fs::write(f.paths.data.join("sync/mode.json"), b"invalid").unwrap(),
                8 => {
                    let p = f.paths.cache.join("owner.json");
                    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    v["actor"] = json!(Uuid::new_v4());
                    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
                9 => fs::write(generation.join("commits/bad.json"), b"invalid").unwrap(),
                10 => {
                    let p = fs::read_dir(generation.join("objects"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path();
                    fs::write(p, b"corrupt").unwrap();
                }
                11 | 12 | 15 | 16 => {}
                13 | 14 => fs::write(
                    generation.join("format.json"),
                    if case == 13 { b"2" } else { b"3" },
                )
                .unwrap(),
                _ => unreachable!(),
            }
            let before = inventory(&f.root);
            let r = finish(f.run(Operation::Apply, &json!({"intent":intent,"artifact":a})));
            assert_ne!(r["status"], "ok", "case {case}");
            assert_eq!(r["outcome"], "not-dispatched");
            assert_eq!(inventory(&f.root), before, "case {case}");
        }
    }
}

#[test]
fn canonical_config_owned_root_and_root_alias_overlap_refuses() {
    for case in 0..4 {
        let f = Fixture::new(false, false);
        let mut raw: Value = serde_json::from_slice(&fs::read(&f.path).unwrap()).unwrap();
        let path = if case == 0 {
            let p = f.paths.data.join("config.json");
            fs::copy(&f.path, &p).unwrap();
            p
        } else {
            f.path.clone()
        };
        match case {
            1 => raw["paths"]["cache"] = json!(f.paths.data.join("child")),
            2 => raw["paths"]["cache"] = json!(f.paths.data),
            3 => raw["paths"]["credentials"] = json!(f.root),
            _ => (),
        };
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let doc = Document::load(Some(path)).unwrap();
        assert!(doc.identities().is_err());
    }
}

#[cfg(windows)]
#[test]
fn actual_windows_case_aliases_refuse_canonical_overlap_but_keep_exact_path_binding() {
    let f = Fixture::new(false, false);
    let mut raw: Value = serde_json::from_slice(&fs::read(&f.path).unwrap()).unwrap();
    raw["paths"]["cache"] = json!(f.paths.data.to_str().unwrap().to_uppercase());
    fs::write(&f.path, serde_json::to_vec(&raw).unwrap()).unwrap();
    assert!(Document::load(Some(f.path.clone()))
        .unwrap()
        .identities()
        .is_err());
    let original = Document::load(Some(f.path.clone())).unwrap();
    let alias =
        Document::load(Some(PathBuf::from(f.path.to_str().unwrap().to_uppercase()))).unwrap();
    assert_eq!(original.hash, alias.hash);
    assert_ne!(original.path_hash, alias.path_hash);
}

#[test]
fn original_publisher_faults_keep_unknown_and_status_is_observation_only() {
    for notebook in [false, true] {
        for fault in [
            ConfigFault::AtomicCall,
            ConfigFault::AfterPublication,
            ConfigFault::ChangedReadback,
        ] {
            let f = Fixture::new(notebook, false);
            let a = changed();
            let intent: Intent =
                serde_json::from_value(f.prepared(&a)["data"]["intent"].clone()).unwrap();
            let before = inventory(&f.paths.data);
            let doc = Document::load(Some(f.path.clone())).unwrap();
            let next = target(&doc, &a).unwrap();
            let report = apply_with(doc, intent.clone(), next, |s, p, e, b| {
                let mode = s.sync_mode_snapshot().unwrap();
                assert!(s
                    .set_sync_mode(&mode.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
                    .is_err());
                s.replace_config_document_fault(p, e, b, fault)
            })
            .unwrap();
            assert_eq!(report.outcome, "unknown");
            assert_eq!(report.status, Status::Failed);
            assert!(!report.disk_readback_verified);
            finish(report);
            let bytes = fs::read(&f.path).unwrap();
            let status = finish(f.run(Operation::Status, &json!({"intent":intent})));
            assert_eq!(
                status["outcome"],
                if digest(&bytes) == intent.target_sha256 {
                    "desired-file-observed"
                } else {
                    "unresolved"
                }
            );
            assert_eq!(fs::read(&f.path).unwrap(), bytes);
            assert_eq!(inventory(&f.paths.data), before);
        }
    }
}

#[test]
fn consuming_output_holds_real_ownership_through_success_immediate_and_prefix_failure() {
    for notebook in [false, true] {
        for prefix in [None, Some(0), Some(17)] {
            let f = Fixture::new(notebook, true);
            let a = changed();
            let intent = f.prepared(&a)["data"]["intent"].clone();
            let report = f.run(Operation::Apply, &json!({"intent":intent,"artifact":a}));
            assert_eq!(report.status, Status::Ok);
            struct Probe<'a> {
                paths: &'a StorePaths,
                prefix: Option<usize>,
                bytes: Vec<u8>,
                calls: usize,
            }
            impl Write for Probe<'_> {
                fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                    assert!(matches!(
                        Store::open_existing_for_portable_preferences(self.paths.clone()),
                        Err(Refusal::Busy)
                    ));
                    self.calls += 1;
                    match self.prefix {
                        Some(n) if self.calls == 1 && n > 0 => {
                            self.bytes.extend_from_slice(&bytes[..n]);
                            Ok(n)
                        }
                        Some(_) => Err(io::Error::other("private output stopped")),
                        None => {
                            self.bytes.extend_from_slice(bytes);
                            Ok(bytes.len())
                        }
                    }
                }
                fn flush(&mut self) -> io::Result<()> {
                    panic!("mutating protocol must not flush")
                }
            }
            let mut output = Probe {
                paths: &f.paths,
                prefix,
                bytes: vec![],
                calls: 0,
            };
            assert_eq!(report.write_to(&mut output).is_ok(), prefix.is_none());
            if let Some(n) = prefix {
                assert_eq!(output.bytes.len(), n);
                assert_eq!(output.calls, if n == 0 { 1 } else { 2 });
            }
            drop(Store::open_existing_for_portable_preferences(f.paths.clone()).unwrap());
            assert_eq!(
                finish(f.run(Operation::Status, &json!({"intent":intent})))["outcome"],
                "desired-file-observed"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn linked_config_or_selected_directories_and_special_files_refuse() {
    let f = Fixture::new(true, false);
    let alias = f.root.join("alias.json");
    std::os::unix::fs::symlink(&f.path, &alias).unwrap();
    assert_eq!(finish(inspect(Some(alias)))["status"], "unavailable");
    assert_eq!(
        finish(inspect(Some(PathBuf::from("/dev/null"))))["status"],
        "unavailable"
    );
    let a = changed();
    let intent = f.prepared(&a)["data"]["intent"].clone();
    let outside = f.root.join("empty");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(outside, f.generation().join("selections")).unwrap();
    assert_eq!(
        finish(f.run(Operation::Apply, &json!({"intent":intent,"artifact":a})))["status"],
        "unsupported"
    );
}
