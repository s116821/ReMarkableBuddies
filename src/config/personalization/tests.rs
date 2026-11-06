use super::*;
use crate::{
    config::notebook_preferences_tests::inventory,
    storage::{sync::mode::SyncModeChoice, *},
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    io::{self, Write},
};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    paths: StorePaths,
}
impl Fixture {
    fn new(version: u32) -> Self {
        Self::with_mode(version, None)
    }
    fn with_mode(version: u32, mode: Option<SyncModeChoice>) -> Self {
        let root =
            std::env::temp_dir().join(format!("personalization-document-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let mut store = Store::open(paths.clone()).unwrap();
        let media = b"retained synthetic personalization history";
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
        if version == 2 {
            let generation: Uuid = files::json(&paths.data.join("CURRENT"), 128).unwrap();
            selection::prepare_generation(
                &paths.data.join("generations").join(generation.to_string()),
                store.actor_id,
                generation,
            )
            .unwrap();
        }
        if version == 3 {
            feedback_generation::prepare(&mut store, |_| Ok(())).unwrap();
        }
        let retained_mode = mode.map(|mode| {
            let local = matches!(mode, SyncModeChoice::LocalOnly);
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    mode,
                )
                .unwrap();
            let bytes = fs::read(paths.data.join("sync/mode.json")).unwrap();
            if version == 4 && !local {
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        SyncModeChoice::LocalOnly,
                    )
                    .unwrap();
            }
            (local, bytes)
        });
        drop(store);
        if version == 4 {
            drop(Store::prepare_notebook(paths.clone()).unwrap());
            if let Some((false, bytes)) = retained_mode {
                fs::write(paths.data.join("sync/mode.json"), bytes).unwrap();
            }
        }
        let path = root.join("config.json");
        fs::write(&path,serde_json::to_vec_pretty(&json!({"paths":paths,"model":null,"base_url":"https://private.invalid/SIBLING_KEEP",
            "memory":{},"reader":{},"product_settings":{"writer_enabled":false},"research_policy":null,
            "personalization":{"text":" old disabled\r\nα "}})).unwrap()).unwrap();
        Self { root, path, paths }
    }
    fn raw(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.path).unwrap()).unwrap()
    }
    fn request(&self) -> Value {
        json!({"contract_version":1,"expected_config_sha256":digest(&fs::read(&self.path).unwrap())})
    }
    fn set(&self, text: &str) -> Value {
        let mut v = self.request();
        v["enabled"] = json!(true);
        v["text"] = json!(text);
        v
    }
    fn run(&self, operation: Operation, input: &Value) -> Report {
        run(
            operation,
            Some(self.path.clone()),
            &mut serde_json::to_vec(input).unwrap().as_slice(),
        )
    }
    fn generation(&self) -> PathBuf {
        let generation: Uuid = files::json(&self.paths.data.join("CURRENT"), 128).unwrap();
        self.paths
            .data
            .join("generations")
            .join(generation.to_string())
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
                .starts_with("personalization-document-"));
            fs::remove_dir_all(path).unwrap();
        }
    }
}
fn frame(report: &Report) -> Value {
    let mut bytes = vec![];
    report.write_to(&mut bytes).unwrap();
    assert!(bytes.len() <= MAX_IO_BYTES && bytes.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&bytes).contains("SIBLING_KEEP"));
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 7);
    value
}
fn without_personalization(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("personalization");
    value
}

