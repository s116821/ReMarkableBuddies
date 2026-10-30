use super::*;
use mode::SyncModeChoice;
use serde_json::{json, Value};
use std::fs;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("mode-maintenance-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> storage::StorePaths {
        storage::StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn store(&self) -> Arc<Store> {
        Arc::new(Store::open(self.paths()).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn choice(index: usize) -> SyncModeChoice {
    match index {
        0 => SyncModeChoice::LocalOnly,
        1 => SyncModeChoice::IsolatedBackup {
            binding_sha256: storage::digest(b"fixture"),
        },
        _ => SyncModeChoice::SharedGroup {
            group: Uuid::new_v4(),
            binding_sha256: storage::digest(b"fixture"),
            root_id: "fixture-root".into(),
        },
    }
}
fn set(store: &Store, choice: SyncModeChoice) -> SyncModeReceipt {
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            choice,
        )
        .unwrap()
}
fn report_value(report: Report) -> Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn validated(value: &Value) -> Validated {
    serde_json::from_value::<Request>(value.clone())
        .unwrap()
        .validate()
        .unwrap()
}

#[test]
fn output_holds_current_mode_and_whole_store_through_success_or_prefix_failure() {
    struct Writer {
        store: Arc<Store>,
        calls: usize,
        bytes: Vec<u8>,
        fail: bool,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            assert!(self
                .store
                .set_sync_mode(
                    &self.store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    SyncModeChoice::LocalOnly
                )
                .is_err());
            assert!(matches!(
                Store::open_existing_for_mode(self.store.paths.clone()),
                Err(Refusal::Busy)
            ));
            if self.fail && self.calls == 2 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            let count = if self.fail {
                7.min(bytes.len())
            } else {
                bytes.len()
            };
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for index in 0..4 {
        let fixture = Fixture::new();
        let store = fixture.store();
        if index > 0 {
            set(&store, choice(index - 1));
        }
        let report = finish(Operation::Inspect, Status::Ok, store.clone(), None);
        let expected = report.encoded().unwrap();
        let mut writer = Writer {
            store: store.clone(),
            calls: 0,
            bytes: vec![],
            fail: index == 3,
        };
        let result = report.write_to(&mut writer);
        if writer.fail {
            assert!(result.is_err());
            assert_eq!(writer.calls, 2);
            assert_eq!(writer.bytes, expected[..7]);
        } else {
            result.unwrap();
            assert_eq!(writer.bytes, expected);
        }
        // Report was consumed: no mode guard remains after either result.
        set(&store, SyncModeChoice::LocalOnly);
        drop(writer);
        drop(store);
        Store::open_existing_for_mode(fixture.paths()).unwrap();
    }
}

#[test]
fn original_uncertain_request_replays_history_without_fresh_precondition_or_second_commit() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let request = json!({"operation":Uuid::new_v4(),"expected":store.sync_mode_snapshot().unwrap().token,"choice":choice(1)});
    store.set_fault(storage::Fault::AfterCommit).unwrap();
    let failed = report_value(choose(store.clone(), validated(&request)));
    assert_eq!(failed["status"], "failed");
    assert!(failed["data"]["receipt"].is_null());
    let path = fixture.paths().data.join("sync/mode.json");
    let first_bytes = fs::read(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&first_bytes).unwrap()["transitions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let recovered = report_value(choose(store.clone(), validated(&request)));
    assert_eq!(recovered["status"], "ok");
    assert_eq!(recovered["data"]["receipt_status"], "current");
    assert_eq!(fs::read(&path).unwrap(), first_bytes);
    let old_receipt = recovered["data"]["receipt"].clone();
    let later = set(&store, SyncModeChoice::LocalOnly);
    let later_bytes = fs::read(&path).unwrap();
    let old = report_value(choose(store.clone(), validated(&request)));
    assert_eq!(old["data"]["receipt"], old_receipt);
    assert_eq!(old["data"]["receipt_status"], "historical");
    assert_eq!(old["data"]["current_snapshot"]["epoch"], json!(later.epoch));
    assert_eq!(
        old["data"]["current_snapshot"]["mode"]["kind"],
        "local-only"
    );
    assert_eq!(fs::read(&path).unwrap(), later_bytes);
    let mut changed = request.clone();
    changed["choice"] = json!(choice(0));
    assert_eq!(
        report_value(choose(store.clone(), validated(&changed)))["status"],
        "conflict"
    );
    assert_eq!(fs::read(&path).unwrap(), later_bytes);
}

#[test]
fn ordinary_runtime_owns_both_guards_and_refuses_explicit_or_selected_modes() {
    for index in 0..4 {
        let fixture = Fixture::new();
        let store = fixture.store();
        if index > 0 {
            set(&store, choice(index - 1));
        }
        if index >= 2 {
            assert!(admit_ordinary(&store, false).is_err());
            continue;
        }
        let guard = admit_ordinary(&store, false).unwrap();
        assert!(store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                choice(1)
            )
            .is_err());
        let memory = crate::memory::selected::SelectedMemory::new(
            store.clone(),
            crate::memory::catalog::Identity {
                group: Uuid::new_v4(),
                profile: Uuid::new_v4(),
                binding_sha256: storage::digest(b"fixture"),
            },
        )
        .unwrap();
        assert!(memory
            .initialize_selection(crate::memory::selected::InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"fixture"),
                registrations: vec![],
            })
            .is_err());
        drop(guard);
        if index == 0 {
            drop(admit_ordinary(&store, true).unwrap());
        }
        if index == 1 {
            assert!(admit_ordinary(&store, true).is_err());
        }
        // No leaked lease after rejection or guard cleanup.
        set(&store, choice(1));
    }
}

