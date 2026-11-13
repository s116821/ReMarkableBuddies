//! Original-only target restore under the existing conditional notebook owner.
use super::conditional_restore::{self as conditional, target_history_hash};
use super::*;
use crate::storage::ordinary_restore::{Conditions, History, Summary};

pub(in crate::storage) const ORIGINAL: &str = "target-restore-original.json";
const HISTORY: &str = "target-restore-history.json";
// A canonical Vec adds delimiters to the existing cumulative manifest-byte cap.
const HISTORY_BYTES: usize = MAX_METADATA + MAX_ITEMS + 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::storage) struct Original {
    format: u32,
    pub(in crate::storage) operation: Uuid,
    pub(in crate::storage) actor: Uuid,
    pub(in crate::storage) conditions: Conditions,
    pub(in crate::storage) mode_sha256: String,
    source: PathBuf,
    paths_sha256: String,
    base: Option<target_generation::ArchiveOrigin>,
    archive: Option<target_generation::ArchiveOrigin>,
    flags: (bool, bool),
    history_sha256: String,
    work: u64,
    metadata: u64,
    media: u64,
    initial: selection::SelectedReadCheckpoint,
}
impl Original {
    pub(in crate::storage) fn sha256(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(self)?))
    }
    pub(in crate::storage) fn validate(&self) -> Result<()> {
        let c = &self.conditions;
        ensure!(
            self.format == 1
                && !self.actor.is_nil()
                && !c.base_generation.is_nil()
                && !c.target_generation.is_nil()
                && c.base_generation != c.target_generation
                && self.operation
                    == Uuid::new_v5(&c.target_generation, b"ordinary-target-restore-v1")
                && c.complete_media
                && (self.base.is_some() || self.archive.is_some())
                && [
                    &c.archive_sha256,
                    &c.base_history_sha256,
                    &c.target_history_sha256,
                    &self.mode_sha256,
                    &self.history_sha256,
                    &self.paths_sha256,
                ]
                .iter()
                .all(|v| valid_digest(v))
                && c.staged_configuration_sha256
                    .as_ref()
                    .is_none_or(|v| valid_digest(v))
                && self.source.is_absolute()
                && self.source.to_string_lossy().len() <= 4096
                && !self.source.components().any(|c| matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                ))
                && self.work > 0
                && self.metadata > 0,
            "invalid original target restore"
        );
        if let Some(base) = &self.base {
            base.validate(self.actor, c.base_generation)?;
        }
        if let Some(archive) = &self.archive {
            archive.validate_actor(self.actor)?;
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    original: Original,
    remaining: selection::SelectedReadBudget,
}
impl Receipt {
    fn read(root: &Path) -> Result<Self> {
        let value: Self = files::json(&root.join(ORIGINAL), MAX_RECORD as u64)?;
        value.original.validate()?;
        ensure!(
            root.file_name().and_then(|s| s.to_str())
                == Some(
                    value
                        .original
                        .conditions
                        .target_generation
                        .to_string()
                        .as_str()
                )
                && value
                    .remaining
                    .is_monotonic_successor(&value.original.initial),
            "original target restore identity/allowance differs"
        );
        Ok(value)
    }
    fn reserve(&mut self, root: &Path) -> Result<selection::SelectedReadBudget> {
        let result = self.remaining.reserve(
            0,
            self.original.work,
            self.original.metadata,
            self.original.media,
        );
        // Persist spending, including sticky exhaustion, before repeated proof I/O.
        files::atomic_json(&root.join(ORIGINAL), self)?;
        result
    }
}
pub(in crate::storage) fn read_original(root: &Path) -> Result<Original> {
    Ok(Receipt::read(root)?.original)
}

/// Only this owner constructs a context, after validating the original plan.
pub(in crate::storage) struct RestoreContext<'a> {
    original: &'a Original,
    root: &'a Path,
}
impl RestoreContext<'_> {
    pub(in crate::storage) fn actor(&self) -> Uuid {
        self.original.actor
    }
    pub(in crate::storage) fn validate_record(
        &self,
        root: &Path,
        envelope: &Envelope,
    ) -> Result<()> {
        ensure!(
            root == self.root && envelope.actor_id == self.original.actor,
            "restored target record differs from its original plan"
        );
        Ok(())
    }
}