#[test]
fn raw_declarations_and_retained_records_survive_all_mutations_and_reload() {
    for version in [1, 4] {
        for local in [false, true] {
            let f = Fixture::with_mode(version, local.then_some(SyncModeChoice::LocalOnly));
            fs::write(
                f.generation().join("commits/.stage-retained"),
                b"retained stage",
            )
            .unwrap();
            let original = f.raw();
            let before = inventory(&f.root);
            let old = Config::load(Some(f.path.clone()))
                .unwrap()
                .personalization_snapshot()
                .unwrap();
            let text = "  Current preference 🦊\r\n第二行\n\t ";
            let saved = f.run(Operation::Set, &f.set(text));
            assert_eq!(saved.status, Status::Ok);
            assert!(saved.disk_reload_verified);
            assert_eq!(saved.runtime_activation, "restart-required");
            assert_eq!(frame(&saved)["personalization"]["text"], text);
            assert_eq!(
                without_personalization(f.raw()),
                without_personalization(original.clone())
            );
            let in_flight = Config::load(Some(f.path.clone()))
                .unwrap()
                .personalization_snapshot()
                .unwrap();
            assert_eq!(in_flight.effective_text(), Some(text));
            assert!(old.effective_text().is_none());
            let disabled = f.run(Operation::Disable, &f.request());
            assert_eq!(disabled.status, Status::Ok);
            let value = frame(&disabled);
            assert_eq!(value["personalization"]["enabled"], false);
            assert_eq!(value["personalization"]["text"], text);
            let cleared = f.run(Operation::Clear, &f.request());
            assert_eq!(cleared.status, Status::Ok);
            assert_eq!(cleared.personalization.unwrap(), Personalization::default());
            assert_eq!(in_flight.effective_text(), Some(text));
            assert_eq!(
                without_personalization(f.raw()),
                without_personalization(original)
            );
            let mut after = inventory(&f.root);
            let mut expected = before;
            after.remove(Path::new("config.json"));
            expected.remove(Path::new("config.json"));
            assert_eq!(after, expected);
            drop(Store::open_existing_for_personalization(f.paths.clone()).unwrap());
        }
    }
}

#[test]
fn distinct_admission_keeps_legacy_refusals_and_notebook_guards_without_repair() {
    for version in [2, 3] {
        let f = Fixture::new(version);
        let before = inventory(&f.root);
        for op in [Operation::Set, Operation::Disable, Operation::Clear] {
            let input = if matches!(op, Operation::Set) {
                f.set("new")
            } else {
                f.request()
            };
            assert_eq!(f.run(op, &input).status, Status::Unsupported);
        }
        assert_eq!(inventory(&f.root), before);
    }
    for case in 0..8 {
        let mode = match case {
            6 => Some(SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"fixture"),
            }),
            7 => Some(SyncModeChoice::SharedGroup {
                group: Uuid::new_v4(),
                binding_sha256: digest(b"fixture"),
                root_id: "fixture".into(),
            }),
            _ => None,
        };
        let f = Fixture::with_mode(4, mode);
        let input = f.set("new");
        match case {
            0 => fs::remove_file(f.paths.data.join("capabilities.json")).unwrap(),
            1 => fs::write(
                f.paths.cache.join("owner.json"),
                serde_json::to_vec(&json!({"format":1,"actor":Uuid::new_v4()})).unwrap(),
            )
            .unwrap(),
            2 => {
                fs::create_dir_all(f.generation().join("selection-history")).unwrap();
                fs::write(
                    f.generation().join("selection-history/residue"),
                    b"retained",
                )
                .unwrap();
            }
            3 => fs::write(f.paths.data.join("sync/mode.json"), b"invalid").unwrap(),
            4 => fs::write(f.generation().join("commits/bad.json"), b"invalid").unwrap(),
            5 => fs::write(f.generation().join("format.json"), b"1").unwrap(),
            _ => (),
        }
        let before = inventory(&f.root);
        let report = f.run(Operation::Set, &input);
        assert!(matches!(
            report.status,
            Status::Unsupported | Status::Unavailable
        ));
        assert_eq!(inventory(&f.root), before);
    }
    let f = Fixture::new(4);
    let owner = Store::open_existing_for_personalization(f.paths.clone()).unwrap();
    assert_eq!(f.run(Operation::Set, &f.set("new")).status, Status::Busy);
    drop(owner);
    assert!(Store::open_existing(f.paths.clone()).is_err());
    // No new ordinary-mode rule is silently imposed on previously admitted legacy1.
    let f = Fixture::with_mode(
        1,
        Some(SyncModeChoice::IsolatedBackup {
            binding_sha256: digest(b"legacy"),
        }),
    );
    assert_eq!(f.run(Operation::Set, &f.set("legacy")).status, Status::Ok);
}

