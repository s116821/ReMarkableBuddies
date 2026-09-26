use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    format: u32,
    manifests: Vec<Manifest>,
    objects: Vec<ObjectRef>,
    complete_media: bool,
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
        for item in inventory.values() {
            let bytes = files::read(&root.join("objects").join(&item.sha256), item.bytes)?;
            files::object(
                &destination.join("objects").join(&item.sha256),
                &item.sha256,
                &bytes,
            )?;
        }
        let complete_media = descriptors.iter().all(|h| inventory.contains_key(h));
        files::atomic_json(
            &destination.join("backup.json"),
            &Backup {
                format: FORMAT,
                manifests,
                objects: inventory.into_values().collect(),
                complete_media,
            },
        )?;
        Ok(complete_media)
    }

    /// Additive staged restore. Old generation and source are retained for rollback.
    pub fn restore(&self, source: &Path) -> Result<()> {
        files::safe_path(source)?;
        let backup: Backup = files::json(&source.join("backup.json"), MAX_METADATA as u64)?;
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
            let bytes = files::read(&source.join("objects").join(&item.sha256), item.bytes)?;
            ensure!(
                bytes.len() as u64 == item.bytes && digest(&bytes) == item.sha256,
                "backup integrity failure"
            );
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
                    let bytes = files::read(&root.join("objects").join(&item.sha256), item.bytes)?;
                    files::object(
                        &next_root.join("objects").join(&item.sha256),
                        &item.sha256,
                        &bytes,
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
}
