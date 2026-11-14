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
    assert!(
        matches!(report.data.as_ref(), Some(Data::Archive { inspection }) if !inspection.complete_media)
    );
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
    assert!(
        matches!(report.data, Some(Data::Archive { inspection }) if !inspection.configuration_included)
    );
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

fn isolated_package(partial: bool) -> (Fixture, PathBuf, String, serde_json::Value) {
    use std::collections::BTreeMap;
    use storage::{
        Coverage, Envelope, Kind, Manifest, Media, Namespace, ObjectRef, Omission, Scope,
    };
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let media = if partial {
        b"PRIVATE_OMITTED_CONTENT".as_slice()
    } else {
        b"".as_slice()
    };
    let media_sha = storage::digest(media);
    let record = Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: Default::default(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::json!({"text":"PRIVATE_CONVERSATION_CONTENT"}),
        media_descriptors: vec![Media {
            sha256: media_sha.clone(),
            bytes: media.len() as u64,
            media_type: "application/octet-stream".into(),
        }],
    };
    if partial {
        let bytes = serde_json::to_vec(&record).unwrap();
        let hash = storage::digest(&bytes);
        store
            .import(
                Manifest {
                    format: 1,
                    transaction_id: Uuid::new_v4(),
                    scope: Scope::LocalTransaction,
                    records: vec![ObjectRef {
                        sha256: hash.clone(),
                        bytes: bytes.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
                    media: vec![],
                    media_coverage: vec![Coverage::Omitted {
                        sha256: media_sha,
                        reason: Omission::SizeDeferred,
                    }],
                },
                BTreeMap::from([(hash, bytes)]),
            )
            .unwrap();
    } else {
        store
            .commit(vec![record], BTreeMap::from([(media_sha, media.to_vec())]))
            .unwrap();
    }
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::IsolatedBackup {
                binding_sha256: storage::digest(b"fixture binding"),
            },
        )
        .unwrap();
    let lease = store
        .sync_mode_lease(&store.sync_mode_snapshot().unwrap().token)
        .unwrap();
    let config = fixture.root.join("config.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"paths":fixture.paths(),
        "personalization":{"enabled":true,"text":"PRIVATE_PORTABLE_TEXT"},
        "model":"PRIVATE_MODEL", "model_credential":"PRIVATE_CREDENTIAL"}))
        .unwrap(),
    )
    .unwrap();
    let package = fixture.root.join("isolated-package");
    let mut marker = None;
    let inspection = store
        .export_isolated_package(&lease, &package, &config, &mut marker)
        .unwrap()
        .unwrap();
    let marker = marker.unwrap();
    assert_eq!(inspection.marker_sha256, marker);
    assert_eq!(inspection.data.complete_media, !partial);
    let expected = serde_json::to_value(inspection).unwrap();
    drop(lease);
    drop(store);
    (fixture, package, marker, expected)
}

