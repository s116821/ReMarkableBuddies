//! Original-bound format1 export. Shares the unchanged legacy writer.
use super::*;
use crate::config::inspection;
#[derive(Clone, Copy, Debug)]
pub(crate) enum Refused {
    InvalidInput,
    Unavailable,
    Unsupported,
    Conflict,
}
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ordinary export refused")
    }
}
impl std::error::Error for Refused {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Conditions {
    pub base_generation: Uuid,
    pub base_history_sha256: String,
    pub configuration_snapshot_sha256: Option<String>,
    pub archive_sha256: String,
    pub complete_media: bool,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Summary {
    pub format: u32,
    pub manifests: usize,
    pub objects: usize,
    pub complete_media: bool,
    pub configuration_included: bool,
}
fn summary(backup: &Backup) -> Summary {
    Summary {
        format: FORMAT,
        manifests: backup.manifests.len(),
        objects: backup.objects.len(),
        complete_media: backup.complete_media,
        configuration_included: backup.config_snapshot.is_some(),
    }
}
pub(crate) struct Plan {
    pub conditions: Conditions,
    pub source: Summary,
}
pub(crate) enum Publication {
    Verified(Summary),
    NotPublished,
    Unknown,
}
fn snapshot(store: &Store, backup: &Backup) -> Result<Option<String>> {
    let path = store.paths.data.join("config.snapshot.json");
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ensure!(backup.config_snapshot.is_none(), "snapshot changed");
            Ok(None)
        }
        Err(_) => Err(Refused::Unavailable.into()),
        Ok(meta) => {
            ensure!(meta.is_file(), "snapshot file required");
            let bytes = files::read(&path, MAX_RECORD as u64)?;
            let (config, _) = inspection::document(&bytes).map_err(|_| Refused::Unsupported)?;
            ensure!(
                serde_json::to_value(Some(config))?
                    == serde_json::to_value(&backup.config_snapshot)?,
                "snapshot changed"
            );
            Ok(Some(digest(&bytes)))
        }
    }
}
fn collect(store: &Store, inner: &Inner) -> Result<(Plan, Backup, Vec<u8>)> {
    let history = store
        .ordinary_export_fingerprint(inner)
        .map_err(|_| Refused::Unsupported)?;
    let backup = store
        .export_backup_locked(inner)
        .map_err(|_| Refused::Unsupported)?;
    let snapshot = snapshot(store, &backup).map_err(|_| Refused::Unsupported)?;
    let bytes = serde_json::to_vec(&backup)?;
    ensure!(bytes.len() <= MAX_METADATA, "export metadata bound");
    let plan = Plan {
        conditions: Conditions {
            base_generation: inner.generation,
            base_history_sha256: history,
            configuration_snapshot_sha256: snapshot,
            archive_sha256: digest(&bytes),
            complete_media: backup.complete_media,
        },
        source: summary(&backup),
    };
    Ok((plan, backup, bytes))
}
/// Reads only the explicit archive; no Store/source/config/mode/provider admission.
pub(crate) fn observe(destination: &Path, expected: &Conditions) -> Result<Option<Summary>> {
    files::safe_path(destination).map_err(|_| Refused::InvalidInput)?;
    match fs::symlink_metadata(destination.join("backup.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Refused::Unavailable.into()),
        Ok(meta) if !meta.is_file() => return Err(Refused::Unsupported.into()),
        _ => {}
    }
    let (backup, hash) = match ordinary_restore::load_archive(destination) {
        Ok(v) => v,
        Err(error) => {
            return match error.downcast_ref::<ordinary_restore::Refused>() {
                Some(ordinary_restore::Refused::Unavailable) => Err(Refused::Unavailable.into()),
                Some(
                    ordinary_restore::Refused::InvalidInput
                    | ordinary_restore::Refused::Unsupported,
                ) => Ok(None),
                _ => Err(Refused::Unavailable.into()),
            }
        }
    };
    if hash != expected.archive_sha256 || backup.complete_media != expected.complete_media {
        return Ok(None);
    }
    Ok(Some(summary(&backup)))
}
impl Store {
    pub(crate) fn prepare_ordinary_export(
        &self,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Plan> {
        let inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        Ok(collect(self, &inner)?.0)
    }
    pub(crate) fn apply_ordinary_export(
        &self,
        destination: &Path,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Publication> {
        self.apply_ordinary_export_inner(
            destination,
            expected,
            binding,
            || {},
            files::atomic,
            |_, _| Ok(()),
        )
    }
    fn apply_ordinary_export_inner(
        &self,
        destination: &Path,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
        after_checks: impl FnOnce(),
        publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
        after_marker: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> Result<Publication> {
        let inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        if inner.generation != expected.base_generation {
            return Err(Refused::Conflict.into());
        }
        let (actual, backup, bytes) = collect(self, &inner)?;
        if actual.conditions != *expected {
            return Err(Refused::Conflict.into());
        }
        after_checks();
        fs::create_dir(destination).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Refused::Conflict
            } else {
                Refused::Unavailable
            }
        })?;
        let mut attempted = false;
        let result = (|| -> Result<()> {
            files::directory(&destination.join("objects"))?;
            self.export_write_locked(
                &inner,
                destination,
                &backup,
                &bytes,
                &mut attempted,
                publish,
            )?;
            after_marker(destination, &bytes)?;
            Ok(())
        })();
        if result.is_err() {
            return Ok(if attempted {
                Publication::Unknown
            } else {
                Publication::NotPublished
            });
        }
        match observe(destination, expected) {
            Ok(Some(s)) => Ok(Publication::Verified(s)),
            _ => Ok(Publication::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::ordinary_restore::tests::{record, Fixture};
    use super::*;
    use std::sync::mpsc;
    fn export(store: &Store, dest: &Path, conditions: &Conditions) -> Publication {
        store
            .apply_ordinary_export(dest, conditions, || Ok(()))
            .unwrap()
    }
    #[test]
    fn independent_empty_marker_and_history_match_actual_shared_core() {
        let f = Fixture::new();
        let store = f.open();
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        assert_eq!(
            plan.conditions.base_history_sha256,
            "19bf4c537633f421ba1fcec5e6897e9a29a392d701b7fe2ecdce93c21ec1272b"
        );
        assert_eq!(
            plan.conditions.archive_sha256,
            "2d61ecb63ec1e95d63e178bad36e1a04131c04e586a00c829782f62c7daa1d61"
        );
        let dest = f.root.join("export");
        assert!(matches!(
            export(&store, &dest, &plan.conditions),
            Publication::Verified(_)
        ));
        assert_eq!(fs::read(dest.join("backup.json")).unwrap(),br#"{"format":1,"manifests":[],"objects":[],"complete_media":true,"config_snapshot":null}"#);
        assert!(observe(&dest, &plan.conditions).unwrap().is_some());
    }
    #[test]
    fn original_same_generation_history_and_raw_snapshot_changes_refuse_before_creation() {
        let f = Fixture::new();
        let store = f.open();
        let first = store.prepare_ordinary_export(|| Ok(())).unwrap();
        store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
        let dest = f.root.join("out");
        assert!(store
            .apply_ordinary_export(&dest, &first.conditions, || Ok(()))
            .is_err());
        assert!(!dest.exists());
        let config: crate::config::Config =
            serde_json::from_slice(&fs::read(f.root.join("config.json")).unwrap()).unwrap();
        store.snapshot_config(&config).unwrap();
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        let path = store.paths.data.join("config.snapshot.json");
        let mut b = fs::read(&path).unwrap();
        b.push(b' ');
        fs::write(&path, &b).unwrap();
        assert!(store
            .apply_ordinary_export(&dest, &plan.conditions, || Ok(()))
            .is_err());
        assert!(!dest.exists());
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        assert_eq!(
            plan.conditions.configuration_snapshot_sha256,
            Some(digest(&b))
        );
        assert!(matches!(
            export(&store, &dest, &plan.conditions),
            Publication::Verified(_)
        ));
        let target = Fixture::new();
        let restored = target.open();
        let restore = restored.prepare_ordinary_restore(&dest, || Ok(())).unwrap();
        restored
            .apply_ordinary_restore(&dest, &restore.conditions, || Ok(()))
            .unwrap();
        assert_eq!(restored.manifests().unwrap().len(), 1);
        assert_eq!(fs::read(&path).unwrap(), b);
    }
    #[test]
    fn exclusive_destination_race_retains_competitor_and_no_marker() {
        let f = Fixture::new();
        let store = f.open();
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        let dest = f.root.join("race");
        let r = store.apply_ordinary_export_inner(
            &dest,
            &plan.conditions,
            || Ok(()),
            || {
                fs::create_dir(&dest).unwrap();
                fs::write(dest.join("sentinel"), b"competitor").unwrap();
            },
            files::atomic,
            |_, _| Ok(()),
        );
        assert!(matches!(
            r.err().unwrap().downcast_ref::<Refused>(),
            Some(Refused::Conflict)
        ));
        assert_eq!(fs::read(dest.join("sentinel")).unwrap(), b"competitor");
        assert!(!dest.join("backup.json").exists());
    }
    #[test]
    fn same_real_mutex_spans_checks_copy_marker_and_readback() {
        let f = Fixture::new();
        let store = f.open();
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        let dest = f.root.join("out");
        let (start_tx, start_rx) = mpsc::channel();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let other = store.clone();
        let worker = std::thread::spawn(move || {
            start_rx.recv().unwrap();
            attempt_tx.send(()).unwrap();
            other.commit(vec![record(&other)], BTreeMap::new()).unwrap();
            done_tx.send(()).unwrap();
        });
        let result = store
            .apply_ordinary_export_inner(
                &dest,
                &plan.conditions,
                || Ok(()),
                || {
                    start_tx.send(()).unwrap();
                    attempt_rx.recv().unwrap();
                    assert!(store.inner.try_lock().is_err());
                },
                |path, b| {
                    assert!(store.inner.try_lock().is_err());
                    assert!(done_rx.try_recv().is_err());
                    files::atomic(path, b)
                },
                |_, _| {
                    assert!(store.inner.try_lock().is_err());
                    assert!(done_rx.try_recv().is_err());
                    Ok(())
                },
            )
            .unwrap();
        assert!(matches!(result, Publication::Verified(_)));
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(store.manifests().unwrap().len(), 1);
        assert_eq!(Store::inspect_backup(&dest).unwrap().manifests, 0);
    }
    #[test]
    fn actual_copy_failure_before_marker_retains_partial_output() {
        let f = Fixture::new();
        let store = f.open();
        store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        let dest = f.root.join("partial");
        let path = {
            let inner = store.inner.lock().unwrap();
            let hash = &inner.index.manifests.values().next().unwrap().records[0].sha256;
            store
                .generation(inner.generation)
                .join("objects")
                .join(hash)
        };
        let r = store
            .apply_ordinary_export_inner(
                &dest,
                &plan.conditions,
                || Ok(()),
                || {
                    fs::remove_file(&path).unwrap();
                },
                files::atomic,
                |_, _| Ok(()),
            )
            .unwrap();
        assert!(matches!(r, Publication::NotPublished));
        assert!(dest.join("objects").is_dir());
        assert!(!dest.join("backup.json").exists());
        assert!(!path.exists());
    }
    #[test]
    fn actual_marker_attempt_and_changed_readback_are_unknown_without_cleanup() {
        for fault in 0..3 {
            let f = Fixture::new();
            let store = f.open();
            let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
            let dest = f.root.join("out");
            let r = store
                .apply_ordinary_export_inner(
                    &dest,
                    &plan.conditions,
                    || Ok(()),
                    || {},
                    |path, b| {
                        if fault == 0 {
                            anyhow::bail!("atomic attempted");
                        }
                        files::atomic(path, b)
                    },
                    |path, _| {
                        if fault == 1 {
                            anyhow::bail!("after marker");
                        }
                        if fault == 2 {
                            fs::write(path.join("backup.json"), b"{}").unwrap();
                        }
                        Ok(())
                    },
                )
                .unwrap();
            assert!(matches!(r, Publication::Unknown));
            assert!(dest.is_dir());
            assert_eq!(
                observe(&dest, &plan.conditions).unwrap().is_some(),
                fault == 1
            );
        }
    }
    #[test]
    fn selected_feature_and_corrupt_snapshot_refuse_before_writes() {
        let f = Fixture::new();
        let store = f.open();
        let current = store.inner.lock().unwrap().generation;
        fs::write(
            store.generation(current).join("selection-feature.json"),
            b"{}",
        )
        .unwrap();
        assert!(store.prepare_ordinary_export(|| Ok(())).is_err());
        fs::remove_file(store.generation(current).join("selection-feature.json")).unwrap();
        fs::write(store.paths.data.join("config.snapshot.json"), b"[]").unwrap();
        assert!(store.prepare_ordinary_export(|| Ok(())).is_err());
    }
    #[test]
    fn intentional_omission_is_valid_but_missing_promised_object_is_not_observed() {
        use serde_json::{json, Value};
        let f = Fixture::new();
        let store = f.open();
        let hash = digest(b"intentionally omitted payload");
        let mut e = record(&store);
        e.media_descriptors.push(Media {
            sha256: hash.clone(),
            bytes: 29,
            media_type: "image/png".into(),
        });
        let bytes = serde_json::to_vec(&e).unwrap();
        let object = ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        let manifest = Manifest {
            format: 1,
            transaction_id: Uuid::new_v4(),
            scope: Scope::LocalTransaction,
            records: vec![object.clone()],
            record_namespaces: BTreeMap::from([(object.sha256.clone(), e.namespace)]),
            media: vec![],
            media_coverage: vec![Coverage::Omitted {
                sha256: hash.clone(),
                reason: Omission::SizeDeferred,
            }],
        };
        store
            .import(manifest, BTreeMap::from([(object.sha256, bytes)]))
            .unwrap();
        let plan = store.prepare_ordinary_export(|| Ok(())).unwrap();
        assert!(!plan.conditions.complete_media);
        let dest = f.root.join("partial-media");
        assert!(matches!(
            export(&store, &dest, &plan.conditions),
            Publication::Verified(_)
        ));
        assert!(
            !observe(&dest, &plan.conditions)
                .unwrap()
                .unwrap()
                .complete_media
        );
        let mut value: Value =
            serde_json::from_slice(&fs::read(dest.join("backup.json")).unwrap()).unwrap();
        let promised = json!({"sha256":hash,"bytes":29});
        value["manifests"][0]["media"] = json!([promised.clone()]);
        value["manifests"][0]["media_coverage"] =
            json!([{"availability":"included","sha256":hash}]);
        value["objects"].as_array_mut().unwrap().push(promised);
        value["complete_media"] = json!(true);
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(dest.join("backup.json"), &bytes).unwrap();
        let mut matching = plan.conditions.clone();
        matching.archive_sha256 = digest(&bytes);
        matching.complete_media = true;
        assert!(observe(&dest, &matching).unwrap().is_none());
    }
}