// The caller already holds the Store mutex. Reuse the existing validated
// mode decoder without reacquiring that same mutex.
fn mode_pin(store: &Store) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&sync::mode::load(store)?)?))
}
fn metadata(root: &Path) -> Result<Vec<Manifest>> {
    let mut result = BTreeMap::new();
    let mut remaining = MAX_METADATA as u64;
    for (count, entry) in fs::read_dir(root.join("commits"))?.enumerate() {
        ensure!(count < MAX_ITEMS * 2, "restore commit inventory bound");
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with(".stage-"))
        {
            continue;
        }
        let bytes = files::read(&path, remaining)?;
        remaining -= bytes.len() as u64;
        let m: Manifest = serde_json::from_slice(&bytes)?;
        m.validate()?;
        ensure!(
            matches!(m.scope, Scope::LocalTransaction)
                && path.file_name().and_then(|s| s.to_str())
                    == Some(format!("{}.json", m.transaction_id).as_str())
                && result.insert(m.transaction_id, m).is_none(),
            "restore commit identity differs"
        );
    }
    Ok(result.into_values().collect())
}
fn encode_history(manifests: &[Manifest]) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(manifests)?;
    ensure!(
        bytes.len() <= HISTORY_BYTES && manifests.len() <= MAX_ITEMS,
        "restore history bound"
    );
    Ok(bytes)
}
fn config_matches(root: &Path, expected: &Option<String>) -> Result<bool> {
    let path = root.join("restored-config.json");
    let actual = if command::regular_if_present(&path)? {
        Some(digest(&files::read(&path, MAX_RECORD as u64)?))
    } else {
        None
    };
    Ok(&actual == expected)
}
fn history(
    store: &Store,
    inner: &Inner,
    source: &Path,
    backup: &NotebookBackup,
) -> Result<History> {
    let mut result = store.notebook_restore_history(inner)?;
    let context = backup.target_context(source)?;
    for manifest in &backup.manifests {
        result.add_with_archive(source, manifest, Some(store.actor_id), context.as_ref())?;
    }
    result.validate()?;
    ensure!(
        result
            .summary(backup.config_snapshot.is_some())
            .complete_media,
        "incomplete target restore"
    );
    Ok(result)
}

fn pending(paths: &StorePaths) -> Result<Option<PathBuf>> {
    let directory = paths.data.join("generations");
    files::safe_path(&directory)?;
    if !directory.exists() {
        return Ok(None);
    }
    let mut found = None;
    let mut bytes = 0u64;
    for (count, entry) in fs::read_dir(directory)?.enumerate() {
        ensure!(
            count < MAX_ITEMS,
            "target restore generation inventory bound"
        );
        let root = entry?.path();
        let path = root.join(ORIGINAL);
        files::safe_path(&path)?;
        if !path.exists() {
            continue;
        }
        bytes = bytes
            .checked_add(fs::metadata(&path)?.len())
            .context("restore inventory overflow")?;
        ensure!(
            bytes <= MAX_METADATA as u64,
            "restore originals metadata bound"
        );
        let _ = Receipt::read(&root)?;
        if target_generation::restore_completed(&root)? {
            continue;
        }
        ensure!(
            found.is_none(),
            "multiple original target restores require inspection"
        );
        found = Some(root);
    }
    Ok(found)
}
pub(super) fn refuse_pending(paths: &StorePaths) -> Result<()> {
    ensure!(
        pending(paths)?.is_none(),
        "an original target restore requires completion"
    );
    Ok(())
}

