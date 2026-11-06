//! Real config-only publication on retained notebook history. No runtime activation.
use super::{product_settings as product, research_settings as research};
use crate::storage::{digest, maintenance::Refusal, sync::mode::SyncModeChoice, *};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if meta.is_dir() {
                out.insert(relative, b"directory".to_vec());
                walk(root, &path, out);
            } else if meta.is_symlink() {
                out.insert(
                    relative,
                    fs::read_link(path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                );
            } else {
                out.insert(relative, fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub(super) fn seed(paths: &StorePaths) {
    let store = Store::open(paths.clone()).unwrap();
    let bytes = b"retained synthetic notebook media";
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
    drop(Store::prepare_notebook(paths.clone()).unwrap());
}
struct Fixture {
    root: PathBuf,
    config: PathBuf,
    paths: StorePaths,
}
impl Fixture {
    fn new() -> Self {
        Self::with_mode(None)
    }
    fn with_mode(mode: Option<SyncModeChoice>) -> Self {
        Self::with_options(mode, false)
    }
    fn with_options(mode: Option<SyncModeChoice>, features: bool) -> Self {
        let root = std::env::temp_dir().join(format!("notebook-preferences-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let mut store = Store::open(paths.clone()).unwrap();
        if features {
            let generation: Uuid =
                serde_json::from_slice(&fs::read(paths.data.join("CURRENT")).unwrap()).unwrap();
            selection::prepare_generation(
                &paths.data.join("generations").join(generation.to_string()),
                store.actor_id,
                generation,
            )
            .unwrap();
            feedback_generation::prepare(&mut store, |_| Ok(())).unwrap();
        }
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
            if !ordinary {
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
        seed(&paths);
        if let Some((false, bytes)) = retained_mode {
            // Retain a previously valid nonordinary mode to exercise refusal of unsupported persisted state.
            fs::write(paths.data.join("sync/mode.json"), bytes).unwrap();
        }
        let config = root.join("config.json");
        fs::write(
            &config,
            serde_json::to_vec_pretty(&json!({"paths":paths,"model":null,
            "personalization":{"enabled":false,"text":"PRIVATE exact\r\nα"},"memory":{},
            "base_url":"https://private.invalid/?PRIVATE"}))
            .unwrap(),
        )
        .unwrap();
        Self {
            root,
            config,
            paths,
        }
    }
    fn generation(&self) -> PathBuf {
        let id: Uuid =
            serde_json::from_slice(&fs::read(self.paths.data.join("CURRENT")).unwrap()).unwrap();
        self.paths.data.join("generations").join(id.to_string())
    }
    fn run(&self, product: bool, operation: &str, input: &Value) -> Value {
        let mut output = Vec::new();
        let bytes = serde_json::to_vec(input).unwrap();
        if product {
            let op = match operation {
                "inspect" => product::Operation::Inspect,
                "prepare" => product::Operation::Prepare,
                "apply" => product::Operation::Apply,
                _ => product::Operation::Status,
            };
            (if operation == "inspect" {
                product::inspect(Some(self.config.clone()))
            } else {
                product::run_stdin(op, Some(self.config.clone()), &mut bytes.as_slice())
            })
            .write_to(&mut output)
            .unwrap();
        } else {
            let op = match operation {
                "inspect" => research::Operation::Inspect,
                "prepare" => research::Operation::Prepare,
                "apply" => research::Operation::Apply,
                _ => research::Operation::Status,
            };
            (if operation == "inspect" {
                research::inspect(Some(self.config.clone()))
            } else {
                research::run_stdin(op, Some(self.config.clone()), &mut bytes.as_slice())
            })
            .write_to(&mut output)
            .unwrap();
        }
        assert!(!String::from_utf8_lossy(&output).contains("PRIVATE"));
        serde_json::from_slice(&output).unwrap()
    }
    fn prepared(&self, product: bool) -> Value {
        let current = self.run(product, "inspect", &Value::Null);
        let changes = if product {
            json!({"reader_enabled":{"action":"set","value":false}})
        } else {
            json!({"research_policy":{"action":"set","value":"disabled"}})
        };
        let mut request = json!({"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],
            "config_path_sha256":current["data"]["config_path_sha256"],"changes":changes});
        request[if product {
            "product_settings_version"
        } else {
            "research_settings_version"
        }] = json!(1);
        let result = self.run(product, "prepare", &request);
        assert_eq!(result["outcome"], "prepared");
        if product {
            json!({"intent":result["data"]["intent"]})
        } else {
            json!({"intent":result["data"]["intent"],"changes":changes})
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let path = self.root.canonicalize().unwrap();
            assert_eq!(
                path.parent(),
                Some(std::env::temp_dir().canonicalize().unwrap().as_path())
            );
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("notebook-preferences-"));
            fs::remove_dir_all(path).unwrap();
        }
    }
}

#[test]
fn notebook_preferences_save_reopen_status_preserve_history_and_private_declarations() {
    for product in [true, false] {
        for (local_mode, features) in [(false, false), (true, false), (false, true), (true, true)] {
            let f =
                Fixture::with_options(local_mode.then_some(SyncModeChoice::LocalOnly), features);
            // Incomplete stage files are retained, never cleaned by maintenance recovery.
            fs::write(
                f.generation().join("commits/.stage-retain"),
                b"retain incomplete stage",
            )
            .unwrap();
            let before = inventory(&f.root);
            let input = f.prepared(product);
            assert_eq!(
                f.run(product, "status", &json!({"intent":input["intent"]}))["outcome"],
                "unresolved"
            );
            let owner = Store::open(f.paths.clone()).unwrap();
            assert_eq!(f.run(product, "apply", &input)["status"], "busy");
            drop(owner);
            let saved = f.run(product, "apply", &input);
            assert_eq!(saved["outcome"], "verified");
            assert_eq!(saved["disk_readback_verified"], true);
            assert_eq!(
                saved["runtime_activation"],
                if product {
                    "restart-required"
                } else {
                    "unverified"
                }
            );
            let status = f.run(product, "status", &json!({"intent":input["intent"]}));
            assert_eq!(status["outcome"], "desired-file-observed");
            assert_eq!(status["disk_readback_verified"], false);
            assert_eq!(f.run(product, "apply", &input)["status"], "conflict");
            let mut expected = before.clone();
            expected.remove(Path::new("config.json"));
            let mut after = inventory(&f.root);
            after.remove(Path::new("config.json"));
            assert_eq!(after, expected);
            let original: Value =
                serde_json::from_slice(&before[Path::new("config.json")]).unwrap();
            let current: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
            for (key, value) in original.as_object().unwrap() {
                assert_eq!(&current[key], value);
            }
            drop(Store::open(f.paths.clone()).unwrap());
        }
    }
}

#[test]
fn notebook_preferences_refuse_readiness_ownership_corruption_modes_and_residue_without_repair() {
    for product in [true, false] {
        for case in 0..15 {
            let mode = match case {
                8 => Some(SyncModeChoice::IsolatedBackup {
                    binding_sha256: digest(b"fixture"),
                }),
                9 => Some(SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: digest(b"fixture"),
                    root_id: "fixture".into(),
                }),
                _ => None,
            };
            let f = Fixture::with_mode(mode);
            let input = f.prepared(product);
            let generation = f.generation();
            match case {
                0 => fs::remove_file(f.paths.data.join("capabilities.json")).unwrap(),
                1 => fs::write(f.paths.data.join("contract-v3.json"), b"wrong").unwrap(),
                2 => {
                    let p = generation.join("notebook-feature.json");
                    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    v["actor"] = json!(Uuid::new_v4());
                    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
                3 => fs::write(generation.join("format.json"), b"1").unwrap(),
                4 => {
                    fs::create_dir_all(generation.join("selections")).unwrap();
                    fs::write(generation.join("selections/residue"), b"private").unwrap();
                }
                5 => {
                    fs::create_dir_all(generation.join("selection-history")).unwrap();
                    fs::write(generation.join("selection-history/residue"), b"private").unwrap();
                }
                6 => fs::write(generation.join("selection-history"), b"not a directory").unwrap(),
                7 => fs::write(f.paths.data.join("sync/mode.json"), b"invalid mode").unwrap(),
                8 | 9 => {}
                10 => {
                    let p = f.paths.cache.join("owner.json");
                    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    v["actor"] = json!(Uuid::new_v4());
                    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
                11 => fs::write(generation.join("commits/bad.json"), b"invalid commit").unwrap(),
                12 => {
                    let p = fs::read_dir(generation.join("objects"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path();
                    fs::write(p, b"corrupt retained object").unwrap();
                }
                13 => {
                    let p = fs::read_dir(generation.join("commits"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path();
                    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    v["scope"] = json!("selected-records");
                    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
                _ => fs::write(
                    f.paths.data.join("CURRENT"),
                    serde_json::to_vec(&Uuid::new_v4()).unwrap(),
                )
                .unwrap(),
            }
            let before = inventory(&f.root);
            let result = f.run(product, "apply", &input);
            assert!(
                matches!(
                    result["status"].as_str(),
                    Some("unsupported" | "unavailable")
                ),
                "case{case}: {result}"
            );
            assert_eq!(inventory(&f.root), before, "case{case}");
        }
    }
}

#[test]
fn notebook_preferences_do_not_widen_other_admissions_and_require_definite_empty_directories() {
    let f = Fixture::new();
    let before = inventory(&f.root);
    assert!(matches!(
        Store::open_existing(f.paths.clone()),
        Err(Refusal::Unsupported)
    ));
    assert!(matches!(
        Store::open_existing_for_mode(f.paths.clone()),
        Err(Refusal::Unsupported)
    ));
    assert!(matches!(
        Store::open_existing_for_archive(f.paths.clone()),
        Err(Refusal::Unsupported)
    ));
    assert!(matches!(
        Store::open_existing_for_outbox(f.paths.clone()),
        Err(Refusal::Unsupported)
    ));
    drop(Store::open_existing_for_notebook_restore(f.paths.clone()).unwrap());
    assert_eq!(inventory(&f.root), before);
    for name in ["selections", "selection-history"] {
        fs::create_dir_all(f.generation().join(name)).unwrap();
    }
    drop(Store::open_existing_for_preferences(f.paths.clone()).unwrap());
}

#[test]
#[cfg(unix)]
fn notebook_preferences_refuse_linked_selected_directories_without_following() {
    for name in ["selections", "selection-history"] {
        let f = Fixture::new();
        let empty = f.root.join("outside-empty");
        fs::create_dir(&empty).unwrap();
        std::os::unix::fs::symlink(&empty, f.generation().join(name)).unwrap();
        let before = inventory(&f.root);
        assert!(matches!(
            Store::open_existing_for_preferences(f.paths.clone()),
            Err(Refusal::Unsupported)
        ));
        assert_eq!(inventory(&f.root), before);
    }
}