#[test]
fn a_live_legacy_publisher_is_reported_busy_without_stopping_or_rebasing_it() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let request = json!({"operation":Uuid::new_v4(),"expected":store.sync_mode_snapshot().unwrap().token,"choice":choice(0)});
    let publisher = store.legacy_publisher_lease().unwrap();
    assert_eq!(
        report_value(choose(store.clone(), validated(&request)))["status"],
        "busy"
    );
    assert!(store.sync_mode_snapshot().unwrap().mode.is_none());
    assert!(!fixture.paths().data.join("sync/mode.json").exists());
    drop(publisher);
    assert_eq!(
        report_value(choose(store, validated(&request)))["status"],
        "ok"
    );
}

#[test]
fn selected_aware_opener_preserves_legacy_refusal_and_ordinary_startup_cannot_fallback() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let identity = crate::memory::catalog::Identity {
        group: Uuid::new_v4(),
        profile: Uuid::new_v4(),
        binding_sha256: storage::digest(b"fixture"),
    };
    let memory =
        crate::memory::selected::SelectedMemory::new(store.clone(), identity.clone()).unwrap();
    memory
        .initialize_selection(crate::memory::selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"fixture"),
            registrations: vec![],
        })
        .unwrap();
    let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    assert!(admit_ordinary(&store, false).is_err());
    drop(memory);
    drop(store);
    assert!(matches!(
        Store::open_existing(fixture.paths()),
        Err(Refusal::Unsupported)
    ));
    let opened = Arc::new(Store::open_existing_for_mode(fixture.paths()).unwrap());
    assert!(admit_ordinary(&opened, false).is_err());
    let value = report_value(finish(Operation::Inspect, Status::Ok, opened, None));
    assert_eq!(value["status"], "ok");
    assert_eq!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        current
    );
}

#[test]
fn unsupported_input_never_reads_and_only_exact_trusted_errors_are_classified() {
    struct MustNotRead;
    impl Read for MustNotRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("unexpected input read")
        }
    }
    for operation in [Operation::Initialize, Operation::Inspect] {
        assert_eq!(
            run(operation, Some(Path::new("-")), None, &mut MustNotRead).status,
            Status::InvalidInput
        );
    }
    assert!(SyncModeToken::from_maintenance_request(Uuid::nil(), storage::digest(b"x")).is_err());
    assert!(SyncModeToken::from_maintenance_request(Uuid::new_v4(), "bad".into()).is_err());
    assert_eq!(
        mutation_status(&anyhow::anyhow!("stale mode token")),
        Status::Conflict
    );
    assert_eq!(
        mutation_status(&anyhow::anyhow!("private prefix stale mode token")),
        Status::Failed
    );
}
