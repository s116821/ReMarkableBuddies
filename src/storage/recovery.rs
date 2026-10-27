use super::*;

#[path = "ordinary_export.rs"]
pub(crate) mod ordinary_export;
#[path = "ordinary_restore.rs"]
pub(crate) mod ordinary_restore;

/// Distinguish a proved prepublication conflict from a possible atomic publication.
pub(crate) enum ConfigPublication {
    Verified(String),
    Conflict,
    Refused,
    Unknown,
}
#[cfg(test)]
pub(crate) enum ConfigFault {
    AtomicCall,
    AfterPublication,
    ChangedReadback,
}
#[derive(Debug)]
struct ConfigRevisionChanged;
impl std::fmt::Display for ConfigRevisionChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("configuration revision changed")
    }
}
impl std::error::Error for ConfigRevisionChanged {}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    feedback_history: Option<FeedbackHistory>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedbackHistory {
    format: u32,
    origin_actor: Uuid,
    origin_generation: Uuid,
    namespace: Namespace,
    domain_schema_version: u32,
}
impl FeedbackHistory {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.format == 1
                && !self.origin_actor.is_nil()
                && !self.origin_generation.is_nil()
                && self.namespace == Namespace::UnassociatedFeedback
                && self.domain_schema_version == 1,
            "unsupported feedback archive origin"
        );
        Ok(())
    }
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

/// Verified historical archive membership for domain-owned validation.
/// No live token or current group/device/native authority is carried here.
pub struct ArchivedSelectedSnapshot {
    pub origin_actor: Uuid,
    pub origin_generation: Uuid,
    /// Original raw metadata identity, including noncanonical JSON bytes.
    pub head_reference: ObjectRef,
    pub transaction: selection::SelectionTransaction,
    /// Order matches transaction.selected.records; retain those exact ObjectRefs.
    pub selected_records: Vec<Envelope>,
    /// Manifest order, then each retained manifest's record reference order.
    pub retained_records: Vec<Envelope>,
    pub media: BTreeMap<String, files::Source>,
    /// Completeness of the entire archive, not only this requested scope.
    pub complete_media: bool,
}