fn command_json(
    operation: Operation,
    mut input: &[u8],
    missing_config: PathBuf,
) -> serde_json::Value {
    let report = run(operation, Path::new("-"), Some(missing_config), &mut input);
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert!(bytes.len() <= MAX_OUTPUT && bytes.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
    serde_json::from_slice(&bytes).unwrap()
}

// Opt in with READER_BUDDY_ISOLATED_FIXTURE_OUTPUT set to an ABSENT absolute
// canonical-temp child named reader-buddy-isolated-fixtures-<UUID>. Commit first,
// source the explicit Cargo environment, then reproduce with default features:
// cargo test --locked --offline --lib \
//   archive_maintenance::tests::emit_isolated_producer_fixtures \
//   -- --ignored --exact --nocapture
// Fresh UUIDs mean semantic reproduction, not byte-identical fixtures. Only the
// exporter-created closure and receipt survive; the source fixture owns its
// normal Store/cache/credential teardown. Never delete/reuse the output root.
#[test]
#[ignore = "explicit absent scratch output and clean committed source required"]
fn emit_isolated_producer_fixtures() -> Result<()> {
    use std::{collections::BTreeMap, io::Write, process::Command};

    fn git(args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture source identity unavailable"
        );
        output.stdout
    }
    fn source() -> serde_json::Value {
        assert!(
            git(&["status", "--porcelain", "--untracked-files=normal"]).is_empty(),
            "fixture source must be clean"
        );
        let commit = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        assert_eq!(commit.trim(), env!("VERGEN_GIT_SHA"));
        assert_eq!(option_env!("VERGEN_GIT_DIRTY"), Some("false"));
        let tree = String::from_utf8(git(&["rev-parse", "HEAD^{tree}"])).unwrap();
        let lock = git(&["show", "HEAD:Cargo.lock"]);
        let parsed: toml::Value = toml::from_str(std::str::from_utf8(&lock).unwrap()).unwrap();
        let sdk = parsed["package"]
            .as_array()
            .unwrap()
            .iter()
            .find(|package| package["name"].as_str() == Some("remarkable-open-sdk"))
            .unwrap();
        serde_json::json!({"commit":commit.trim(),"tree":tree.trim(),"clean":true,
            "compiled_commit":env!("VERGEN_GIT_SHA"),"git_derived_identity":env!("READER_BUDDY_VERSION"),
            "cargo_lock_sha256":storage::digest(&lock),"sdk_source":sdk["source"]})
    }
    fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    fn closure(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
        fn visit(
            root: &Path,
            directory: &Path,
            files: &mut BTreeMap<String, Vec<u8>>,
        ) -> Result<()> {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.is_dir() {
                    ensure!(
                        matches!(relative.as_str(), "data" | "data/objects"),
                        "unexpected fixture directory"
                    );
                    visit(root, &path, files)?;
                } else {
                    ensure!(metadata.is_file(), "nonregular fixture member");
                    ensure!(
                        matches!(
                            relative.as_str(),
                            "data/backup.json"
                                | "isolated-backup.json"
                                | "portable-configuration.json"
                        ) || relative
                            .strip_prefix("data/objects/")
                            .is_some_and(storage::valid_digest),
                        "unexpected fixture member"
                    );
                    ensure!(
                        files.len() < 16 && metadata.len() <= 64 * 1024,
                        "fixture exceeds small emission bound"
                    );
                    files.insert(relative, fs::read(path)?);
                }
            }
            Ok(())
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files)?;
        ensure!(
            files.values().map(Vec::len).sum::<usize>() <= 128 * 1024,
            "fixture exceeds aggregate bound"
        );
        Ok(files)
    }

    let output = PathBuf::from(
        std::env::var_os("READER_BUDDY_ISOLATED_FIXTURE_OUTPUT")
            .expect("explicit fixture output is required"),
    );
    let temporary = fs::canonicalize(std::env::temp_dir())?;
    ensure!(
        output.is_absolute() && output.parent().map(Path::as_os_str) == Some(temporary.as_os_str()),
        "output must be a direct canonical-temp child"
    );
    let suffix = output
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("reader-buddy-isolated-fixtures-"));
    ensure!(
        suffix.is_some_and(|suffix| Uuid::parse_str(suffix).is_ok()),
        "output requires the fixture prefix and UUID"
    );
    let features = [
        cfg!(feature = "drive-slot-qualification"),
        cfg!(feature = "development-input-diagnostics"),
        cfg!(feature = "development-resident-bootstrap"),
        cfg!(feature = "development-native-close"),
        cfg!(feature = "installed-resident-output"),
    ];
    ensure!(
        !features.into_iter().any(|enabled| enabled),
        "emitter requires default features"
    );
    let source_before = source();
    let settings: BTreeMap<_, _> = [
        "RUSTUP_TOOLCHAIN",
        "CARGO_BUILD_JOBS",
        "CARGO_INCREMENTAL",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
    ]
    .into_iter()
    .map(|key| {
        (
            key,
            std::env::var(key).expect("explicit Cargo setting required"),
        )
    })
    .collect();
    let compiler = Command::new("rustc").arg("-Vv").output()?;
    ensure!(
        compiler.status.success(),
        "reproduction compiler unavailable"
    );
    let mut directory = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(&output)?; // Existing paths are refused; no overwrite/cleanup.
    ensure!(
        fs::canonicalize(&output)?.as_os_str() == output.as_os_str(),
        "output identity changed"
    );
    let mut cases = BTreeMap::new();
    for (name, partial) in [("complete", false), ("media-omitted", true)] {
        let (fixture, package, marker, expected) = isolated_package(partial);
        let before = fixture.inventory();
        let original = closure(&package)?;
        let destination = output.join(name);
        fs::create_dir(&destination)?;
        fs::create_dir(destination.join("data"))?;
        fs::create_dir(destination.join("data/objects"))?;
        // Copy the actual producer marker last without reserializing any bytes.
        for (path, bytes) in original
            .iter()
            .filter(|(path, _)| path.as_str() != "isolated-backup.json")
        {
            write_new(&destination.join(path), bytes)?;
        }
        write_new(
            &destination.join("isolated-backup.json"),
            &original["isolated-backup.json"],
        )?;
        let report = command_json(
            Operation::InspectIsolated,
            &serde_json::to_vec(&serde_json::json!({"path":destination,"marker_sha256":marker}))?,
            output.join("absent-config"),
        );
        assert_eq!(
            report,
            serde_json::json!({"report_version":1,"operation":"inspect-isolated","status":"ok",
            "incomplete":partial,"activation_performed":false,"data":{"isolated_package":expected}})
        );
        assert_eq!(closure(&destination)?, original);
        assert_eq!(closure(&package)?, original);
        assert_eq!(fixture.inventory(), before);
        let inventory: BTreeMap<_, _> = original
            .iter()
            .map(|(path, bytes)| {
                (
                    path,
                    serde_json::json!({"bytes":bytes.len(),"sha256":storage::digest(bytes)}),
                )
            })
            .collect();
        cases.insert(
            name,
            serde_json::json!({"directory":name,"marker_sha256":marker,
            "files":inventory,"expected_report":report}),
        );
    }
    assert_eq!(
        source(),
        source_before,
        "source changed during fixture emission"
    );
    let receipt = serde_json::json!({"fixture_receipt_version":1,"synthetic_test_content_only":true,
        "producer":"Store::export_isolated_package with the original sync-mode lease via isolated_package(false/true)",
        "source":source_before,"cases":cases,"reproduction":{"cargo_args":["test","--locked","--offline","--lib",
        "archive_maintenance::tests::emit_isolated_producer_fixtures","--","--ignored","--exact","--nocapture"],
        "required_output_env":"READER_BUDDY_ISOLATED_FIXTURE_OUTPUT","output_policy":"absent canonical-temp child reader-buddy-isolated-fixtures-<UUID>; never reuse/delete",
        "environment":settings,"runtime_rustc_verbose":String::from_utf8(compiler.stdout)?,
        "profile":"test","debug_assertions":cfg!(debug_assertions),"crate_features":"default",
        "uuid_reproduction":"fresh synthetic UUIDs; semantics reproducible, bytes differ",
        "evidence_limits":"actual synthetic isolated-producer package and in-process archive handler only; test-only SDK features may unify; no retained normal release, native platform, provider publication or restore proof"}});
    let mut bytes = serde_json::to_vec_pretty(&receipt)?;
    bytes.push(b'\n');
    ensure!(bytes.len() <= 32 * 1024, "fixture receipt too large");
    write_new(&output.join("receipt.json"), &bytes)?;
    println!(
        "emitted actual synthetic isolated fixtures; receipt_sha256={}",
        storage::digest(&bytes)
    );
    Ok(())
}

