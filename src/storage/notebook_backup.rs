//! Explicit ordinary same-actor notebook archives. Legacy backup formats stay unchanged.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotebookBackup {
    format: u32,
    origin_actor: Uuid,
    origin_generation: Uuid,
    notebook_feature: u32,
    feedback_feature: bool,
    selected_feature: bool,
    manifests: Vec<Manifest>,
    objects: Vec<ObjectRef>,
    complete_media: bool,
    config_snapshot: Option<crate::config::Config>,
}
impl NotebookBackup {
    fn read(source: &Path) -> Result<Self> {
        files::safe_path(source)?;
        let value: Self = files::json(&source.join("backup.json"), MAX_METADATA as u64)?;
        ensure!(
            value.format == 4
                && value.notebook_feature == 1
                && !value.origin_actor.is_nil()
                && !value.origin_generation.is_nil(),
            "unsupported notebook backup"
        );
        Ok(value)
    }
    fn inspect(&self, source: &Path) -> Result<BackupInspection> {
        if let Some(config) = &self.config_snapshot {
            config.validate_values()?;
        }
        ensure!(
            self.manifests.len() <= MAX_ITEMS && self.objects.len() <= MAX_ITEMS,
            "notebook backup inventory bound"
        );
        let mut inventory = BTreeMap::new();
        for item in &self.objects {
            ensure!(
                valid_digest(&item.sha256) && item.bytes <= MAX_MEDIA,
                "invalid notebook backup object"
            );
            ensure!(
                inventory
                    .insert(item.sha256.clone(), item.clone())
                    .is_none(),
                "duplicate notebook backup object"
            );
            files::Source::open(&source.join("objects").join(&item.sha256), item)?;
        }
        let mut required = BTreeSet::new();
        let mut manifests = BTreeSet::new();
        let mut records = BTreeMap::new();
        let mut bytes = 0u64;
        let mut record_hashes = BTreeSet::new();
        for manifest in &self.manifests {
            ensure!(
                matches!(manifest.scope, Scope::LocalTransaction),
                "selected notebook backup unsupported"
            );
            ensure!(
                manifests.insert(manifest.transaction_id),
                "duplicate notebook manifest"
            );
            for item in manifest.records.iter().chain(&manifest.media) {
                ensure!(
                    inventory.get(&item.sha256) == Some(item),
                    "notebook inventory mismatch"
                );
                required.insert(item.sha256.clone());
            }
            for item in &manifest.records {
                if record_hashes.insert(item.sha256.clone()) {
                    bytes = bytes
                        .checked_add(item.bytes)
                        .context("notebook metadata overflow")?;
                    ensure!(bytes <= MAX_METADATA as u64, "notebook metadata bound");
                }
            }
            // This authority comes only from validated format4 metadata, never
            // from arbitrary files a legacy archive could plant beside objects.
            for (record, hash) in
                Store::validate_objects_policy(source, manifest, None, Some(self.origin_actor))?
            {
                if record.namespace == Namespace::UnassociatedFeedback {
                    ensure!(
                        self.feedback_feature,
                        "feedback feature absent from notebook archive"
                    );
                    crate::unassociated_feedback::validate_envelope(&record, self.origin_actor)?;
                }
                if let Some((_, previous)) =
                    records.insert(record.revision_id, (record, hash.clone()))
                {
                    ensure!(previous == hash, "notebook revision collision");
                }
            }
        }
        ensure!(
            required.len() == inventory.len(),
            "unreachable notebook backup inventory"
        );
        let records = records.into_values().collect::<Vec<_>>();
        Store::can_apply(&Index::default(), &records)?;
        notebook_generation::validate_records(source, &Index::default(), &records)?;
        let complete = self
            .manifests
            .iter()
            .flat_map(|m| &m.media_coverage)
            .all(|coverage| inventory.contains_key(coverage.hash()));
        ensure!(
            self.complete_media && complete,
            "incomplete notebook backup"
        );
        Ok(BackupInspection {
            format: 4,
            manifests: self.manifests.len(),
            objects: self.objects.len(),
            selected_scopes: 0,
            selection_receipts: 0,
            complete_media: true,
            configuration_included: self.config_snapshot.is_some(),
        })
    }
}
impl Store {
    /// Validate the entire ordinary format4 archive, without granting runtime authority.
    pub fn inspect_notebook_backup(source: &Path) -> Result<BackupInspection> {
        NotebookBackup::read(source)?.inspect(source)
    }
    /// Preserve all reachable original record/media objects and manifest lineage.
    /// Retains a validated optional configuration snapshot; credentials are excluded.
    pub fn export_notebook_backup(&self, destination: &Path) -> Result<BackupInspection> {
        files::safe_path(destination)?;
        for owned in [&self.paths.data, &self.paths.cache, &self.paths.credentials] {
            ensure!(
                !destination.starts_with(owned) && !owned.starts_with(destination),
                "notebook backup overlaps owned roots"
            );
        }
        ensure!(
            !destination.exists(),
            "notebook backup requires new destination"
        );
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        self.notebook_archive_custody(&inner)?;
        let root = self.generation(inner.generation);
        let mut inventory = BTreeMap::new();
        for manifest in inner.index.manifests.values() {
            ensure!(
                matches!(manifest.scope, Scope::LocalTransaction),
                "selected notebook backup unsupported"
            );
            Self::validate_objects(&root, manifest)?;
            for item in manifest.records.iter().chain(&manifest.media) {
                if let Some(prior) = inventory.insert(item.sha256.clone(), item.clone()) {
                    ensure!(prior == *item, "notebook object collision");
                }
            }
        }
        ensure!(
            inner
                .index
                .manifests
                .values()
                .flat_map(|m| &m.media_coverage)
                .all(|coverage| inventory.contains_key(coverage.hash())),
            "incomplete notebook backup"
        );
        let backup = NotebookBackup {
            format: 4,
            origin_actor: self.actor_id,
            origin_generation: inner.generation,
            notebook_feature: 1,
            feedback_feature: root.join(feedback_generation::MARKER).exists(),
            selected_feature: root.join("selection-feature.json").exists(),
            manifests: inner.index.manifests.values().cloned().collect(),
            objects: inventory.into_values().collect(),
            complete_media: true,
            config_snapshot: if self.paths.data.join("config.snapshot.json").exists() {
                let config: crate::config::Config = files::json(
                    &self.paths.data.join("config.snapshot.json"),
                    MAX_RECORD as u64,
                )?;
                config.validate()?;
                Some(config)
            } else {
                None
            },
        };
        let bytes = serde_json::to_vec(&backup)?;
        ensure!(
            bytes.len() <= MAX_METADATA
                && backup.objects.len() <= MAX_ITEMS
                && backup.manifests.len() <= MAX_ITEMS,
            "notebook backup bound"
        );
        // All policy checks precede destination staging.
        files::directory(&destination.join("objects"))?;
        for item in &backup.objects {
            files::copy_verified(
                &destination.join("objects").join(&item.sha256),
                File::open(root.join("objects").join(&item.sha256))?,
                item,
            )?;
        }
        let report = backup.inspect(destination)?;
        files::atomic(&destination.join("backup.json"), &bytes)?;
        Ok(report)
    }
    fn notebook_archive_custody(&self, inner: &Inner) -> Result<()> {
        notebook_generation::validate_ready(&self.paths, self.actor_id, inner.generation)?;
        ensure!(
            inner.index.unavailable == 0
                && inner.mode_handles == 0
                && inner.legacy_sync_handles == 0
                && inner.local_domain_handles == 0
                && inner.legacy_publisher_handles == 0
                && !self.writer_active.load(std::sync::atomic::Ordering::SeqCst)
                && !self.export_active.load(std::sync::atomic::Ordering::SeqCst),
            "notebook archive requires complete idle custody"
        );
        ensure!(
            sync::mode::ordinary_local(self)?,
            "notebook archive requires local mode"
        );
        let root = self.generation(inner.generation);
        selection::refuse_selected_maintenance(&root)?;
        let retained = root.join("selection-history");
        files::safe_path(&retained)?;
        ensure!(
            !retained.exists() || fs::read_dir(&retained)?.next().is_none(),
            "retained selected notebook archive unsupported"
        );
        Ok(())
    }
    /// Additive same-actor restore; no selected/group/cross-actor activation.
    /// A lost activation/capabilities acknowledgment requires explicit inspection
    /// and preparation; it must never be treated as a fresh execution grant.
    pub fn restore_notebook_backup(&self, source: &Path) -> Result<()> {
        for owned in [&self.paths.data, &self.paths.cache, &self.paths.credentials] {
            ensure!(
                !source.starts_with(owned) && !owned.starts_with(source),
                "notebook restore overlaps owned roots"
            );
        }
        let backup = NotebookBackup::read(source)?;
        backup.inspect(source)?;
        ensure!(
            backup.origin_actor == self.actor_id,
            "cross-actor notebook restore unsupported"
        );
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        self.notebook_archive_custody(&inner)?;
        let root = self.generation(inner.generation);
        let feedback = root.join(feedback_generation::MARKER).exists() || backup.feedback_feature;
        let selected_feature =
            root.join("selection-feature.json").exists() || backup.selected_feature;
        let next = Uuid::new_v4();
        notebook_generation::create(
            &self.paths.data,
            self.actor_id,
            next,
            feedback,
            selected_feature,
        )?;
        let next_root = self.generation(next);
        if let Some(config) = &backup.config_snapshot {
            files::atomic_json(&next_root.join("restored-config.json"), config)?;
        }
        for (origin, manifests) in [
            (
                &root,
                inner.index.manifests.values().cloned().collect::<Vec<_>>(),
            ),
            (&source.to_owned(), backup.manifests),
        ] {
            for manifest in manifests {
                for item in manifest.records.iter().chain(&manifest.media) {
                    files::copy_verified(
                        &next_root.join("objects").join(&item.sha256),
                        File::open(origin.join("objects").join(&item.sha256))?,
                        item,
                    )?;
                }
                let path = next_root
                    .join("commits")
                    .join(format!("{}.json", manifest.transaction_id));
                if path.exists() {
                    ensure!(
                        files::json::<Manifest>(&path, MAX_METADATA as u64)? == manifest,
                        "notebook restored transaction collision"
                    );
                } else {
                    files::atomic_json(&path, &manifest)?;
                }
            }
        }
        let rebuilt = self.recover(next)?;
        ensure!(
            rebuilt.unavailable == 0,
            "notebook restore unresolved lineage"
        );
        Self::trip(&mut inner, Fault::BeforeActivation)?;
        files::atomic_json(&self.paths.data.join("CURRENT"), &next)?;
        inner.generation = next;
        inner.index = rebuilt;
        notebook_generation::publish_capabilities(self, next)?;
        Self::trip(&mut inner, Fault::AfterActivation)?;
        Ok(())
    }
}
