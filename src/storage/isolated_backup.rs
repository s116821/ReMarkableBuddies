//! Local ordinary isolated backup package with an explicit completion marker.
use super::*;
use crate::config::{self, portable};

const MARKER: &str = "isolated-backup.json";
const DATA: &str = "data";
const BACKUP: &str = "backup.json";
const OBJECTS: &str = "objects";
const PREFERENCES: &str = "portable-configuration.json";
const MAX_MARKER: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    isolated_backup_version: u32,
    data_backup: ObjectRef,
    portable_configuration: ObjectRef,
}

/// Redacted inspection of a complete package. Hashes refer to raw file bytes.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct PackageInspection {
    pub marker_sha256: String,
    pub data_backup: ObjectRef,
    pub portable_configuration: ObjectRef,
    pub declared_settings: usize,
    pub data: BackupInspection,
}

fn reference(bytes: &[u8]) -> ObjectRef {
    ObjectRef {
        bytes: bytes.len() as u64,
        sha256: digest(bytes),
    }
}

fn read_component(path: &Path, expected: &ObjectRef, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        expected.bytes != 0 && expected.bytes <= limit && valid_digest(&expected.sha256),
        "invalid isolated backup component reference"
    );
    let bytes = files::read(path, expected.bytes)?;
    ensure!(
        reference(&bytes) == *expected,
        "isolated backup component changed"
    );
    Ok(bytes)
}