impl Backup {
    fn read(source: &Path) -> Result<Self> {
        let bytes = files::read(&source.join("backup.json"), MAX_METADATA as u64)?;
        let backup: Self = serde_json::from_slice(&bytes).context("unsupported backup metadata")?;
        if backup.format == FORMAT {
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            ensure!(
                value.get("selected_history").is_none() && value.get("feedback_history").is_none(),
                "legacy backup cannot carry selected metadata"
            );
        }
        let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
        if backup.format == feedback_generation::VERSION {
            ensure!(
                raw.get("selected_history").is_none(),
                "selected feedback archives unsupported"
            );
        } else {
            ensure!(
                raw.get("feedback_history").is_none(),
                "legacy feedback metadata unsupported"
            );
        }
        match (backup.format, &backup.feedback_history) {
            (feedback_generation::VERSION, Some(feature)) => {
                feature.validate()?;
                ensure!(
                    backup.selected_history.is_none(),
                    "selected feedback archives unsupported"
                );
            }
            (1 | 2, None) => (),
            _ => anyhow::bail!("unsupported feedback archive feature"),
        }
        Ok(backup)
    }
    fn inspect(&self, source: &Path) -> Result<BackupInspection> {
        ensure!(
            (self.format == FORMAT && self.selected_history.is_none())
                || (self.format == SELECTED_BACKUP_FORMAT
                    && self.selected_history.is_some()
                    && self.config_snapshot.is_none())
                || (self.format == feedback_generation::VERSION
                    && self.feedback_history.is_some()
                    && self.selected_history.is_none()),
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
                if record.namespace == Namespace::UnassociatedFeedback {
                    let origin = self
                        .feedback_history
                        .as_ref()
                        .context("feedback archive origin missing")?;
                    crate::unassociated_feedback::validate_envelope(&record, origin.origin_actor)?;
                }
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
        self.export_policy(destination, false)
    }
    pub(crate) fn export_feedback(&self, destination: &Path) -> Result<bool> {
        self.export_policy(destination, true)
    }
    fn export_policy(&self, destination: &Path, feedback: bool) -> Result<bool> {
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
        let backup = if feedback {
            self.check_feedback(&inner)?;
            self.export_backup_policy(&inner, true)?
        } else {
            self.export_backup_locked(&inner)?
        };
        files::directory(&destination.join("objects"))?;
        let backup_bytes = serde_json::to_vec(&backup)?;
        ensure!(
            backup_bytes.len() <= MAX_METADATA,
            "backup metadata exceeds supported restore bound"
        );
        self.export_write_locked(
            &inner,
            destination,
            &backup,
            &backup_bytes,
            &mut false,
            files::atomic,
        )?;
        Ok(backup.complete_media)
    }
    // Shared collection/serialization and publication path; old caller order stays intact.
    fn export_backup_locked(&self, inner: &Inner) -> Result<Backup> {
        self.export_backup_policy(inner, false)
    }
    fn export_backup_policy(&self, inner: &Inner, feedback: bool) -> Result<Backup> {
        let root = self.generation(inner.generation);
        let version = feedback_generation::version(&root)?;
        ensure!(
            if feedback {
                version == feedback_generation::VERSION
                    && !root.join("selection-feature.json").exists()
            } else {
                version != feedback_generation::VERSION
            },
            "unsupported backup generation policy"
        );
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
        Ok(Backup {
            format: if feedback {
                feedback_generation::VERSION
            } else {
                FORMAT
            },
            manifests,
            objects: inventory.values().cloned().collect(),
            complete_media,
            config_snapshot,
            selected_history: None,
            feedback_history: feedback.then_some(FeedbackHistory {
                format: 1,
                origin_actor: self.actor_id,
                origin_generation: inner.generation,
                namespace: Namespace::UnassociatedFeedback,
                domain_schema_version: 1,
            }),
        })
    }
    fn export_write_locked(
        &self,
        inner: &Inner,
        destination: &Path,
        backup: &Backup,
        backup_bytes: &[u8],
        attempted: &mut bool,
        publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let root = self.generation(inner.generation);
        for item in &backup.objects {
            let path = root.join("objects").join(&item.sha256);
            files::safe_path(&path)?;
            files::copy_verified(
                &destination.join("objects").join(&item.sha256),
                File::open(path)?,
                item,
            )?;
        }
        *attempted = true;
        publish(&destination.join("backup.json"), backup_bytes)?;
        Ok(())
    }

    /// Verify a complete format-1 or selected format-2 archive without opening a
    /// live Store. Returns counts/completeness only; does not grant restore,
    /// membership, native or effect authority. Unknown metadata and incomplete
    /// archives refuse. Inventory/receipt counts are limited to 4096 and unique
    /// encoded record bytes and selected metadata each to 8 MiB. These inspection
    /// bounds can refuse a legacy archive accepted by the older restore path.
    pub fn inspect_backup(source: &Path) -> Result<BackupInspection> {
        files::safe_path(source)?;
        let backup = Backup::read(source)?;
        backup.inspect(source)
    }

    /// Inspect one archived selection for strict domain-owned payload validation.
    /// Validates the whole format-2 archive, including unrelated scopes, before
    /// returning historical references. Absence is not an empty winning closure.
    /// No Store is opened, no live token is minted, and restore remains disabled.
    pub fn inspect_selected_backup(
        source: &Path,
        scope: &selection::SelectionScope,
        limit: usize,
    ) -> Result<Option<ArchivedSelectedSnapshot>> {
        scope.validate()?;
        ensure!(
            (1..=MAX_ITEMS).contains(&limit),
            "invalid archived snapshot bound"
        );
        files::safe_path(source)?;
        let backup = Backup::read(source)?;
        let report = backup.inspect(source)?;
        let archive = backup
            .selected_history
            .as_ref()
            .context("selected inspection requires a format-2 archive")?;
        // Preserve verified raw metadata bytes; never derive head identity from
        // reserializing a transaction, or treat its historical pins as live ones.
        let validated = archive.validate(source)?;
        for reference in &archive.heads {
            let bytes = validated
                .metadata
                .get(&reference.sha256)
                .context("verified archived head is absent")?;
            let transaction: selection::SelectionTransaction = serde_json::from_slice(bytes)?;
            if &transaction.scope != scope {
                continue;
            }
            transaction.validate(limit)?;
            let selected_records = Self::validate_objects(source, &transaction.selected)?
                .into_iter()
                .map(|(record, _)| record)
                .collect();
            let mut retained_records = Vec::new();
            let mut media = BTreeMap::new();
            for manifest in &transaction.retained {
                retained_records.extend(
                    Self::validate_objects(source, manifest)?
                        .into_iter()
                        .map(|(record, _)| record),
                );
            }
            for manifest in std::iter::once(&transaction.selected).chain(&transaction.retained) {
                for item in &manifest.media {
                    if let Some(existing) = media.get(&item.sha256) {
                        let existing: &files::Source = existing;
                        ensure!(
                            existing.reference() == *item,
                            "conflicting archived media reference"
                        );
                    } else {
                        media.insert(
                            item.sha256.clone(),
                            files::Source::open(&source.join("objects").join(&item.sha256), item)?,
                        );
                    }
                }
            }
            return Ok(Some(ArchivedSelectedSnapshot {
                origin_actor: archive.origin_actor,
                origin_generation: archive.origin_generation,
                head_reference: reference.clone(),
                transaction,
                selected_records,
                retained_records,
                media,
                complete_media: report.complete_media,
            }));
        }
        Ok(None)
    }

    /// Preserve selected history as non-authoritative evidence in format 2.
    /// Copies reachable accepted history, retained evidence/media and unrelated
    /// committed scopes, excluding configuration, credentials and runtime/native
    /// trees. Does not change the source or serialize a live selection token.
    /// The destination must be new and outside owned Store roots; `backup.json`
    /// is published only after closure verification. If an error occurs after
    /// publication, use [`Self::inspect_backup`] to determine whether the archive
    /// completed before retrying. Partial staging is not a complete backup.
    /// Inspection capacity applies; [`Self::restore`] refuses this format until
    /// the separate selected restore activation policy is implemented.
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
        ensure!(
            feedback_generation::version(&root)? != feedback_generation::VERSION,
            "selected format3 backup unsupported"
        );
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
            feedback_history: None,
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
        self.restore_policy(source, false)
    }
    pub(crate) fn restore_feedback(&self, source: &Path) -> Result<()> {
        self.restore_policy(source, true)
    }
    fn restore_policy(&self, source: &Path, feedback: bool) -> Result<()> {
        files::safe_path(source)?;
        let backup = Backup::read(source)?;
        if let Some(config) = &backup.config_snapshot {
            config.validate_values()?;
        }
        ensure!(
            (backup.format == FORMAT
                || (feedback && backup.format == feedback_generation::VERSION))
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
            let records = Self::validate_objects(source, manifest)?;
            for (record, _) in &records {
                if record.namespace == Namespace::UnassociatedFeedback {
                    ensure!(
                        feedback && backup.format == feedback_generation::VERSION,
                        "legacy feedback archive unsupported"
                    );
                    crate::unassociated_feedback::validate_envelope(record, self.actor_id)?;
                }
            }
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
        let root = self.generation(inner.generation);
        if feedback {
            self.check_feedback(&inner)?;
            ensure!(
                !root.join("selection-feature.json").exists(),
                "selected feedback restore unsupported"
            );
            if let Some(origin) = &backup.feedback_history {
                origin.validate()?;
                ensure!(
                    origin.origin_actor == self.actor_id,
                    "cross-owner feedback restore unsupported"
                );
            }
        } else {
            ensure!(
                feedback_generation::version(&root)? != feedback_generation::VERSION,
                "ordinary restore does not support feedback3"
            );
        }
        selection::refuse_selected_maintenance(&root)?;
        let next = Uuid::new_v4();
        self.restore_locked(&mut inner, source, backup, next, false, &mut false)
    }
    /// The existing staging/rebuild/CURRENT writer shared by both entrypoints.
    fn restore_locked(
        &self,
        inner: &mut Inner,
        source: &Path,
        backup: Backup,
        next: Uuid,
        exclusive: bool,
        activation_attempted: &mut bool,
    ) -> Result<()> {
        if exclusive {
            let root = self.generation(next);
            files::safe_path(&root)?;
            // The old helper intentionally permits existing directories. This
            // new admission reserves its own empty root atomically first.
            fs::create_dir(&root)?;
        }
        let current_root = self.generation(inner.generation);
        if feedback_generation::version(&current_root)? == feedback_generation::VERSION {
            feedback_generation::create(&self.paths.data, next, self.actor_id)?;
        } else {
            Self::create_generation(&self.paths.data, next)?;
        }
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
        Self::trip(inner, Fault::BeforeActivation)?;
        *activation_attempted = true;
        files::atomic_json(&self.paths.data.join("CURRENT"), &next)?;
        inner.generation = next;
        inner.index = rebuilt;
        Self::trip(inner, Fault::AfterActivation)?;
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
        self.publish_config(
            path,
            expected,
            || Ok(serde_json::to_vec(config)?),
            &mut false,
            files::atomic,
            false,
        )
    }

    /// Declaration-preserving sibling of the typed writer; same CAS and atomic path.
    pub(crate) fn replace_config_document(
        &self,
        path: &Path,
        expected: &str,
        bytes: &[u8],
    ) -> ConfigPublication {
        self.replace_config_document_with(path, expected, bytes, files::atomic)
    }

    #[cfg(test)]
    pub(crate) fn replace_config_document_fault(
        &self,
        path: &Path,
        expected: &str,
        bytes: &[u8],
        fault: ConfigFault,
    ) -> ConfigPublication {
        self.replace_config_document_with(path, expected, bytes, |path, bytes| {
            if matches!(fault, ConfigFault::AtomicCall) {
                anyhow::bail!("injected atomic call failure");
            }
            files::atomic(path, bytes)?;
            match fault {
                ConfigFault::AfterPublication => anyhow::bail!("injected post-publication failure"),
                ConfigFault::ChangedReadback => files::atomic(path, b"{}"),
                ConfigFault::AtomicCall => unreachable!(),
            }
        })
    }

    fn replace_config_document_with(
        &self,
        path: &Path,
        expected: &str,
        bytes: &[u8],
        publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> ConfigPublication {
        let Ok((config, _)) = crate::config::inspection::document(bytes) else {
            return ConfigPublication::Refused;
        };
        if config.validate().is_err()
            || config.paths.data != self.paths.data
            || config.paths.cache != self.paths.cache
            || config.paths.credentials != self.paths.credentials
            || !valid_digest(expected)
        {
            return ConfigPublication::Refused;
        }
        let mut attempted = false;
        let result = self.publish_config(
            path,
            Some(expected),
            || Ok(bytes.to_vec()),
            &mut attempted,
            publish,
            true,
        );
        match result {
            Ok(hash) => ConfigPublication::Verified(hash),
            Err(_) if attempted => ConfigPublication::Unknown,
            Err(error) if error.is::<ConfigRevisionChanged>() => ConfigPublication::Conflict,
            Err(_) => ConfigPublication::Refused,
        }
    }

    fn publish_config(
        &self,
        path: &Path,
        expected: Option<&str>,
        encode: impl FnOnce() -> Result<Vec<u8>>,
        attempted: &mut bool,
        publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
        verify: bool,
    ) -> Result<String> {
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
        if current.as_deref() != expected {
            return Err(ConfigRevisionChanged.into());
        }
        let bytes = encode()?;
        ensure!(bytes.len() <= MAX_RECORD, "configuration exceeds bound");
        *attempted = true;
        publish(path, &bytes)?;
        if verify {
            ensure!(
                files::read(path, MAX_RECORD as u64)? == bytes,
                "configuration readback changed"
            );
        }
        Ok(digest(&bytes))
    }
}

#[cfg(target_os = "linux")]
impl Store {
    /// Same Store mutex as configuration/data publication; no credential-specific owner.
    pub(crate) fn publish_declared_model_credential(
        &self,
        path: &std::path::Path,
        intent: &crate::config::model_credential::linux::Intent,
        change: &crate::config::model_credential::linux::Change,
    ) -> crate::config::model_credential::Report {
        self.with_credential_mutex(|| {
            crate::config::model_credential::linux::publish_locked(
                path,
                &self.paths,
                intent,
                change,
            )
        })
    }
    fn with_credential_mutex(
        &self,
        publish: impl FnOnce() -> crate::config::model_credential::Report,
    ) -> crate::config::model_credential::Report {
        let Ok(_inner) = self.inner.lock() else {
            return crate::config::model_credential::Report::refusal(
                crate::config::model_credential::Operation::Apply,
                crate::config::basic::Status::Unavailable,
            );
        };
        publish()
    }
    #[cfg(test)]
    pub(crate) fn credential_publication_test_hook(
        &self,
        path: &std::path::Path,
        intent: &crate::config::model_credential::linux::Intent,
        change: &crate::config::model_credential::linux::Change,
        hook: impl FnMut(
            crate::config::model_credential::linux::Phase,
        ) -> std::result::Result<(), crate::config::basic::Status>,
    ) -> crate::config::model_credential::Report {
        self.with_credential_mutex(|| {
            crate::config::model_credential::linux::publish_with(
                path,
                &self.paths,
                intent,
                change,
                hook,
            )
        })
    }
}
