use super::*;
use crate as buddy;
use crate::storage::{self, sync::mode::SyncModeChoice, Fault, Uuid};
use std::fs;
#[path = "../../tests/support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;

#[test]
fn real_export_holds_mode_and_process_until_single_write_finishes_or_fails() {
    struct Writer {
        store: Arc<Store>,
        prefix: bool,
        calls: usize,
        bytes: Vec<u8>,
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
                Store::open_existing_for_archive(self.store.paths.clone()),
                Err(Refusal::Busy)
            ));
            if self.prefix && self.calls > 1 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            let count = if self.prefix {
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
    for choice in [
        None,
        Some(SyncModeChoice::LocalOnly),
        Some(SyncModeChoice::IsolatedBackup {
            binding_sha256: storage::digest(b"binding"),
        }),
        Some(SyncModeChoice::SharedGroup {
            group: Uuid::new_v4(),
            binding_sha256: storage::digest(b"binding"),
            root_id: "fixture-root".into(),
        }),
    ] {
        for prefix in [false, true] {
            let f = Fixture::new();
            let (store, _) = f.selected(false);
            if let Some(choice) = choice.clone() {
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        choice,
                    )
                    .unwrap();
            }
            let before = f.inventory();
            let destination = f.root.join("archive");
            let report = export(store.clone(), &destination);
            assert_eq!(report.status, Status::Ok);
            let mut writer = Writer {
                store: store.clone(),
                prefix,
                calls: 0,
                bytes: vec![],
            };
            assert_eq!(report.write_to(&mut writer).is_err(), prefix);
            if prefix {
                assert_eq!(writer.bytes.len(), 7);
                assert_eq!(writer.calls, 2)
            } else {
                assert!(writer.bytes.ends_with(b"\n"));
            }
            assert_eq!(f.inventory(), before);
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    SyncModeChoice::LocalOnly,
                )
                .unwrap();
            drop(writer);
            drop(store);
            assert!(Store::open_existing_for_archive(f.paths()).is_ok());
            assert!(Store::open_existing(f.paths()).is_err());
        }
    }
}

#[test]
fn before_and_after_marker_faults_preserve_source_and_require_deliberate_inspection() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::AfterCommit,
    ] {
        let f = Fixture::new();
        let (store, scope) = f.selected(false);
        let before = f.inventory();
        let token = store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token;
        let destination = f.root.join("archive");
        store.set_fault(fault).unwrap();
        let report = export(store.clone(), &destination);
        assert_eq!(report.status, Status::Failed);
        assert!(report.incomplete);
        let mut output = vec![];
        report.write_to(&mut output).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap()["data"],
            serde_json::Value::Null
        );
        assert_eq!(
            destination.join("backup.json").exists(),
            fault == Fault::AfterCommit
        );
        assert_eq!(
            inspect(&destination).status,
            if fault == Fault::AfterCommit {
                Status::Ok
            } else {
                Status::Unavailable
            }
        );
        assert_eq!(f.inventory(), before);
        assert_eq!(
            store
                .selected_snapshot(&scope, 4096)
                .unwrap()
                .unwrap()
                .token,
            token
        );
        // Existing partial/complete destinations are preserved; no implicit replay.
        if destination.exists() {
            let before = fs::read_dir(&destination).unwrap().count();
            assert_eq!(export(store.clone(), &destination).status, Status::Failed);
            assert_eq!(fs::read_dir(&destination).unwrap().count(), before);
        }
    }
}

#[test]
fn partial_media_is_truthful_and_complete_archive_inspection_requires_no_config_or_store() {
    let f = Fixture::new();
    let (store, _) = f.selected(true);
    let destination = f.root.join("archive");
    let report = export(store.clone(), &destination);
    assert_eq!(report.status, Status::Ok);
    assert!(report.incomplete);
    assert!(!report.data.as_ref().unwrap().inspection.complete_media);
    drop(report);
    drop(store);
    let bytes = serde_json::to_vec(&serde_json::json!({"path":destination})).unwrap();
    let report = run(
        Operation::Inspect,
        Path::new("-"),
        Some(f.root.join("absent-config")),
        &mut bytes.as_slice(),
    );
    assert_eq!(report.status, Status::Ok);
    assert!(report.incomplete);
    assert!(!report.data.unwrap().inspection.configuration_included);
}
