use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    format: u32,
    manifests: Vec<Manifest>,
    objects: Vec<ObjectRef>,
    complete_media: bool,
    config_snapshot: Option<crate::config::Config>,
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

    /// Additive staged restore. Old generation and source are retained for rollback.
    pub fn restore(&self, source: &Path) -> Result<()> {
        files::safe_path(source)?;
        let backup: Backup = files::json(&source.join("backup.json"), MAX_METADATA as u64)?;
        if let Some(config) = &backup.config_snapshot {
            config.validate()?;
        }
        ensure!(
            backup.format == FORMAT
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