impl Store {
    pub(super) fn restore_target_locked(
        &self,
        inner: &mut Inner,
        source: &Path,
        backup: NotebookBackup,
        expected: &Conditions,
        attempted: &mut bool,
    ) -> Result<()> {
        self.notebook_archive_custody(inner)?;
        refuse_pending(&self.paths)?;
        let hash = digest(&files::read(
            &source.join("backup.json"),
            MAX_METADATA as u64,
        )?);
        ensure!(
            self.notebook_restore_plan(inner, source, &backup, hash, expected.target_generation)?
                .conditions
                == *expected,
            "original target restore plan changed"
        );
        let combined = history(self, inner, source, &backup)?;
        let manifests = combined.manifests.values().cloned().collect::<Vec<_>>();
        let raw = encode_history(&manifests)?;
        let root = self.generation(expected.target_generation);
        ensure!(!root.exists(), "target restore generation already exists");
        let base_root = self.generation(inner.generation);
        let flags = conditional::features(&base_root)?;
        let media = manifests
            .iter()
            .flat_map(|m| m.records.iter().chain(&m.media))
            .try_fold(0u64, |n, r| {
                n.checked_add(r.bytes).context("restore work overflow")
            })?
            .checked_mul(32)
            .context("restore work overflow")?;
        let work = (MAX_ITEMS as u64)
            .checked_mul(256)
            .context("restore work overflow")?;
        let metadata = (MAX_METADATA as u64) * 64;
        let remaining = selection::SelectedReadBudget::new(
            0,
            work * 3,
            metadata * 3,
            media.checked_mul(3).context("restore work overflow")?,
        );
        let original = Original {
            format: 1,
            operation: Uuid::new_v5(&expected.target_generation, b"ordinary-target-restore-v1"),
            actor: self.actor_id,
            conditions: expected.clone(),
            mode_sha256: mode_pin(self)?,
            source: source.to_owned(),
            paths_sha256: digest(&serde_json::to_vec(&self.paths)?),
            base: target_generation::archive_origin(&base_root, self.actor_id, inner.generation)?,
            archive: backup.target_feature.clone(),
            flags: (
                flags.0 || backup.feedback_feature,
                flags.1 || backup.selected_feature,
            ),
            history_sha256: digest(&raw),
            work,
            metadata,
            media,
            initial: remaining.checkpoint(),
        };
        original.validate()?;
        fs::create_dir(&root)?;
        let mut receipt = Receipt {
            original,
            remaining,
        };
        files::atomic_json(&root.join(ORIGINAL), &receipt)?;
        files::atomic(&root.join(HISTORY), &raw)?;
        let mut reads = receipt.reserve(&root)?;
        self.resume_target_locked(inner, &receipt.original, &mut reads, attempted)
    }

