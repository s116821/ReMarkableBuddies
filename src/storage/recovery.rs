use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    format: u32,
    manifests: Vec<Manifest>,
    objects: Vec<ObjectRef>,
    complete_media: bool,
    config_snapshot: Option<crate::config::Config>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected_history: Option<selection::portable::SelectionArchive>,
}

const SELECTED_BACKUP_FORMAT: u32 = 2;

/// Read-only archive evidence. This report grants no restore/native authority.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupInspection {
    pub format: u32,
    pub manifests: usize,
    pub objects: usize,
    pub selected_scopes: usize,
    pub selection_receipts: usize,
    pub complete_media: bool,
    pub configuration_included: bool,
}

impl Backup {
    fn read(source: &Path) -> Result<Self> {
        let bytes = files::read(&source.join("backup.json"), MAX_METADATA as u64)?;
        let backup: Self = serde_json::from_slice(&bytes).context("unsupported backup metadata")?;
        if backup.format == FORMAT {
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            ensure!(
                value.get("selected_history").is_none(),
                "legacy backup cannot carry selected metadata"
            );
        }
        Ok(backup)
    }
    fn inspect(&self, source: &Path) -> Result<BackupInspection> {
        ensure!(
            (self.format == FORMAT && self.selected_history.is_none())
                || (self.format == SELECTED_BACKUP_FORMAT
                    && self.selected_history.is_some()
                    && self.config_snapshot.is_none()),
            "unsupported backup format/policy"
        );
        ensure!(
            self.manifests.len() <= MAX_ITEMS && self.objects.len() <= MAX_ITEMS,
            "backup inventory exceeds bound"
        );
        if let Some(config) = &self.config_snapshot {
            config.validate_values()?;
        }
        let mut inventory = BTreeMap::new();
        for item in &self.objects {
            ensure!(
                valid_digest(&item.sha256) && item.bytes <= MAX_MEDIA,
                "invalid backup object"
            );
            ensure!(
                inventory
                    .insert(item.sha256.clone(), item.clone())
                    .is_none(),
                "duplicate backup object"
            );
            let _ = files::Source::open(&source.join("objects").join(&item.sha256), item)?;
        }
        let mut required = BTreeSet::new();
        let mut descriptors = BTreeSet::new();
        let mut manifests = BTreeMap::new();
        let mut records: BTreeMap<Uuid, (Envelope, String)> = BTreeMap::new();
        let mut record_bytes = 0u64;
        for manifest in &self.manifests {
            ensure!(
                manifests
                    .insert(manifest.transaction_id, manifest)
                    .is_none(),
                "duplicate backup manifest"
            );
            for item in manifest.records.iter().chain(&manifest.media) {
                ensure!(
                    inventory.get(&item.sha256) == Some(item),
                    "backup inventory mismatch"
                );
                required.insert(item.sha256.clone());
            }
            for item in &manifest.records {
                if !records.values().any(|(_, hash)| hash == &item.sha256) {
                    record_bytes = record_bytes
                        .checked_add(item.bytes)
                        .context("backup size overflow")?;
                    ensure!(
                        record_bytes <= MAX_METADATA as u64,
                        "backup inspection record metadata exceeds bound"
                    );
                }
            }
            for (record, hash) in Store::validate_objects(source, manifest)? {
                if let Some((_, prior)) = records.get(&record.revision_id) {
                    ensure!(prior == &hash, "backup revision identity collision");
                }
                records.insert(record.revision_id, (record, hash));
            }
            descriptors.extend(manifest.media_coverage.iter().map(|c| c.hash().to_owned()));
        }
        Store::can_apply(
            &Index::default(),
            &records.into_values().collect::<Vec<_>>(),
        )?;
        let (selected_scopes, selection_receipts) = if let Some(archive) = &self.selected_history {
            let selected = archive.validate(source)?;
            for (hash, bytes) in &selected.metadata {
                ensure!(
                    inventory.get(hash)
                        == Some(&ObjectRef {
                            sha256: hash.clone(),
                            bytes: bytes.len() as u64
                        }),
                    "selection metadata missing from inventory"
                );
                required.insert(hash.clone());
            }
            for (id, manifest) in &selected.manifests {
                ensure!(
                    manifests.get(id).copied() == Some(manifest),
                    "selected history manifest missing or changed"
                );
            }
            (archive.heads.len(), selected.receipts)
        } else {
            (0, 0)
        };
        ensure!(
            required.len() == inventory.len(),
            "unexpected backup objects"
        );
        ensure!(
            self.complete_media == descriptors.iter().all(|hash| inventory.contains_key(hash)),
            "false media completeness"
        );
        Ok(BackupInspection {
            format: self.format,
            manifests: self.manifests.len(),
            objects: inventory.len(),
            selected_scopes,
            selection_receipts,
            complete_media: self.complete_media,
            configuration_included: self.config_snapshot.is_some(),
        })
    }
}

