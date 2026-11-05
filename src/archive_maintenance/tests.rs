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

#[test]
fn notebook_archive_report_keeps_lease_through_prefix_and_flush_errors() {
    struct Output {
        paths: crate::storage::StorePaths,
        prefix: bool,
        writes: usize,
        flushes: usize,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            assert!(matches!(
                ExportSource::open(self.paths.clone()),
                Err(Refusal::Busy)
            ));
            self.writes += 1;
            if self.prefix && self.writes == 2 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            Ok(if self.prefix { 7 } else { bytes.len() })
        }
        fn flush(&mut self) -> std::io::Result<()> {
            assert!(matches!(
                ExportSource::open(self.paths.clone()),
                Err(Refusal::Busy)
            ));
            self.flushes += 1;
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    let f = Fixture::new();
    drop(Store::prepare_notebook(f.paths()).unwrap());
    for prefix in [false, true] {
        let path = f.root.join(if prefix { "prefix" } else { "flush" });
        let report = export_notebook(
            Config {
                paths: f.paths(),
                ..Config::default()
            },
            &path,
        );
        assert_eq!(report.status, Status::Ok);
        let mut output = Output {
            paths: f.paths(),
            prefix,
            writes: 0,
            flushes: 0,
        };
        assert!(report.write_to(&mut output).is_err());
        assert_eq!(
            (output.writes, output.flushes),
            if prefix { (2, 0) } else { (1, 1) }
        );
        assert!(ExportSource::open(f.paths()).is_ok());
        assert_eq!(inspect_notebook(&path).status, Status::Ok);
    }
}

#[test]
fn notebook_archive_detached_witness_refuses_change_during_flush() {
    struct Output {
        marker: PathBuf,
        frames: Vec<u8>,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.frames.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            fs::write(&self.marker, b"{")
        }
    }
    let f = Fixture::new();
    drop(Store::prepare_notebook(f.paths()).unwrap());
    let path = f.root.join("archive");
    let mut export_frame = Vec::new();
    export_notebook(
        Config {
            paths: f.paths(),
            ..Config::default()
        },
        &path,
    )
    .write_to(&mut export_frame)
    .unwrap();
    let report = inspect_notebook(&path);
    assert_eq!(report.status, Status::Ok);
    let mut output = Output {
        marker: path.join("backup.json"),
        frames: Vec::new(),
    };
    assert!(report.write_to(&mut output).is_err());
    assert_eq!(
        output.frames.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    assert_eq!(inspect_notebook(&path).status, Status::Unsupported);
}

#[test]
fn notebook_archive_no_clobber_has_one_winner_and_refuses_existing_marker() {
    let a = Fixture::new();
    let b = Fixture::new();
    for fixture in [&a, &b] {
        drop(Store::prepare_notebook(fixture.paths()).unwrap());
    }
    let destination = a.root.join("archive");
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let launch = |paths| {
            let destination = &destination;
            let barrier = &barrier;
            scope.spawn(move || {
                let mut source = ExportSource::open(paths).unwrap();
                let prepared = source.prepare().unwrap();
                barrier.wait();
                source.export(prepared, destination).is_ok()
            })
        };
        let first = launch(a.paths());
        let second = launch(b.paths());
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_ne!(results[0], results[1]);
    let marker = destination.join("backup.json");
    let original = fs::read(&marker).unwrap();
    assert!(files::publish_archive_marker(&destination, b"collision").is_err());
    assert_eq!(fs::read(marker).unwrap(), original);
    assert_eq!(inspect_notebook(&destination).status, Status::Ok);
}

#[test]
#[cfg(unix)]
fn notebook_archive_marker_fault_preserves_unpublished_and_possibly_published_roots() {
    let f = Fixture::new();
    drop(Store::prepare_notebook(f.paths()).unwrap());
    for (name, fault, status) in [
        (
            "before",
            files::AtomicFailure::BeforeRename,
            Status::Unavailable,
        ),
        (
            "after",
            files::AtomicFailure::RenameAcknowledgment,
            Status::Ok,
        ),
    ] {
        let path = f.root.join(name);
        files::fail_next_atomic(path.join("backup.json"), fault);
        let report = export_notebook(
            Config {
                paths: f.paths(),
                ..Config::default()
            },
            &path,
        );
        assert_eq!(report.status, Status::Failed);
        drop(report);
        assert!(path.join("objects").is_dir());
        assert_eq!(inspect_notebook(&path).status, status);
    }
}