    fn resume_target_locked(
        &self,
        inner: &mut Inner,
        original: &Original,
        reads: &mut selection::SelectedReadBudget,
        attempted: &mut bool,
    ) -> Result<()> {
        original.validate()?;
        let c = &original.conditions;
        let root = self.generation(c.target_generation);
        ensure!(
            self.actor_id == original.actor
                && digest(&serde_json::to_vec(&self.paths)?) == original.paths_sha256
                && mode_pin(self)? == original.mode_sha256
                && (inner.generation == c.base_generation
                    || inner.generation == c.target_generation),
            "original target restore owner changed"
        );
        // This fixed reservation covers bounded legacy validators that do not
        // accept a caller budget, including source and target index recovery.
        reads.charge(
            0,
            original.work / 2,
            original.metadata / 2,
            original.media / 2,
        )?;
        let current: Uuid = files::json(&self.paths.data.join("CURRENT"), 128)?;
        ensure!(
            current == inner.generation,
            "original restore CURRENT changed"
        );
        Self::trip(inner, Fault::BeforeObjects)?;
        let manifests: Vec<Manifest> = if current == c.base_generation {
            let (backup, hash) = conditional::read_archive(&original.source)?;
            ensure!(
                hash == c.archive_sha256
                    && self
                        .notebook_restore_plan(
                            inner,
                            &original.source,
                            &backup,
                            hash,
                            c.target_generation
                        )?
                        .conditions
                        == *c,
                "original target restore conditions changed"
            );
            let combined = history(self, inner, &original.source, &backup)?;
            let manifests = combined.manifests.values().cloned().collect::<Vec<_>>();
            let raw = encode_history(&manifests)?;
            ensure!(
                digest(&raw) == original.history_sha256,
                "original restore history changed"
            );
            if root.join(HISTORY).exists() {
                ensure!(
                    digest(&files::read(&root.join(HISTORY), HISTORY_BYTES as u64)?)
                        == original.history_sha256,
                    "original staged history changed"
                );
            } else {
                files::atomic(&root.join(HISTORY), &raw)?;
            }
            if root.join("format.json").exists() {
                let family = feedback_generation::version(&root)?;
                ensure!(
                    (1..=4).contains(&family) || family == 6,
                    "original restore staging family changed"
                );
                ensure!(
                    !root.join(feedback_generation::MARKER).exists() || original.flags.0,
                    "unexpected staged feedback feature"
                );
                ensure!(
                    !root.join("selection-feature.json").exists() || original.flags.1,
                    "unexpected staged selection feature"
                );
                selection::validate_generation_preparing_targets(
                    &root,
                    self.actor_id,
                    c.target_generation,
                )?;
            }
            if !root.join("format.json").exists() || feedback_generation::version(&root)? != 6 {
                notebook_generation::create(
                    &self.paths.data,
                    self.actor_id,
                    c.target_generation,
                    original.flags.0,
                    original.flags.1,
                )?;
            }
            if let Some(config) = &backup.config_snapshot {
                let path = root.join("restored-config.json");
                if path.exists() {
                    ensure!(
                        config_matches(&root, &c.staged_configuration_sha256)?,
                        "original staged configuration changed"
                    );
                } else {
                    files::atomic_json(&path, config)?;
                }
            }
            let mut inventory = BTreeMap::new();
            for manifest in &manifests {
                for item in manifest.records.iter().chain(&manifest.media) {
                    if let Some(old) = inventory.insert(item.sha256.clone(), item.clone()) {
                        ensure!(old == *item, "restore object collision");
                    }
                }
            }
            let records = manifests
                .iter()
                .flat_map(|m| &m.records)
                .map(|r| r.sha256.as_str())
                .collect::<BTreeSet<_>>();
            for item in inventory.values() {
                if records.contains(item.sha256.as_str()) {
                    reads.charge(0, 1, item.bytes, 0)?;
                } else {
                    reads.charge(0, 1, 0, item.bytes)?;
                }
                let base = self
                    .generation(c.base_generation)
                    .join("objects")
                    .join(&item.sha256);
                let archive = original.source.join("objects").join(&item.sha256);
                let path = if base.exists() { base } else { archive };
                files::safe_path(&path)?;
                files::copy_verified(
                    &root.join("objects").join(&item.sha256),
                    File::open(path)?,
                    item,
                )?;
            }
            Self::trip(inner, Fault::AfterObjects)?;
            Self::trip(inner, Fault::BeforeCommit)?;
            for manifest in &manifests {
                let path = root
                    .join("commits")
                    .join(format!("{}.json", manifest.transaction_id));
                if path.exists() {
                    ensure!(
                        files::json::<Manifest>(&path, MAX_METADATA as u64)? == *manifest,
                        "restored manifest changed"
                    );
                } else {
                    files::atomic_json(&path, manifest)?;
                }
            }
            target_generation::stage_restore(self, original)?;
            manifests
        } else {
            *attempted = true;
            target_generation::validate_feature(&root, self.actor_id, c.target_generation)?;
            serde_json::from_slice(&files::read(&root.join(HISTORY), HISTORY_BYTES as u64)?)?
        };
        ensure!(
            digest(&encode_history(&manifests)?) == original.history_sha256
                && metadata(&root)? == manifests
                && config_matches(&root, &c.staged_configuration_sha256)?,
            "original staged target closure differs"
        );
        ensure!(
            conditional::features(&root)? == original.flags,
            "original staged features differ"
        );
        selection::refuse_selected_maintenance(&root)?;
        let context = RestoreContext {
            original,
            root: &root,
        };
        let rebuilt = self.recover_target_restore(c.target_generation, Some(&context))?;
        ensure!(
            rebuilt.unavailable == 0
                && rebuilt.manifests.values().cloned().collect::<Vec<_>>() == manifests,
            "original target restore closure unresolved"
        );
        if current == c.base_generation {
            ensure!(
                digest(&files::read(
                    &original.source.join("backup.json"),
                    MAX_METADATA as u64
                )?) == c.archive_sha256
                    && mode_pin(self)? == original.mode_sha256,
                "original restore source changed before activation"
            );
            Self::trip(inner, Fault::BeforeActivation)?;
            *attempted = true;
            files::atomic_json(&self.paths.data.join("CURRENT"), &c.target_generation)?;
            inner.generation = c.target_generation;
        }
        inner.index = rebuilt;
        // Exercise loss after CURRENT but before capabilities through the same
        // existing fault mechanism; only the saved original can repair it.
        Self::trip(inner, Fault::AfterCommit)?;
        target_generation::publish_restore_capabilities(self, original)?;
        target_generation::complete_restore(self, original)?;
        Self::trip(inner, Fault::AfterActivation)?;
        Ok(())
    }
}