#[test]
fn isolated_command_inspects_actual_export_with_exact_redacted_output_and_zero_byte_object() {
    let (fixture, package, marker, expected) = isolated_package(false);
    let before = fixture.inventory();
    assert_eq!(
        fs::metadata(package.join("data/objects").join(storage::digest(b"")))
            .unwrap()
            .len(),
        0
    );
    let input =
        serde_json::to_vec(&serde_json::json!({"path":package,"marker_sha256":marker})).unwrap();
    let result = command_json(
        Operation::InspectIsolated,
        &input,
        fixture.root.join("missing-config"),
    );
    assert_eq!(
        result,
        serde_json::json!({"report_version":1,"operation":"inspect-isolated","status":"ok",
        "incomplete":false,"activation_performed":false,"data":{"isolated_package":expected}})
    );
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains(fixture.root.to_str().unwrap()));
    assert_eq!(fixture.inventory(), before);
}

#[test]
fn isolated_command_preserves_declared_media_omission_without_claiming_completeness() {
    let (fixture, package, marker, expected) = isolated_package(true);
    let result = command_json(
        Operation::InspectIsolated,
        &serde_json::to_vec(&serde_json::json!({"path":package,"marker_sha256":marker})).unwrap(),
        fixture.root.join("missing-config"),
    );
    assert_eq!(result["status"], "ok");
    assert_eq!(result["incomplete"], true);
    assert_eq!(
        result["data"],
        serde_json::json!({"isolated_package":expected})
    );
}

#[test]
fn isolated_command_refuses_corruption_extra_members_and_wrong_original_marker() {
    let (fixture, package, marker, _) = isolated_package(false);
    let check = |marker: &str| {
        command_json(
            Operation::InspectIsolated,
            &serde_json::to_vec(&serde_json::json!({"path":package,"marker_sha256":marker}))
                .unwrap(),
            fixture.root.join("missing-config"),
        )
    };
    let before = fixture.inventory();
    let refused = |result: serde_json::Value| {
        assert_eq!(result["status"], "unsupported");
        assert_eq!(result["data"], serde_json::Value::Null);
        assert_eq!(result["activation_performed"], false);
    };
    refused(check(&storage::digest(b"wrong original marker")));
    for path in [
        package.join("PRIVATE_EXTRA"),
        package.join("data/PRIVATE_EXTRA"),
        package.join("data/objects/PRIVATE_EXTRA"),
    ] {
        fs::write(&path, b"PRIVATE_EXTRA").unwrap();
        refused(check(&marker));
        assert_eq!(fs::read(&path).unwrap(), b"PRIVATE_EXTRA");
        fs::remove_file(path).unwrap();
    }
    let content = package.join("data/objects").join(storage::digest(b""));
    fs::write(&content, b"PRIVATE_CORRUPT_OBJECT").unwrap();
    refused(check(&marker));
    assert_eq!(fs::read(&content).unwrap(), b"PRIVATE_CORRUPT_OBJECT");
    fs::write(&content, b"").unwrap();
    let marker_path = package.join("isolated-backup.json");
    let original = fs::read(&marker_path).unwrap();
    let mut unsupported: serde_json::Value = serde_json::from_slice(&original).unwrap();
    unsupported["isolated_backup_version"] = serde_json::json!(2);
    let unsupported = serde_json::to_vec(&unsupported).unwrap();
    fs::write(&marker_path, &unsupported).unwrap();
    refused(check(&storage::digest(&unsupported)));
    fs::write(&marker_path, &original).unwrap();
    assert_eq!(check(&marker)["status"], "ok");
    assert_eq!(fixture.inventory(), before);
}