struct Never;
impl Read for Never {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("unexpected stdin read")
    }
}
#[test]
fn whole_source_and_path_validation_still_precede_stdin_and_inspection_is_read_only() {
    let f = Fixture::new(4);
    let before = inventory(&f.root);
    let owner = Store::open_existing_for_personalization(f.paths.clone()).unwrap();
    let report = run(Operation::Inspect, Some(f.path.clone()), &mut Never);
    assert_eq!(report.status, Status::Ok);
    assert_eq!(report.runtime_activation, "unverified");
    drop(owner);
    assert_eq!(inventory(&f.root), before);
    assert_eq!(
        run(Operation::Set, None, &mut Never).status,
        Status::Unavailable
    );
    for invalid in [
        format!("{{\"paths\":{},\"memory\":{{}},\"memory\":{{}}}}",serde_json::to_string(&f.paths).unwrap()),
        serde_json::to_string(&json!({"paths":{"data":f.paths.data,"cache":f.paths.data,"credentials":f.paths.credentials}})).unwrap(),
        "{\"personalization\":{\"text\":\"PRIVATE\",\"schema_version\":99}}".into(),
    ] {
        fs::write(&f.path,invalid.as_bytes()).unwrap();
        let report=run(Operation::Set,Some(f.path.clone()),&mut Never);assert_eq!(report.status,Status::Unavailable);
        assert!(!frame(&report).to_string().contains("PRIVATE"));assert_eq!(fs::read(&f.path).unwrap(),invalid.as_bytes());
    }
}

#[test]
fn inclusive_document_stdin_and_escaped_utf8_text_bounds() {
    let f = Fixture::new(1);
    let mut bytes = fs::read(&f.path).unwrap();
    bytes.resize(MAX_RECORD, b' ');
    fs::write(&f.path, &bytes).unwrap();
    assert_eq!(
        run(Operation::Inspect, Some(f.path.clone()), &mut Never).status,
        Status::Ok
    );
    bytes.push(b' ');
    fs::write(&f.path, &bytes).unwrap();
    assert_eq!(
        run(Operation::Set, Some(f.path.clone()), &mut Never).status,
        Status::Unavailable
    );
    bytes.pop();
    fs::write(&f.path, &bytes).unwrap();
    let text = "\u{1}".repeat(MAX_TEXT_BYTES);
    let mut input = serde_json::to_vec(&f.set(&text)).unwrap();
    input.resize(MAX_IO_BYTES, b' ');
    let report = run(Operation::Set, Some(f.path.clone()), &mut input.as_slice());
    assert_eq!(report.status, Status::Ok);
    assert_eq!(frame(&report)["personalization"]["text"], text);
    let before = fs::read(&f.path).unwrap();
    for text in ["é".repeat(MAX_TEXT_BYTES / 2) + "x", "bad\0text".into()] {
        assert_eq!(
            f.run(Operation::Set, &f.set(&text)).status,
            Status::InvalidInput
        );
        assert_eq!(fs::read(&f.path).unwrap(), before);
    }
    input = serde_json::to_vec(&f.set("bounded")).unwrap();
    input.resize(MAX_IO_BYTES + 1, b' ');
    assert_eq!(
        run(Operation::Set, Some(f.path.clone()), &mut input.as_slice()).status,
        Status::InvalidInput
    );
    assert_eq!(fs::read(&f.path).unwrap(), before);
}