impl Store {
    /// Export only committed, verified data. A complete marker is published last.
    /// Configuration export is separate so credentials cannot enter through opaque input.
    pub fn export(&self, destination: &Path) -> Result<bool> {
        files::safe_path(destination)?;
        ensure!(
            !destination.starts_with(&self.paths.data) && !self.paths.data.starts_with(destination),
            "backup overlaps store"
        );
        ensure!(!destination.exists(), "export requires a new destination");
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            inner.index.unavailable == 0,
            "unavailable commits prevent complete export"
        );
        let root = self.generation(inner.generation);
        files::directory(&destination.join("objects"))?;
        selection::refuse_selected_maintenance(&root)?;
        let manifests: Vec<_> = inner.index.manifests.values().cloned().collect();
        let mut inventory = BTreeMap::new();
        let mut descriptors = BTreeSet::new();
        for manifest in &manifests {
            Self::validate_objects(&root, manifest)?;
            for item in manifest.records.iter().chain(&manifest.media) {
                inventory.insert(item.sha256.clone(), item.clone());
            }
            descriptors.extend(manifest.media_coverage.iter().map(|c| c.hash().to_owned()));
        }
        ensure!(
            manifests.len() <= MAX_ITEMS && inventory.len() <= MAX_ITEMS,
            "backup inventory exceeds supported restore bounds"
        );
        let complete_media = descriptors.iter().all(|h| inventory.contains_key(h));
        let config_path = self.paths.data.join("config.snapshot.json");
        let config_snapshot = if config_path.exists() {
            let config: crate::config::Config = files::json(&config_path, MAX_RECORD as u64)?;
            config.validate()?;
            Some(config)
        } else {
            None
        };
        let backup = Backup {
            format: FORMAT,
            manifests,
            objects: inventory.values().cloned().collect(),
            complete_media,
            config_snapshot,
            selected_history: None,
        };
        let backup_bytes = serde_json::to_vec(&backup)?;
        ensure!(
            backup_bytes.len() <= MAX_METADATA,
            "backup metadata exceeds supported restore bound"
        );
        for item in inventory.values() {
            let path = root.join("objects").join(&item.sha256);
            files::safe_path(&path)?;
            files::copy_verified(
                &destination.join("objects").join(&item.sha256),
                File::open(path)?,
                item,
            )?;
        }
        files::atomic(&destination.join("backup.json"), &backup_bytes)?;
        Ok(complete_media)
    }

    pub fn inspect_backup(source: &Path) -> Result<BackupInspection> {
        files::safe_path(source)?;
        let backup = Backup::read(source)?;
        backup.inspect(source)
    }

    /// Preserve selected history as non-authoritative evidence. Restore is separate.
    pub fn export_selected(&self, destination: &Path) -> Result<BackupInspection> {
        files::safe_path(destination)?;
        for owned in [&self.paths.data, &self.paths.cache, &self.paths.credentials] {
            ensure!(
                !destination.starts_with(owned) && !owned.starts_with(destination),
                "backup overlaps owned store roots"
            );
        }
        ensure!(!destination.exists(), "export requires a new destination");
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            inner.index.unavailable == 0,
            "unavailable commits prevent complete export"
        );
        let root = self.generation(inner.generation);
        let selected = selection::portable::collect(&root, self.actor_id, inner.generation)?;
        let manifests: Vec<_> = inner.index.manifests.values().cloned().collect();
        let mut inventory = BTreeMap::new();
        let mut descriptors = BTreeSet::new();
        for manifest in &manifests {
            Self::validate_objects(&root, manifest)?;
            for item in manifest.records.iter().chain(&manifest.media) {
                if let Some(prior) = inventory.insert(item.sha256.clone(), item.clone()) {
                    ensure!(prior == *item, "backup object reference collision");
                }
            }
            descriptors.extend(manifest.media_coverage.iter().map(|c| c.hash().to_owned()));
        }
        for (hash, bytes) in &selected.metadata {
            let item = ObjectRef {
                sha256: hash.clone(),
                bytes: bytes.len() as u64,
            };
            if let Some(prior) = inventory.insert(hash.clone(), item.clone()) {
                ensure!(prior == item, "backup metadata reference collision");
            }
        }
        ensure!(
            manifests.len() <= MAX_ITEMS && inventory.len() <= MAX_ITEMS,
            "backup inventory exceeds bound"
        );
        let backup = Backup {
            format: SELECTED_BACKUP_FORMAT,
            manifests,
            complete_media: descriptors.iter().all(|hash| inventory.contains_key(hash)),
            objects: inventory.values().cloned().collect(),
            config_snapshot: None,
            selected_history: Some(selected.archive),
        };
        let bytes = serde_json::to_vec(&backup)?;
        ensure!(bytes.len() <= MAX_METADATA, "backup metadata exceeds bound");
        Self::trip(&mut inner, Fault::BeforeObjects)?;
        files::directory(&destination.join("objects"))?;
        for item in inventory.values() {
            let target = destination.join("objects").join(&item.sha256);
            if let Some(bytes) = selected.metadata.get(&item.sha256) {
                files::object(&target, &item.sha256, bytes)?;
            } else {
                let source = root.join("objects").join(&item.sha256);
                files::safe_path(&source)?;
                files::copy_verified(&target, File::open(source)?, item)?;
            }
        }
        Self::trip(&mut inner, Fault::AfterObjects)?;
        let report = backup.inspect(destination)?;
        Self::trip(&mut inner, Fault::BeforeCommit)?;
        files::atomic(&destination.join("backup.json"), &bytes)?;
        Self::trip(&mut inner, Fault::AfterCommit)?;
        Ok(report)
    }

    /// Additive staged restore. Old generation and source are retained for rollback.
    pub fn restore(&self, source: &Path) -> Result<()> {
        files::safe_path(source)?;
        let backup = Backup::read(source)?;
        if let Some(config) = &backup.config_snapshot {
            config.validate_values()?;
        }
        ensure!(
            backup.format == FORMAT
                && backup.selected_history.is_none()
                && backup.manifests.len() <= MAX_ITEMS
                && backup.objects.len() <= MAX_ITEMS,
            "unsupported or oversized backup"
        );
        let mut inventory = BTreeMap::new();
        for item in &backup.objects {
            ensure!(
                valid_digest(&item.sha256) && item.bytes <= MAX_MEDIA,
                "invalid backup object"
            );
            ensure!(
                inventory
                    .insert(item.sha256.clone(), item.clone())
                    .is_none(),
                "duplicate backup object"
            );
            let _ = files::Source::open(&source.join("objects").join(&item.sha256), item)?;
        }
        let mut required = BTreeSet::new();
        let mut descriptors = BTreeSet::new();
        for manifest in &backup.manifests {
            Self::validate_objects(source, manifest)?;
            for item in manifest.records.iter().chain(&manifest.media) {
                ensure!(
                    inventory.get(&item.sha256) == Some(item),
                    "backup inventory mismatch"
                );
                required.insert(item.sha256.clone());
            }
            descriptors.extend(manifest.media_coverage.iter().map(|c| c.hash().to_owned()));
        }
        ensure!(
            required.len() == inventory.len(),
            "unexpected backup objects"
        );
        ensure!(
            backup.complete_media == descriptors.iter().all(|h| inventory.contains_key(h)),
            "false media completeness"
        );
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            inner.index.unavailable == 0,
            "repair unavailable local commits before restore"
        );
        selection::refuse_selected_maintenance(&self.generation(inner.generation))?;
        let next = Uuid::new_v4();
        Self::create_generation(&self.paths.data, next)?;
        let next_root = self.generation(next);
        if let Some(config) = &backup.config_snapshot {
            files::atomic_json(&next_root.join("restored-config.json"), config)?;
        }
        let current_root = self.generation(inner.generation);
        for (root, manifests) in [
            (
                current_root.as_path(),
                inner.index.manifests.values().cloned().collect::<Vec<_>>(),
            ),
            (source, backup.manifests),
        ] {
            for manifest in manifests {
                for item in manifest.records.iter().chain(&manifest.media) {
                    let path = root.join("objects").join(&item.sha256);
                    files::safe_path(&path)?;
                    files::copy_verified(
                        &next_root.join("objects").join(&item.sha256),
                        File::open(path)?,
                        item,
                    )?;
                }
                let path = next_root
                    .join("commits")
                    .join(format!("{}.json", manifest.transaction_id));
                if path.exists() {
                    ensure!(
                        files::json::<Manifest>(&path, MAX_METADATA as u64)? == manifest,
                        "restored transaction collision"
                    );
                } else {
                    files::atomic_json(&path, &manifest)?;
                }
            }
        }
        let rebuilt = self.recover(next)?;
        ensure!(
            rebuilt.unavailable == 0,
            "restore contains unresolved lineage or identities"
        );
        Self::trip(&mut inner, Fault::BeforeActivation)?;
        files::atomic_json(&self.paths.data.join("CURRENT"), &next)?;
        inner.generation = next;
        inner.index = rebuilt;
        Self::trip(&mut inner, Fault::AfterActivation)?;
        Ok(())
    }
    pub fn snapshot_config(&self, config: &crate::config::Config) -> Result<()> {
        config.validate()?;
        let _inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        files::atomic_json(&self.paths.data.join("config.snapshot.json"), config)
    }
    /// Caller must perform the public stop/confirm/lease maintenance protocol.
    pub fn replace_config(
        &self,
        path: &Path,
        expected: Option<&str>,
        config: &crate::config::Config,
    ) -> Result<String> {
        config.validate()?;
        files::safe_path(path)?;
        ensure!(
            !path.starts_with(&self.paths.data)
                && !path.starts_with(&self.paths.cache)
                && !path.starts_with(&self.paths.credentials),
            "configuration overlaps another owned category"
        );
        let _inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let current = if path.exists() {
            Some(digest(&files::read(path, MAX_RECORD as u64)?))
        } else {
            None
        };
        ensure!(
            current.as_deref() == expected,
            "configuration revision changed"
        );
        let bytes = serde_json::to_vec(config)?;
        ensure!(bytes.len() <= MAX_RECORD, "configuration exceeds bound");
        files::atomic(path, &bytes)?;
        Ok(digest(&bytes))
    }
}