#[test]
fn isolated_command_enforces_closed_bounded_input_and_old_operation_shapes() {
    use clap::ValueEnum;
    let (fixture, package, marker, _) = isolated_package(false);
    let config = fixture.root.join("missing-config");
    let json = |input: serde_json::Value| {
        command_json(
            Operation::InspectIsolated,
            &serde_json::to_vec(&input).unwrap(),
            config.clone(),
        )
    };
    for input in [
        serde_json::json!(null),
        serde_json::json!({"path":package}),
        serde_json::json!({"marker_sha256":marker}),
        serde_json::json!({"path":package,"marker_sha256":"NOT_A_DIGEST"}),
        serde_json::json!({"path":"relative","marker_sha256":marker}),
        serde_json::json!({"path":package,"marker_sha256":marker,"grant":"PRIVATE_GRANT"}),
    ] {
        assert_eq!(json(input)["status"], "invalid-input");
    }
    let path = serde_json::to_string(&package).unwrap();
    let duplicate = format!(
        "{{\"path\":{path},\"marker_sha256\":\"{marker}\",\"marker_sha256\":\"{marker}\"}}"
    );
    assert_eq!(
        command_json(
            Operation::InspectIsolated,
            duplicate.as_bytes(),
            config.clone()
        )["status"],
        "invalid-input"
    );
    for raw in [
        vec![b' '; MAX_INPUT + 1],
        vec![0xff],
        b"\xef\xbb\xbf{}".to_vec(),
    ] {
        assert_eq!(
            command_json(Operation::InspectIsolated, &raw, config.clone())["status"],
            "invalid-input"
        );
    }
    let input = fixture.root.join("oversized-input.json");
    fs::write(&input, vec![b' '; MAX_INPUT + 1]).unwrap();
    assert_eq!(
        run(
            Operation::InspectIsolated,
            &input,
            Some(config.clone()),
            &mut std::io::empty()
        )
        .status,
        Status::InvalidInput
    );
    assert_eq!(
        Operation::from_str("inspect-isolated", false).unwrap(),
        Operation::InspectIsolated
    );
    for (operation, name) in [
        (Operation::Inspect, "inspect"),
        (Operation::ExportSelected, "export-selected"),
        (Operation::InspectNotebook, "inspect-notebook"),
        (Operation::ExportNotebook, "export-notebook"),
    ] {
        assert_eq!(Operation::from_str(name, false).unwrap(), operation);
        assert_eq!(serde_json::to_value(operation).unwrap(), name);
        assert_eq!(
            run(
                operation,
                Path::new("-"),
                Some(config.clone()),
                &mut &*serde_json::to_vec(
                    &serde_json::json!({"path":package,"marker_sha256":marker})
                )
                .unwrap()
            )
            .status,
            Status::InvalidInput
        );
    }
    let legacy = command_json(
        Operation::Inspect,
        &serde_json::to_vec(&serde_json::json!({"path":package.join("data")})).unwrap(),
        config,
    );
    assert_eq!(legacy["status"], "ok");
    assert_eq!(
        legacy["data"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["inspection"]
    );
}

#[test]
fn isolated_command_refuses_nonregular_or_absent_marker_without_creating_state() {
    let fixture = Fixture::new();
    let package = fixture.root.join("not-a-package");
    fs::create_dir(&package).unwrap();
    let bytes = serde_json::to_vec(
        &serde_json::json!({"path":package,"marker_sha256":storage::digest(b"marker")}),
    )
    .unwrap();
    let check = || {
        command_json(
            Operation::InspectIsolated,
            &bytes,
            fixture.root.join("missing-config"),
        )
    };
    assert_eq!(check()["status"], "unavailable");
    fs::create_dir(package.join("isolated-backup.json")).unwrap();
    assert_eq!(check()["status"], "unsupported");
    assert!(fixture.inventory().is_empty());
}
