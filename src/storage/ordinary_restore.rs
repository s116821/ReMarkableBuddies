//! Conditional ordinary restore; shares its parent's existing activation writer.
use super::*;
use crate::config::inspection::strict_json;
use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Refused {
    InvalidInput,
    Unsupported,
    Unavailable,
    Conflict,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ordinary restore refused")
    }
}
impl std::error::Error for Refused {}
fn require(value: bool, refusal: Refused) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(refusal.into())
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Summary {
    pub manifests: usize,
    pub objects: usize,
    pub complete_media: bool,
    pub configuration_included: bool,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Preview {
    pub source: Summary,
    pub target: Summary,
    pub additional_manifests: usize,
    pub additive: bool,
    pub configuration_applied: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Conditions {
    pub archive_sha256: String,
    pub base_generation: Uuid,
    pub base_history_sha256: String,
    pub target_generation: Uuid,
    pub target_history_sha256: String,
    pub staged_configuration_sha256: Option<String>,
    pub complete_media: bool,
}
pub(crate) struct Plan {
    pub conditions: Conditions,
    pub preview: Preview,
}
#[derive(Debug)]
pub(crate) enum Publication {
    Verified(Summary),
    NotActivated,
    Unknown,
}
struct History {
    manifests: BTreeMap<Uuid, Manifest>,
    objects: BTreeMap<String, ObjectRef>,
    records: BTreeMap<Uuid, (Envelope, String)>,
    manifest_bytes: usize,
    record_bytes: u64,
    record_hashes: BTreeSet<String>,
}
impl History {
    fn empty() -> Self {
        Self {
            manifests: BTreeMap::new(),
            objects: BTreeMap::new(),
            records: BTreeMap::new(),
            manifest_bytes: 0,
            record_bytes: 0,
            record_hashes: BTreeSet::new(),
        }
    }
    fn add(&mut self, root: &Path, manifest: &Manifest) -> Result<()> {
        if let Some(old) = self.manifests.get(&manifest.transaction_id) {
            ensure!(old == manifest, "restore transaction collision");
            Store::validate_objects(root, manifest)?;
            return Ok(());
        }
        let encoded = serde_json::to_vec(manifest)?;
        let total = self
            .manifest_bytes
            .checked_add(encoded.len())
            .context("restore metadata overflow")?;
        ensure!(total <= MAX_METADATA, "restore manifest metadata bound");
        for reference in manifest.records.iter().chain(&manifest.media) {
            if let Some(old) = self.objects.get(&reference.sha256) {
                ensure!(old == reference, "restore object identity collision");
            }
            self.objects
                .insert(reference.sha256.clone(), reference.clone());
        }
        for (record, hash) in Store::validate_objects(root, manifest)? {
            // Require a record object only on this new admission; arbitrary
            // domain payload arrays remain valid and old readers are untouched.
            let bytes = files::read(&root.join("objects").join(&hash), MAX_RECORD as u64)?;
            ensure!(
                strict_json(&bytes)?.is_object(),
                "restore record object required"
            );
            if let Some((_, previous)) = self.records.get(&record.revision_id) {
                ensure!(previous == &hash, "restore revision collision");
            }
            self.records.insert(record.revision_id, (record, hash));
        }
        self.manifests
            .insert(manifest.transaction_id, manifest.clone());
        self.manifest_bytes = total;
        ensure!(
            self.manifests.len() <= MAX_ITEMS && self.objects.len() <= MAX_ITEMS,
            "restore inventory bound"
        );
        for r in &manifest.records {
            if self.record_hashes.insert(r.sha256.clone()) {
                self.record_bytes = self
                    .record_bytes
                    .checked_add(r.bytes)
                    .context("restore record bytes overflow")?;
            }
        }
        ensure!(
            self.record_bytes <= MAX_METADATA as u64,
            "restore record metadata bound"
        );
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        Store::can_apply(
            &Index::default(),
            &self.records.values().cloned().collect::<Vec<_>>(),
        )
    }
    fn hash(&self) -> Result<String> {
        history_hash(&self.manifests)
    }
    fn summary(&self, config: bool) -> Summary {
        Summary {
            manifests: self.manifests.len(),
            objects: self.objects.len(),
            complete_media: self
                .manifests
                .values()
                .flat_map(|m| &m.media_coverage)
                .all(|c| self.objects.contains_key(c.hash())),
            configuration_included: config,
        }
    }
}
fn history_hash(manifests: &BTreeMap<Uuid, Manifest>) -> Result<String> {
    let mut bytes = b"urn:remarkable-buddies:ordinary-history:v1:\n".to_vec();
    for (id, manifest) in manifests {
        bytes.extend_from_slice(
            format!("{id} {}\n", digest(&serde_json::to_vec(manifest)?)).as_bytes(),
        );
    }
    Ok(digest(&bytes))
}
fn object_records(raw: &Value) -> bool {
    raw.is_object()
        && ["records", "media", "media_coverage"].iter().all(|key| {
            raw.get(key)
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().all(Value::is_object))
        })
        && raw.get("record_namespaces").is_some_and(Value::is_object)
}
fn load_archive(source: &Path) -> Result<(Backup, String)> {
    files::safe_path(source).map_err(|_| Refused::InvalidInput)?;
    let bytes = files::read(&source.join("backup.json"), MAX_METADATA as u64)
        .map_err(|_| Refused::Unavailable)?;
    let raw = strict_json(&bytes).map_err(|_| Refused::InvalidInput)?;
    require(raw.is_object(), Refused::InvalidInput)?;
    require(
        raw.get("format").and_then(Value::as_u64) == Some(u64::from(FORMAT))
            && raw.get("selected_history").is_none(),
        Refused::Unsupported,
    )?;
    require(
        raw.get("manifests")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(object_records))
            && raw
                .get("objects")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().all(Value::is_object)),
        Refused::InvalidInput,
    )?;
    if let Some(config) = raw.get("config_snapshot").filter(|c| !c.is_null()) {
        crate::config::inspection::document(&serde_json::to_vec(config)?)
            .map_err(|_| Refused::InvalidInput)?;
    }
    let backup: Backup = serde_json::from_slice(&bytes).map_err(|_| Refused::InvalidInput)?;
    backup.inspect(source).map_err(|_| Refused::InvalidInput)?;
    Ok((backup, digest(&bytes)))
}
fn staged_hash(backup: &Backup) -> Result<Option<String>> {
    backup
        .config_snapshot
        .as_ref()
        .map(|c| serde_json::to_vec(c).map(|b| digest(&b)))
        .transpose()
        .map_err(Into::into)
}
fn absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}
impl Store {
    fn ordinary_history(&self, generation: Uuid) -> Result<History> {
        let root = self.generation(generation);
        ensure!(
            files::json::<u32>(&root.join("format.json"), 64)? == FORMAT,
            "ordinary generation format required"
        );
        ensure!(
            absent(&root.join("selection-feature.json")),
            "selected feature is not ordinary restore authority"
        );
        selection::refuse_selected_maintenance(&root)?;
        let mut history = History::empty();
        let mut seen = BTreeSet::new();
        for entry in fs::read_dir(root.join("commits"))? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".stage-"))
            {
                continue;
            }
            ensure!(seen.len() < MAX_ITEMS, "restore manifest count bound");
            let bytes = files::read(&path, MAX_METADATA as u64)?;
            let raw = strict_json(&bytes)?;
            ensure!(object_records(&raw), "restore manifest object required");
            let manifest: Manifest = serde_json::from_slice(&bytes)?;
            ensure!(
                path.file_name()
                    == Some(std::ffi::OsStr::new(&format!(
                        "{}.json",
                        manifest.transaction_id
                    ))),
                "restore commit name mismatch"
            );
            ensure!(
                seen.insert(manifest.transaction_id),
                "duplicate restore manifest"
            );
            history.add(&root, &manifest)?;
        }
        history.validate()?;
        Ok(history)
    }
    fn ordinary_plan(
        &self,
        inner: &Inner,
        source: &Path,
        backup: &Backup,
        archive_sha256: String,
        target: Uuid,
    ) -> Result<Plan> {
        require(inner.index.unavailable == 0, Refused::Unsupported)?;
        let current: Uuid =
            files::json(&self.paths.data.join("CURRENT"), 128).map_err(|_| Refused::Unavailable)?;
        require(current == inner.generation, Refused::Conflict)?;
        let mut history = self
            .ordinary_history(inner.generation)
            .map_err(|_| Refused::Unsupported)?;
        require(
            history.manifests == inner.index.manifests,
            Refused::Conflict,
        )?;
        let base_history_sha256 = history.hash()?;
        let before = history.manifests.len();
        for manifest in &backup.manifests {
            history
                .add(source, manifest)
                .map_err(|_| Refused::Conflict)?;
        }
        history.validate().map_err(|_| Refused::Conflict)?;
        let target_summary = history.summary(backup.config_snapshot.is_some());
        Ok(Plan {
            conditions: Conditions {
                archive_sha256,
                base_generation: inner.generation,
                base_history_sha256,
                target_generation: target,
                target_history_sha256: history.hash()?,
                staged_configuration_sha256: staged_hash(backup)?,
                complete_media: target_summary.complete_media,
            },
            preview: Preview {
                source: Summary {
                    manifests: backup.manifests.len(),
                    objects: backup.objects.len(),
                    complete_media: backup.complete_media,
                    configuration_included: backup.config_snapshot.is_some(),
                },
                additional_manifests: target_summary.manifests - before,
                target: target_summary,
                additive: true,
                configuration_applied: false,
            },
        })
    }
    pub(crate) fn prepare_ordinary_restore(
        &self,
        source: &Path,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Plan> {
        let inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        let (backup, archive_hash) = load_archive(source)?;
        let next = Uuid::new_v4();
        require(
            next != inner.generation && absent(&self.generation(next)),
            Refused::Conflict,
        )?;
        self.ordinary_plan(&inner, source, &backup, archive_hash, next)
    }
    pub(crate) fn apply_ordinary_restore(
        &self,
        source: &Path,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Publication> {
        self.apply_ordinary_restore_inner(source, expected, binding, || {}, |_, _| Ok(()))
    }
    fn apply_ordinary_restore_inner(
        &self,
        source: &Path,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
        after_checks: impl FnOnce(),
        after_write: impl FnOnce(&Self, &Conditions) -> Result<()>,
    ) -> Result<Publication> {
        let mut inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        require(
            inner.generation == expected.base_generation,
            Refused::Conflict,
        )?;
        require(
            absent(&self.generation(expected.target_generation)),
            Refused::Conflict,
        )?;
        let (backup, archive_hash) = load_archive(source)?;
        require(archive_hash == expected.archive_sha256, Refused::Conflict)?;
        let actual = self.ordinary_plan(
            &inner,
            source,
            &backup,
            archive_hash,
            expected.target_generation,
        )?;
        require(
            actual.conditions.base_history_sha256 == expected.base_history_sha256
                && actual.conditions.target_history_sha256 == expected.target_history_sha256
                && actual.conditions.staged_configuration_sha256
                    == expected.staged_configuration_sha256
                && actual.conditions.complete_media == expected.complete_media,
            Refused::Conflict,
        )?;
        // Test-only callers can schedule a competing real Store operation here;
        // the actual same mutex remains held through publication and readback.
        after_checks();
        let mut attempted = false;
        let result = self.restore_locked(
            &mut inner,
            source,
            backup,
            expected.target_generation,
            true,
            &mut attempted,
        );
        if result.is_err() {
            return Ok(if attempted {
                Publication::Unknown
            } else {
                Publication::NotActivated
            });
        }
        if after_write(self, expected).is_err() {
            return Ok(Publication::Unknown);
        }
        match self.observe_ordinary_locked(&inner, expected) {
            Ok(Some(summary)) => Ok(Publication::Verified(summary)),
            _ => Ok(Publication::Unknown),
        }
    }
    fn observe_ordinary_locked(
        &self,
        inner: &Inner,
        expected: &Conditions,
    ) -> Result<Option<Summary>> {
        let current: Uuid = files::json(&self.paths.data.join("CURRENT"), 128)?;
        if current != inner.generation || current != expected.target_generation {
            return Ok(None);
        }
        let history = self.ordinary_history(current)?;
        if history.hash()? != expected.target_history_sha256 {
            return Ok(None);
        }
        let config_path = self.generation(current).join("restored-config.json");
        let config = match fs::symlink_metadata(&config_path) {
            Ok(_) => Some(digest(&files::read(&config_path, MAX_RECORD as u64)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let summary = history.summary(config.is_some());
        if config != expected.staged_configuration_sha256
            || summary.complete_media != expected.complete_media
        {
            return Ok(None);
        }
        Ok(Some(summary))
    }
    pub(crate) fn observe_ordinary_restore(
        &self,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Option<Summary>> {
        let inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        self.observe_ordinary_locked(&inner, expected)
            .map_err(|_| Refused::Unsupported.into())
    }
    #[cfg(test)]
    pub(crate) fn ordinary_restore_changed_readback(
        &self,
        source: &Path,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Publication> {
        self.apply_ordinary_restore_inner(
            source,
            expected,
            binding,
            || {},
            |store, conditions| {
                files::atomic(
                    &store
                        .generation(conditions.target_generation)
                        .join("restored-config.json"),
                    b"changed actual readback",
                )
            },
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    pub(crate) struct Fixture {
        pub root: PathBuf,
    }
    impl Fixture {
        pub fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("ordinary-restore-unit-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let fixture = Self { root };
            fs::write(fixture.root.join("config.json"), serde_json::to_vec(&json!({"paths":fixture.paths(),"model":"PRIVATE_MODEL","personalization":{"text":"PRIVATE_TEXT\r\n \u{03b1} "}})).unwrap()).unwrap();
            fixture
        }
        pub fn paths(&self) -> StorePaths {
            StorePaths {
                data: self.root.join("data"),
                cache: self.root.join("cache"),
                credentials: self.root.join("credentials"),
            }
        }
        pub fn open(&self) -> Arc<Store> {
            Arc::new(Store::open(self.paths()).unwrap())
        }
        pub fn archive(&self) -> PathBuf {
            let store = self.open();
            store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
            let path = self.root.join("archive");
            store.export(&path).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if std::thread::panicking() {
                return;
            }
            assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ordinary-restore-unit-"));
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    pub(crate) fn record(store: &Store) -> Envelope {
        Envelope {
            envelope_version: 1,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: json!({"text":"PRIVATE_RECORD"}),
            media_descriptors: vec![],
        }
    }
    #[test]
    fn independent_history_recipe_pins_order_and_empty_inventory() {
        let mut manifests = BTreeMap::new();
        assert_eq!(
            history_hash(&manifests).unwrap(),
            "19bf4c537633f421ba1fcec5e6897e9a29a392d701b7fe2ecdce93c21ec1272b"
        );
        for id in [2, 1] {
            let id = Uuid::from_u128(id);
            manifests.insert(
                id,
                Manifest {
                    format: 1,
                    transaction_id: id,
                    scope: Scope::LocalTransaction,
                    records: vec![
                        ObjectRef {
                            sha256: "22".repeat(32),
                            bytes: 19,
                        },
                        ObjectRef {
                            sha256: "11".repeat(32),
                            bytes: 17,
                        },
                    ],
                    record_namespaces: BTreeMap::from([
                        ("22".repeat(32), Namespace::SubjectMemory),
                        ("11".repeat(32), Namespace::Conversation),
                    ]),
                    media: vec![],
                    media_coverage: vec![],
                },
            );
        }
        // Independently calculated with Python hashlib and explicit field/line
        // bytes, not by another call to this fingerprint implementation.
        assert_eq!(
            history_hash(&manifests).unwrap(),
            "5897f9655ab01309f79d4a4d15cf8c97254c89a80e354059d3c37c0b0e199a61"
        );
        // A separate reviewer's vector also covers included/omitted coverage.
        // It qualifies serialization only, not stored objects or lineage.
        let vector: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/ordinary-restore-history-vector.json"
        ))
        .unwrap();
        let input: Vec<Manifest> =
            serde_json::from_value(vector["manifests_in_unsorted_input_order"].clone()).unwrap();
        for (manifest, expected) in input
            .iter()
            .zip(vector["serialized_manifests"].as_array().unwrap())
        {
            assert_eq!(
                serde_json::to_string(manifest).unwrap(),
                expected["serialized_utf8"].as_str().unwrap()
            );
        }
        let sorted = input.into_iter().map(|m| (m.transaction_id, m)).collect();
        assert_eq!(
            history_hash(&sorted).unwrap(),
            vector["history_sha256"].as_str().unwrap()
        );
    }
    #[test]
    fn actual_additive_restore_preserves_original_data_and_inactive_config() {
        let source = Fixture::new();
        source.archive();
        let source_store = source.open();
        let config = crate::config::Config {
            paths: source.paths(),
            model: Some("PRIVATE_ARCHIVED_MODEL".into()),
            ..Default::default()
        };
        source_store.snapshot_config(&config).unwrap();
        let archive = source.root.join("with-config");
        source_store.export(&archive).unwrap();
        let source_bytes = files::read(&archive.join("backup.json"), MAX_METADATA as u64).unwrap();
        let target = Fixture::new();
        let store = target.open();
        let local = record(&store);
        store.commit(vec![local.clone()], BTreeMap::new()).unwrap();
        let old = fs::read(target.paths().data.join("CURRENT")).unwrap();
        let config_bytes = fs::read(target.root.join("config.json")).unwrap();
        let secret = target.paths().credentials.join("keep-secret");
        fs::write(&secret, b"PRIVATE_CREDENTIAL").unwrap();
        let mode = store.sync_mode_snapshot().unwrap();
        let _lease = store.ordinary_mode_lease(&mode.token).unwrap();
        let plan = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        assert_eq!(plan.preview.additional_manifests, 1);
        assert_eq!(plan.preview.target.manifests, 2);
        assert!(matches!(
            store
                .apply_ordinary_restore(&archive, &plan.conditions, || Ok(()))
                .unwrap(),
            Publication::Verified(_)
        ));
        assert_eq!(
            store.heads(local.namespace, local.record_id).unwrap().len(),
            1
        );
        assert!(store
            .observe_ordinary_restore(&plan.conditions, || Ok(()))
            .unwrap()
            .is_some());
        assert!(store
            .generation(serde_json::from_slice(&old).unwrap())
            .exists());
        assert_eq!(fs::read(&secret).unwrap(), b"PRIVATE_CREDENTIAL");
        assert_eq!(
            fs::read(target.root.join("config.json")).unwrap(),
            config_bytes
        );
        assert_eq!(fs::read(archive.join("backup.json")).unwrap(), source_bytes);
        assert!(store
            .apply_ordinary_restore(&archive, &plan.conditions, || Ok(()))
            .is_err());
    }
    #[test]
    fn original_same_generation_replacement_source_and_reserved_target_refuse() {
        let source = Fixture::new();
        let archive = source.archive();
        let target = Fixture::new();
        let store = target.open();
        let mode = store.sync_mode_snapshot().unwrap();
        let _lease = store.ordinary_mode_lease(&mode.token).unwrap();
        let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
        assert!(matches!(
            store
                .apply_ordinary_restore(&archive, &p.conditions, || Ok(()))
                .unwrap_err()
                .downcast_ref::<Refused>(),
            Some(Refused::Conflict)
        ));
        assert!(absent(&store.generation(p.conditions.target_generation)));
        let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        let mut bytes = fs::read(archive.join("backup.json")).unwrap();
        bytes.push(b' ');
        fs::write(archive.join("backup.json"), &bytes).unwrap();
        assert!(store
            .apply_ordinary_restore(&archive, &p.conditions, || Ok(()))
            .is_err());
        let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        store.restore(&archive).unwrap();
        assert!(store
            .apply_ordinary_restore(&archive, &p.conditions, || Ok(()))
            .is_err());
        let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        fs::create_dir(store.generation(p.conditions.target_generation)).unwrap();
        let sentinel = store
            .generation(p.conditions.target_generation)
            .join("preserve-partial");
        fs::write(&sentinel, b"preserve").unwrap();
        assert!(store
            .apply_ordinary_restore(&archive, &p.conditions, || Ok(()))
            .is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
    }
    #[test]
    fn actual_before_after_activation_faults_keep_staging_and_truthful_phase() {
        let source = Fixture::new();
        let archive = source.archive();
        for fault in [Fault::BeforeActivation, Fault::AfterActivation] {
            let target = Fixture::new();
            let store = target.open();
            let mode = store.sync_mode_snapshot().unwrap();
            let _lease = store.ordinary_mode_lease(&mode.token).unwrap();
            let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
            let original = fs::read(store.paths.data.join("CURRENT")).unwrap();
            store.set_fault(fault).unwrap();
            let result = store
                .apply_ordinary_restore(&archive, &p.conditions, || Ok(()))
                .unwrap();
            if fault == Fault::BeforeActivation {
                assert!(matches!(result, Publication::NotActivated));
                assert_eq!(
                    fs::read(store.paths.data.join("CURRENT")).unwrap(),
                    original
                );
                assert!(store
                    .observe_ordinary_restore(&p.conditions, || Ok(()))
                    .unwrap()
                    .is_none());
            } else {
                assert!(matches!(result, Publication::Unknown));
                assert!(store
                    .observe_ordinary_restore(&p.conditions, || Ok(()))
                    .unwrap()
                    .is_some());
            }
            assert!(store
                .generation(p.conditions.target_generation)
                .join("commits")
                .is_dir());
            assert!(store.generation(p.conditions.base_generation).exists());
            assert!(store
                .apply_ordinary_restore(&archive, &p.conditions, || Ok(()))
                .is_err());
        }
    }
    #[test]
    fn actual_mutex_serializes_intervening_commit_through_readback() {
        let source = Fixture::new();
        let archive = source.archive();
        let target = Fixture::new();
        let store = target.open();
        let mode = store.sync_mode_snapshot().unwrap();
        let _lease = store.ordinary_mode_lease(&mode.token).unwrap();
        let conditions = store
            .prepare_ordinary_restore(&archive, || Ok(()))
            .unwrap()
            .conditions;
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer_store = store.clone();
        let writer_conditions = conditions.clone();
        let writer = std::thread::spawn(move || {
            writer_store.apply_ordinary_restore_inner(
                &archive,
                &writer_conditions,
                || Ok(()),
                || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                },
                |_, _| Ok(()),
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(store.inner.try_lock().is_err());
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let competing = store.clone();
        let later = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = competing.commit(vec![record(&competing)], BTreeMap::new());
            done_tx.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        assert!(matches!(
            writer.join().unwrap().unwrap(),
            Publication::Verified(_)
        ));
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        later.join().unwrap();
        assert_eq!(
            store.inner.lock().unwrap().generation,
            conditions.target_generation
        );
        // CURRENT still equals the intended UUID, but a later commit means its
        // complete desired closure no longer matches the prepared observation.
        assert!(store
            .observe_ordinary_restore(&conditions, || Ok(()))
            .unwrap()
            .is_none());
    }
    #[test]
    fn changed_actual_readback_and_forged_target_never_verify() {
        let source = Fixture::new();
        let archive = source.archive();
        let target = Fixture::new();
        let store = target.open();
        let mode = store.sync_mode_snapshot().unwrap();
        let _lease = store.ordinary_mode_lease(&mode.token).unwrap();
        let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        let mut forged = p.conditions.clone();
        forged.target_history_sha256 = "00".repeat(32);
        assert!(store
            .apply_ordinary_restore(&archive, &forged, || Ok(()))
            .is_err());
        assert!(absent(&store.generation(forged.target_generation)));
        assert!(matches!(
            store
                .ordinary_restore_changed_readback(&archive, &p.conditions, || Ok(()))
                .unwrap(),
            Publication::Unknown
        ));
        assert!(store
            .observe_ordinary_restore(&p.conditions, || Ok(()))
            .unwrap()
            .is_none());
    }
    #[test]
    fn record_collision_and_omitted_versus_missing_media_are_distinct() {
        let source = Fixture::new();
        let source_store = source.open();
        let target = Fixture::new();
        let store = target.open();
        let a = record(&source_store);
        let mut b = a.clone();
        b.actor_id = store.actor_id;
        b.payload = json!("different");
        source_store.commit(vec![a], BTreeMap::new()).unwrap();
        store.commit(vec![b], BTreeMap::new()).unwrap();
        let archive = source.root.join("collision");
        source_store.export(&archive).unwrap();
        assert!(store.prepare_ordinary_restore(&archive, || Ok(())).is_err());
        let media_source = Fixture::new();
        let media_store = media_source.open();
        let mut e = record(&media_store);
        let h = digest(b"intentionally omitted media");
        e.media_descriptors.push(Media {
            sha256: h.clone(),
            bytes: 27,
            media_type: "image/png".into(),
        });
        let bytes = serde_json::to_vec(&e).unwrap();
        let r = ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        let m = Manifest {
            format: 1,
            transaction_id: Uuid::new_v4(),
            scope: Scope::LocalTransaction,
            records: vec![r.clone()],
            record_namespaces: BTreeMap::from([(r.sha256.clone(), e.namespace)]),
            media: vec![],
            media_coverage: vec![Coverage::Omitted {
                sha256: h,
                reason: Omission::SizeDeferred,
            }],
        };
        media_store
            .import(m, BTreeMap::from([(r.sha256, bytes)]))
            .unwrap();
        let archive = media_source.root.join("partial");
        assert!(!media_store.export(&archive).unwrap());
        let p = store.prepare_ordinary_restore(&archive, || Ok(())).unwrap();
        assert!(!p.preview.target.complete_media);
        let mut raw: Value =
            serde_json::from_slice(&fs::read(archive.join("backup.json")).unwrap()).unwrap();
        raw["complete_media"] = json!(true);
        fs::write(
            archive.join("backup.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        assert!(store.prepare_ordinary_restore(&archive, || Ok(())).is_err());
    }
    #[test]
    fn malformed_and_oversized_archive_metadata_never_stages_or_activates() {
        let source = Fixture::new();
        let archive = source.archive();
        let target = Fixture::new();
        let store = target.open();
        let original = fs::read(archive.join("backup.json")).unwrap();
        let raw: Value = serde_json::from_slice(&original).unwrap();
        let mut array_manifest = raw.clone();
        array_manifest["manifests"][0] = serde_json::json!([]);
        let mut array_config = raw.clone();
        array_config["config_snapshot"] = serde_json::json!([]);
        let mut duplicate = b"{\"format\":1,".to_vec();
        duplicate.extend_from_slice(&original[1..]);
        let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
        let generations = fs::read_dir(store.paths.data.join("generations"))
            .unwrap()
            .count();
        for bytes in [
            b"[]".to_vec(),
            serde_json::to_vec(&array_manifest).unwrap(),
            serde_json::to_vec(&array_config).unwrap(),
            duplicate,
            vec![b' '; MAX_METADATA + 1],
        ] {
            fs::write(archive.join("backup.json"), bytes).unwrap();
            assert!(store.prepare_ordinary_restore(&archive, || Ok(())).is_err());
            assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
            assert_eq!(
                fs::read_dir(store.paths.data.join("generations"))
                    .unwrap()
                    .count(),
                generations
            );
        }
    }
    #[test]
    fn changed_generation_format_or_selected_feature_cannot_verify_readback() {
        let source = Fixture::new();
        let archive = source.archive();
        for feature in [false, true] {
            let target = Fixture::new();
            let store = target.open();
            let conditions = store
                .prepare_ordinary_restore(&archive, || Ok(()))
                .unwrap()
                .conditions;
            let result = store
                .apply_ordinary_restore_inner(
                    &archive,
                    &conditions,
                    || Ok(()),
                    || {},
                    |store, c| {
                        let root = store.generation(c.target_generation);
                        if feature {
                            files::atomic(
                                &root.join("selection-feature.json"),
                                b"unexpected selected feature",
                            )
                        } else {
                            files::atomic_json(&root.join("format.json"), &3u32)
                        }
                    },
                )
                .unwrap();
            assert!(matches!(result, Publication::Unknown));
            assert!(store
                .observe_ordinary_restore(&conditions, || Ok(()))
                .is_err());
        }
    }
}