#[test]
fn writer_failures_keep_v1_status_and_original_readback_custody_not_output_custody() {
    for fault in [
        ConfigFault::AtomicCall,
        ConfigFault::AfterPublication,
        ConfigFault::ChangedReadback,
    ] {
        let f = Fixture::new(4);
        let input = serde_json::to_vec(&f.set("PRIVATE_PREF")).unwrap();
        let data = inventory(&f.paths.data);
        let calls = Cell::new(0);
        let report = run_with(
            Operation::Set,
            Some(f.path.clone()),
            &mut input.as_slice(),
            |store, path, base, target| {
                calls.set(calls.get() + 1);
                assert!(matches!(
                    Store::open_existing_for_personalization(f.paths.clone()),
                    Err(maintenance::Refusal::Busy)
                ));
                store.replace_config_document_fault(path, base, target, fault)
            },
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(report.status, Status::Failed);
        assert!(!report.disk_reload_verified);
        assert!(report.personalization.is_none() && report.config_sha256.is_none());
        assert_eq!(inventory(&f.paths.data), data);
        struct Prefix<'a>(&'a StorePaths, usize);
        impl Write for Prefix<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                // The original public report intentionally owns no Store lease.
                drop(Store::open_existing_for_personalization(self.0.clone()).unwrap());
                self.1 += 1;
                if self.1 == 1 {
                    Ok(bytes.len().min(7))
                } else {
                    Err(io::Error::other("prefix failure"))
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("no implicit flush")
            }
        }
        let observed = fs::read(&f.path).unwrap();
        let mut output = Prefix(&f.paths, 0);
        assert!(report.write_to(&mut output).is_err());
        assert_eq!(output.1, 2);
        assert_eq!(calls.get(), 1);
        assert_eq!(fs::read(&f.path).unwrap(), observed);
    }
    // A lower-level successful readback does not replace this operation's final reload.
    for change_personalization in [false, true] {
        let f = Fixture::new(4);
        let input = serde_json::to_vec(&f.set("original desired")).unwrap();
        let report = run_with(
            Operation::Set,
            Some(f.path.clone()),
            &mut input.as_slice(),
            |store, path, base, target| {
                let result = store.replace_config_document(path, base, target);
                assert!(matches!(result, ConfigPublication::Verified(_)));
                let mut competing: Value =
                    serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                if change_personalization {
                    competing["personalization"]["text"] = json!("competing");
                } else {
                    competing["log_level"] = json!("debug");
                }
                fs::write(path, serde_json::to_vec(&competing).unwrap()).unwrap();
                result
            },
        );
        assert_eq!(report.status, Status::Failed);
        assert!(!report.disk_reload_verified && report.config_sha256.is_none());
        assert_eq!(
            f.raw()[if change_personalization {
                "personalization"
            } else {
                "log_level"
            }],
            if change_personalization {
                json!({"schema_version":1,"enabled":true,"text":"competing"})
            } else {
                json!("debug")
            }
        );
    }
    for result in [
        ConfigPublication::Conflict,
        ConfigPublication::Refused,
        ConfigPublication::Unknown,
    ] {
        let f = Fixture::new(1);
        let input = serde_json::to_vec(&f.set("new")).unwrap();
        let before = inventory(&f.root);
        let report = run_with(
            Operation::Set,
            Some(f.path.clone()),
            &mut input.as_slice(),
            |_, _, _, _| result,
        );
        assert_eq!(report.status, Status::Failed);
        assert_eq!(inventory(&f.root), before);
    }
    let f = Fixture::new(4);
    let input = serde_json::to_vec(&f.set("new")).unwrap();
    let mut newer = fs::read(&f.path).unwrap();
    newer.push(b' ');
    let report = run_with(
        Operation::Set,
        Some(f.path.clone()),
        &mut input.as_slice(),
        |store, path, base, target| {
            fs::write(path, &newer).unwrap();
            store.replace_config_document(path, base, target)
        },
    );
    assert_eq!(report.status, Status::Failed);
    assert_eq!(fs::read(&f.path).unwrap(), newer);
    let stale = json!({"contract_version":1,"expected_config_sha256":"0".repeat(64)});
    let report = run_with(
        Operation::Clear,
        Some(f.path.clone()),
        &mut serde_json::to_vec(&stale).unwrap().as_slice(),
        |_, _, _, _| panic!("stale dispatch"),
    );
    assert_eq!(report.status, Status::Conflict);
}

#[test]
#[cfg(unix)]
fn special_source_refuses_before_stdin_without_following_or_blocking() {
    let f = Fixture::new(4);
    let alias = f.root.join("alias.json");
    std::os::unix::fs::symlink(&f.path, &alias).unwrap();
    assert_eq!(
        run(Operation::Set, Some(alias), &mut Never).status,
        Status::Unavailable
    );
    let fifo = f.root.join("fifo.json");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    assert_eq!(
        run(Operation::Set, Some(fifo), &mut Never).status,
        Status::Unavailable
    );
}