fn members(
    root: &Path,
    expected: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> Result<()> {
    files::safe_path(root)?;
    ensure!(
        fs::symlink_metadata(root)?.is_dir(),
        "expected isolated backup directory"
    );
    let mut remaining: BTreeSet<_> = expected
        .into_iter()
        .map(|name| name.as_ref().to_owned())
        .collect();
    for entry in fs::read_dir(root)? {
        ensure!(
            remaining.remove(&entry?.file_name()),
            "unexpected isolated backup entry"
        );
    }
    ensure!(remaining.is_empty(), "missing isolated backup entry");
    Ok(())
}

fn inspect_components(root: &Path, marker_bytes: &[u8]) -> Result<VerifiedPackage> {
    let value = config::inspection::strict_json(marker_bytes)
        .map_err(|_| anyhow::anyhow!("invalid isolated backup marker"))?;
    let marker: Marker = serde_json::from_value(value)
        .map_err(|_| anyhow::anyhow!("invalid isolated backup marker"))?;
    ensure!(
        marker.isolated_backup_version == 1,
        "unsupported isolated backup version"
    );
    let data = root.join(DATA);
    members(&data, [BACKUP, OBJECTS])?;
    let bytes = read_component(&data.join(BACKUP), &marker.data_backup, MAX_METADATA as u64)?;
    let (backup, inspection) = ordinary_restore::parse_archive(&data, &bytes)?;
    ensure!(
        backup.config_snapshot.is_none() && !backup.manifests.is_empty(),
        "isolated package requires data-only nonempty history"
    );
    members(
        &data.join(OBJECTS),
        backup.objects.iter().map(|item| &item.sha256),
    )?;
    let bytes = read_component(
        &root.join(PREFERENCES),
        &marker.portable_configuration,
        portable::MAX_IO as u64,
    )?;
    let artifact = portable::selected::Artifact::parse(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid isolated portable configuration"))?;
    let inspection = PackageInspection {
        marker_sha256: digest(marker_bytes),
        data_backup: marker.data_backup,
        portable_configuration: marker.portable_configuration,
        declared_settings: artifact.value()["settings"]
            .as_object()
            .context("invalid isolated portable settings")?
            .len(),
        data: inspection,
    };
    Ok(VerifiedPackage {
        inspection,
        artifact,
    })
}

struct VerifiedPackage {
    inspection: PackageInspection,
    // The verified handoff retains the exact parsed value from the hashed read.
    artifact: portable::selected::Artifact,
}

fn canonical_store_matches(actual: &StorePaths, declared: &StorePaths) -> Result<()> {
    declared.validate()?;
    for (actual, declared) in [
        (&actual.data, &declared.data),
        (&actual.cache, &declared.cache),
        (&actual.credentials, &declared.credentials),
    ] {
        ensure!(
            canonical_archive_path(actual)? == canonical_archive_path(declared)?,
            "saved configuration belongs to a different Store"
        );
    }
    Ok(())
}

fn capture_saved(
    store: &Store,
    path: &Path,
    destination: &Path,
) -> Result<(PathBuf, String, Vec<u8>)> {
    let canonical = canonical_archive_path(path)?;
    require_archive_disjoint(
        &canonical,
        [
            &store.paths.data,
            &store.paths.cache,
            &store.paths.credentials,
        ]
        .map(PathBuf::as_path),
    )?;
    require_archive_disjoint(&canonical_archive_path(destination)?, [path])?;
    let bytes = files::read(path, MAX_RECORD as u64)?;
    let (config, artifact) = portable::project_saved(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid saved backup configuration"))?;
    canonical_store_matches(&store.paths, &config.paths)?;
    let artifact_bytes = serde_json::to_vec(&artifact)?;
    portable::selected::Artifact::parse(&artifact_bytes)
        .map_err(|_| anyhow::anyhow!("invalid projected portable configuration"))?;
    Ok((canonical, digest(&bytes), artifact_bytes))
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "worker and Manager integration are separate")
)]
impl Store {
    /// Inspect the explicit package root. Ordinary archive readers still accept
    /// the data child by its own path. To reconcile a possible publication,
    /// supply its original expected marker digest.
    pub(crate) fn inspect_isolated_package(
        root: &Path,
        expected_marker_sha256: Option<&str>,
    ) -> Result<PackageInspection> {
        Self::verified_isolated_package(root, expected_marker_sha256)
            .map(|package| package.inspection)
    }

    /// Extract only the typed portable value from a complete, exact package.
    /// Import is a separate destination operation.
    pub(crate) fn isolated_package_preferences(
        root: &Path,
        expected_marker_sha256: &str,
    ) -> Result<portable::selected::Artifact> {
        Self::verified_isolated_package(root, Some(expected_marker_sha256))
            .map(|package| package.artifact)
    }

    fn verified_isolated_package(
        root: &Path,
        expected_marker_sha256: Option<&str>,
    ) -> Result<VerifiedPackage> {
        members(root, [DATA, PREFERENCES, MARKER])?;
        let marker = files::read(&root.join(MARKER), MAX_MARKER)?;
        ensure!(
            expected_marker_sha256.is_none_or(|expected| digest(&marker) == expected),
            "isolated package marker differs from original"
        );
        inspect_components(root, &marker)
    }

    /// Prepare a fresh package under the original isolated lease. An empty
    /// Store returns no work before accessing the saved configuration.
    /// The optional caller-owned marker digest is assigned before attempting
    /// the final publish and can be used for exact read-only reconciliation.
    pub(crate) fn export_isolated_package(
        &self,
        lease: &sync::mode::SyncModeLease,
        destination: &Path,
        saved_config: &Path,
        attempted_marker: &mut Option<String>,
    ) -> Result<Option<PackageInspection>> {
        self.export_isolated_package_with(
            lease,
            destination,
            saved_config,
            attempted_marker,
            |_| Ok(()),
        )
    }

    fn export_isolated_package_with(
        &self,
        lease: &sync::mode::SyncModeLease,
        destination: &Path,
        saved_config: &Path,
        attempted_marker: &mut Option<String>,
        before_marker: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<Option<PackageInspection>> {
        ensure!(
            attempted_marker.is_none(),
            "inspect the original package attempt first"
        );
        self.with_isolated_snapshot(lease, destination, |inner, backup, bytes| {
            let (saved_path, original, artifact) = capture_saved(self, saved_config, destination)?;
            reserve_archive_root(destination)?;
            let data = destination.join(DATA);
            files::directory(&data.join(OBJECTS))?;
            self.export_write_locked(inner, &data, backup, bytes, &mut false, files::atomic)?;
            files::atomic(&destination.join(PREFERENCES), &artifact)?;
            let marker = Marker {
                isolated_backup_version: 1,
                data_backup: reference(bytes),
                portable_configuration: reference(&artifact),
            };
            let marker_bytes = serde_json::to_vec(&marker)?;
            let inspection = inspect_components(destination, &marker_bytes)?.inspection;
            members(destination, [DATA, PREFERENCES])?;
            // Once publication is attempted, preserve its identity even if
            // staging or rename acknowledgment fails.
            *attempted_marker = Some(inspection.marker_sha256.clone());
            let mut config_refused = false;
            let result = files::atomic_json_guarded(&destination.join(MARKER), &marker, || {
                let check = (|| -> Result<()> {
                    before_marker(destination)?;
                    ensure!(
                        canonical_archive_path(saved_config)? == saved_path
                            && digest(&files::read(saved_config, MAX_RECORD as u64)?) == original,
                        "saved backup configuration changed"
                    );
                    Ok(())
                })();
                config_refused = check.is_err();
                check
            });
            if config_refused {
                *attempted_marker = None;
            }
            result?;
            let observed =
                Self::inspect_isolated_package(destination, attempted_marker.as_deref())?;
            ensure!(observed == inspection, "isolated backup readback changed");
            Ok(observed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::portable::import,
        storage::sync::mode::{SyncModeChoice, SyncModeLease},
    };
    use ordinary_restore::tests::{record, Fixture};
    use serde_json::{json, Value};
    use std::sync::Arc;

    const PNG: &[u8] = include_bytes!("../../tests/fixtures/trigger-dismiss/closed.png");

    fn lease(store: &Arc<Store>) -> SyncModeLease {
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::IsolatedBackup {
                    binding_sha256: digest(b"private binding"),
                },
            )
            .unwrap();
        store
            .sync_mode_lease(&store.sync_mode_snapshot().unwrap().token)
            .unwrap()
    }

    fn saved(fixture: &Fixture, settings: Value) -> PathBuf {
        let mut settings = settings.as_object().unwrap().clone();
        settings.insert("paths".into(), json!(fixture.paths()));
        let path = fixture.root.join("config.json");
        fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        path
    }

    fn commit_screenshot(store: &Store) -> (Envelope, ObjectRef) {
        let mut entry = record(store);
        let parent = entry.revision_id;
        store.commit(vec![entry.clone()], BTreeMap::new()).unwrap();
        let image = reference(PNG);
        entry.revision_id = Uuid::new_v4();
        entry.operation_id = Uuid::new_v4();
        entry.parents.insert(parent);
        entry.media_descriptors.push(Media {
            sha256: image.sha256.clone(),
            bytes: image.bytes,
            media_type: "image/png".into(),
        });
        store
            .commit(
                vec![entry.clone()],
                BTreeMap::from([(image.sha256.clone(), PNG.to_vec())]),
            )
            .unwrap();
        (entry, image)
    }

    fn import_report(report: import::Report) -> Value {
        let mut out = Vec::new();
        report.write_to(&mut out).unwrap();
        let report: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["runtime_activation"], "unverified");
        assert!(!String::from_utf8_lossy(&out).contains("PRIVATE"));
        report
    }

    #[test]
    fn package_round_trips_exact_data_and_independently_imports_preferences() {
        let origin = Fixture::new();
        let source = origin.open();
        let lease = lease(&source);
        let (head, image) = commit_screenshot(&source);
        let settings = json!({
            "personalization": {"enabled":false, "text":"PRIVATE saved é\r\n日本語"},
            "product_settings":{"reader_enabled":true},
            "research_policy":null,
        });
        let mut config = settings.clone();
        config["model"] = json!("PRIVATE-SOURCE-MODEL");
        config["model_credential"] = json!("PRIVATE_SOURCE_CREDENTIAL");
        let source_path = saved(&origin, config);
        fs::write(
            source.paths.credentials.join("drive.json"),
            b"PRIVATE_TOKEN",
        )
        .unwrap();
        fs::write(
            source.paths.data.join("config.snapshot.json"),
            b"PRIVATE_INVALID_TYPED_SNAPSHOT",
        )
        .unwrap();
        let root = origin.root.join("package");
        let mut attempted = None;
        let summary = source
            .export_isolated_package(&lease, &root, &source_path, &mut attempted)
            .unwrap()
            .unwrap();
        assert_eq!(
            Store::inspect_isolated_package(&root, attempted.as_deref()).unwrap(),
            summary
        );
        assert_eq!(summary.declared_settings, 3);
        assert!(summary.data.complete_media && !summary.data.configuration_included);
        assert!(Store::inspect_backup(&root).is_err());
        let artifact_bytes = fs::read(root.join(PREFERENCES)).unwrap();
        let artifact =
            Store::isolated_package_preferences(&root, attempted.as_deref().unwrap()).unwrap();
        let artifact = artifact.value();
        assert_eq!(
            artifact,
            &json!({"portable_configuration_version":1,"settings":settings})
        );
        assert_eq!(summary.portable_configuration, reference(&artifact_bytes));
        assert_eq!(
            summary.data_backup,
            reference(&fs::read(root.join(DATA).join(BACKUP)).unwrap())
        );
        assert_eq!(
            fs::read(root.join(DATA).join(OBJECTS).join(&image.sha256)).unwrap(),
            PNG
        );
        assert!(!serde_json::to_string(&summary).unwrap().contains("PRIVATE"));
        assert!(
            !String::from_utf8_lossy(&fs::read(root.join(MARKER)).unwrap()).contains("PRIVATE")
        );

        let destination = Fixture::new();
        let destination_store = destination.open();
        let actor = destination_store.actor_id;
        assert_ne!(source.actor_id, actor);
        let config_path = saved(
            &destination,
            json!({
                "model":"PRIVATE-DESTINATION-MODEL",
                "base_url":"https://private.invalid/v1",
                "model_credential":"private_key",
                "product_settings":{"reader_enabled":false,"writer_enabled":true},
                "research_policy":"disabled",
            }),
        );
        let original_config = fs::read(&config_path).unwrap();
        let plan = destination_store
            .prepare_ordinary_restore(&root.join(DATA), || Ok(()))
            .unwrap();
        assert_eq!(plan.conditions.archive_sha256, summary.data_backup.sha256);
        assert!(matches!(
            destination_store
                .apply_ordinary_restore(&root.join(DATA), &plan.conditions, || Ok(()))
                .unwrap(),
            ordinary_restore::Publication::Verified(_)
        ));
        assert_eq!(fs::read(&config_path).unwrap(), original_config);
        assert_eq!(destination_store.actor_id, actor);
        assert!(
            destination_store
                .heads(head.namespace, head.record_id)
                .unwrap()
                == vec![head.clone()]
        );
        assert_eq!(destination_store.read_object(&image).unwrap(), PNG);
        assert_eq!(
            destination_store.manifests().unwrap(),
            source.manifests().unwrap()
        );
        drop(destination_store);

        let destination_state = import_report(import::inspect(Some(config_path.clone())));
        let run = |op, input: Value| {
            import_report(import::run_stdin(
                op,
                Some(config_path.clone()),
                &mut serde_json::to_vec(&input).unwrap().as_slice(),
            ))
        };
        let prepared = run(
            import::Operation::Prepare,
            json!({
                "portable_import_version":1,
                "operation":Uuid::new_v4(),
                "base_sha256":destination_state["data"]["base_sha256"],
                "config_path_sha256":destination_state["data"]["config_path_sha256"],
                "artifact":artifact,
            }),
        );
        assert_eq!(prepared["outcome"], "prepared");
        let intent = prepared["data"]["intent"].clone();
        assert_eq!(fs::read(&config_path).unwrap(), original_config);
        let changed_artifact = json!({"portable_configuration_version":1,"settings":{}});
        let swapped = run(
            import::Operation::Apply,
            json!({"intent":intent,"artifact":changed_artifact}),
        );
        assert_eq!(swapped["status"], "conflict");
        let mut changed_config = original_config.clone();
        changed_config.push(b'\n');
        fs::write(&config_path, changed_config).unwrap();
        let stale = run(
            import::Operation::Apply,
            json!({"intent":intent,"artifact":artifact}),
        );
        assert_eq!(stale["status"], "conflict");
        fs::write(&config_path, &original_config).unwrap();
        assert_eq!(
            run(
                import::Operation::Apply,
                json!({"intent":intent,"artifact":artifact})
            )["outcome"],
            "verified"
        );
        assert_eq!(
            run(import::Operation::Status, json!({"intent":intent}))["outcome"],
            "desired-file-observed"
        );
        let applied: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        let original: Value = serde_json::from_slice(&original_config).unwrap();
        for key in ["model", "model_credential", "paths", "base_url"] {
            assert_eq!(applied[key], original[key]);
        }
        assert_eq!(
            applied["product_settings"],
            json!({"reader_enabled":true,"writer_enabled":true})
        );
        assert_eq!(applied["personalization"], settings["personalization"]);
        assert!(applied.get("research_policy").unwrap().is_null());
        let reopened = destination.open();
        assert_eq!(reopened.actor_id, actor);
        assert!(reopened.heads(head.namespace, head.record_id).unwrap() == vec![head]);
        assert_eq!(reopened.read_object(&image).unwrap(), PNG);
    }

    #[test]
    fn package_uses_explicit_config_and_publishes_outer_marker_last() {
        let origin = Fixture::new();
        let source = origin.open();
        let lease = lease(&source);
        let root = origin.root.join("package");
        let mut attempted = None;
        let config = origin.root.join("missing.json");
        assert!(source
            .export_isolated_package(&lease, &root, &config, &mut attempted)
            .unwrap()
            .is_none());
        assert!(!root.exists() && attempted.is_none());
        commit_screenshot(&source);
        assert!(source
            .export_isolated_package(&lease, &root, &config, &mut attempted)
            .is_err());
        assert!(!root.exists() && attempted.is_none());
        let conf = saved(&origin, json!({}));
        let exact = fs::read(&conf).unwrap();
        fs::write(&conf, br#"{"product_settings":{},"product_settings":{}}"#).unwrap();
        assert!(source
            .export_isolated_package(&lease, &root, &conf, &mut attempted)
            .is_err());
        assert!(!root.exists() && attempted.is_none());
        let foreign = Fixture::new();
        let foreign_config = saved(&foreign, json!({}));
        assert!(source
            .export_isolated_package(&lease, &root, &foreign_config, &mut attempted)
            .is_err());
        assert!(!root.exists() && attempted.is_none());
        fs::write(&conf, &exact).unwrap();

        assert!(source
            .export_isolated_package_with(&lease, &root, &conf, &mut attempted, |path| {
                assert!(path.join(DATA).join(BACKUP).is_file());
                assert!(path.join(PREFERENCES).is_file());
                assert!(!path.join(MARKER).exists());
                let mut changed = exact.clone();
                changed.push(b'\n');
                fs::write(&conf, changed)?;
                Ok(())
            })
            .is_err());
        assert!(!root.join(MARKER).exists());
        assert!(attempted.is_none());
        assert!(Store::inspect_isolated_package(&root, None).is_err());
        assert!(source
            .export_isolated_package(&lease, &root, &conf, &mut attempted)
            .is_err());
        assert!(attempted.is_none());

        let published = origin.root.join("published");
        #[cfg(unix)]
        files::fail_next_atomic(
            published.join(MARKER),
            files::AtomicFailure::RenameAcknowledgment,
        );
        let outcome = source.export_isolated_package(&lease, &published, &conf, &mut attempted);
        assert_eq!(outcome.is_err(), cfg!(unix));
        let observed = Store::inspect_isolated_package(&published, attempted.as_deref()).unwrap();
        assert!(attempted.is_some());
        assert_eq!(observed.declared_settings, 0);
        assert!(
            Store::inspect_isolated_package(&published, Some(&digest(b"foreign attempt"))).is_err()
        );
        let fresh = origin.root.join("blocked replay");
        assert!(source
            .export_isolated_package(&lease, &fresh, &conf, &mut attempted)
            .is_err());
        assert!(!fresh.exists());
    }

    #[test]
    fn package_requires_exact_membership_and_component_correspondence() {
        let origin = Fixture::new();
        let source = origin.open();
        let lease = lease(&source);
        let (_, image) = commit_screenshot(&source);
        let config = saved(&origin, json!({}));
        let root = origin.root.join("package");
        let summary = source
            .export_isolated_package(&lease, &root, &config, &mut None)
            .unwrap()
            .unwrap();
        let marker_path = root.join(MARKER);
        let marker_bytes = fs::read(&marker_path).unwrap();
        let marker: Value = serde_json::from_slice(&marker_bytes).unwrap();
        for edit in [
            ("isolated_backup_version", json!(2)),
            ("unrecognized", json!(null)),
            (
                "portable_configuration",
                json!({"bytes":1,"sha256":summary.portable_configuration.sha256}),
            ),
            (
                "data_backup",
                json!({"bytes":summary.data_backup.bytes,"sha256":digest(b"other data")}),
            ),
        ] {
            let mut wrong = marker.clone();
            wrong[edit.0] = edit.1;
            fs::write(&marker_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
            assert!(Store::inspect_isolated_package(&root, None).is_err());
        }
        let mut duplicate = marker_bytes.clone();
        duplicate.pop();
        duplicate.extend_from_slice(br#","isolated_backup_version":1}"#);
        fs::write(&marker_path, duplicate).unwrap();
        assert!(Store::inspect_isolated_package(&root, None).is_err());
        fs::write(&marker_path, marker_bytes).unwrap();
        for path in [
            root.join("unknown"),
            root.join(DATA).join("unknown"),
            root.join(DATA).join(OBJECTS).join(digest(b"extra")),
        ] {
            fs::write(&path, b"extra").unwrap();
            assert!(Store::inspect_isolated_package(&root, None).is_err());
            fs::remove_file(&path).unwrap();
        }
        let artifact_path = root.join(PREFERENCES);
        let original = fs::read(&artifact_path).unwrap();
        let mut substituted = original.clone();
        substituted.push(b'\n');
        fs::write(&artifact_path, substituted).unwrap();
        assert!(Store::inspect_isolated_package(&root, None).is_err());
        fs::write(&artifact_path, &original).unwrap();
        let invalid = br#"{"portable_configuration_version":1,"settings":{"model":"private"}}"#;
        let mut reassigned = marker;
        reassigned["portable_configuration"] = json!(reference(invalid));
        fs::write(&artifact_path, invalid).unwrap();
        let original_marker = fs::read(&marker_path).unwrap();
        fs::write(&marker_path, serde_json::to_vec(&reassigned).unwrap()).unwrap();
        assert!(Store::inspect_isolated_package(&root, None).is_err());
        fs::write(&marker_path, original_marker).unwrap();
        fs::write(&artifact_path, original).unwrap();
        let object = root.join(DATA).join(OBJECTS).join(image.sha256);
        fs::write(&object, b"corrupt").unwrap();
        assert!(Store::inspect_isolated_package(&root, None).is_err());
        fs::remove_file(&object).unwrap();
        assert!(Store::inspect_isolated_package(&root, None).is_err());
        fs::write(&object, PNG).unwrap();
        assert_eq!(
            Store::inspect_isolated_package(&root, None).unwrap(),
            summary
        );
    }

    #[test]
    fn package_preserves_intentional_media_omission() {
        let origin = Fixture::new();
        let source = origin.open();
        let lease = lease(&source);
        let media = reference(PNG);
        let mut value = record(&source);
        value.media_descriptors.push(Media {
            sha256: media.sha256.clone(),
            bytes: media.bytes,
            media_type: "image/png".into(),
        });
        let encoded = serde_json::to_vec(&value).unwrap();
        let object = reference(&encoded);
        let coverage = Coverage::Omitted {
            sha256: media.sha256.clone(),
            reason: Omission::SizeDeferred,
        };
        source
            .import(
                Manifest {
                    format: FORMAT,
                    transaction_id: Uuid::new_v4(),
                    scope: Scope::LocalTransaction,
                    records: vec![object.clone()],
                    record_namespaces: BTreeMap::from([(object.sha256.clone(), value.namespace)]),
                    media: vec![],
                    media_coverage: vec![coverage.clone()],
                },
                BTreeMap::from([(object.sha256, encoded)]),
            )
            .unwrap();
        let config = saved(&origin, json!({}));
        let root = origin.root.join("partial");
        let report = source
            .export_isolated_package(&lease, &root, &config, &mut None)
            .unwrap()
            .unwrap();
        assert!(!report.data.complete_media);
        let (backup, _) = ordinary_restore::load_archive(&root.join(DATA)).unwrap();
        assert_eq!(backup.manifests[0].media_coverage, vec![coverage]);
        assert!(!root.join(DATA).join(OBJECTS).join(media.sha256).exists());
    }
}