pub(super) fn observe(
    store: &Store,
    inner: &Inner,
    expected: &Conditions,
) -> Result<Option<Summary>> {
    let root = store.generation(inner.generation);
    let original = read_original(&root)?;
    if original.conditions != *expected
        || !config_matches(&root, &expected.staged_configuration_sha256)?
    {
        return Ok(None);
    }
    let history = store.notebook_restore_history(inner)?;
    if target_history_hash(
        &history,
        conditional::features(&root)?,
        store.actor_id,
        inner.generation,
        &original.base,
        &original.archive,
    )? != expected.target_history_sha256
    {
        return Ok(None);
    }
    Ok(Some(
        history.summary(expected.staged_configuration_sha256.is_some()),
    ))
}

pub(in crate::storage) fn resume_existing(paths: &StorePaths) -> Result<Option<Store>> {
    let Some(root) = pending(paths)? else {
        return Ok(None);
    };
    let custody = maintenance::ExistingCustody::open(paths.clone(), |_, _| Ok(()), None)?;
    let mut receipt = Receipt::read(&root)?;
    ensure!(
        custody.actor == receipt.original.actor
            && digest(&serde_json::to_vec(&custody.paths)?) == receipt.original.paths_sha256
            && [
                receipt.original.conditions.base_generation,
                receipt.original.conditions.target_generation
            ]
            .contains(&custody.generation),
        "original target restore custody differs"
    );
    let store = Store {
        paths: custody.paths,
        runtime_policy: None,
        actor_id: custody.actor,
        _lease: custody.lease,
        inner: Mutex::new(Inner {
            generation: custody.generation,
            index: Index::default(),
            fault: Fault::None,
            legacy_sync_handles: 0,
            mode_handles: 0,
            legacy_publisher_handles: 0,
            local_domain_handles: 0,
        }),
        wake: Mutex::new(None),
        export_active: std::sync::atomic::AtomicBool::new(false),
        writer_active: std::sync::atomic::AtomicBool::new(false),
    };
    let mut reads = receipt.reserve(&root)?;
    if custody.generation == receipt.original.conditions.base_generation {
        // Compare bounded manifest metadata before reading any changed base media.
        let base_root = store.generation(custody.generation);
        notebook_generation::validate_ready(&store.paths, store.actor_id, custody.generation)?;
        let manifests = metadata(&base_root)?;
        let raw_hash = ordinary_restore::history_hash(
            &manifests
                .into_iter()
                .map(|m| (m.transaction_id, m))
                .collect(),
        )?;
        let origin =
            target_generation::archive_origin(&base_root, store.actor_id, custody.generation)?;
        ensure!(
            conditional::target_base_digest(
                &raw_hash,
                conditional::features(&base_root)?,
                store.actor_id,
                &origin
            )? == receipt.original.conditions.base_history_sha256,
            "original restore base metadata changed"
        );
        store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("restore Store unavailable"))?
            .index = store.recover(custody.generation)?;
    }
    {
        let mut inner = store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("restore Store unavailable"))?;
        store.resume_target_locked(&mut inner, &receipt.original, &mut reads, &mut false)?;
    }
    Ok(Some(store))
}
