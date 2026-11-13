//! One closed source-target family. Preparation stays under the original Store lease.
use super::*;
use std::sync::atomic::Ordering;
pub(crate) const VERSION: u32 = 6;
pub(crate) const MARKER: &str = "source-target-feature.json";
const PREPARATION: &str = "source-target-preparation.json";
const CONTRACT: &str = include_str!("contract-v5.json");
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BaseKind {
    OrdinaryNotebook,
    CompoundSelected,
}
impl BaseKind {
    fn version(self) -> u32 {
        match self {
            Self::OrdinaryNotebook => notebook_generation::VERSION,
            Self::CompoundSelected => compound_generation::VERSION,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Feature {
    format: u32,
    operation: Uuid,
    actor: Uuid,
    generation: Uuid,
    source_family: u32,
    base_kind: BaseKind,
    source_sha256: String,
    config_sha256: String,
    mode_sha256: String,
    contract_sha256: String,
}
impl Feature {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.format == 1
                && !self.operation.is_nil()
                && !self.actor.is_nil()
                && !self.generation.is_nil()
                && match self.base_kind {
                    BaseKind::OrdinaryNotebook => (1..=4).contains(&self.source_family),
                    BaseKind::CompoundSelected => self.source_family == 5,
                }
                && [&self.source_sha256, &self.config_sha256, &self.mode_sha256]
                    .iter()
                    .all(|s| valid_digest(s))
                && self.contract_sha256 == digest(CONTRACT.as_bytes()),
            "invalid source-target preparation"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Preparation {
    feature: Feature,
    complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_sha256: Option<String>,
}
fn data(root: &Path) -> Result<&Path> {
    root.parent()
        .and_then(Path::parent)
        .context("invalid target generation path")
}
fn read_preparation(path: &Path) -> Result<Option<Preparation>> {
    files::safe_path(path)?;
    if !path.exists() {
        return Ok(None);
    }
    let original: Preparation = files::json(path, MAX_RECORD as u64)?;
    original.feature.validate()?;
    ensure!(
        original
            .restore_sha256
            .as_ref()
            .is_none_or(|sha| valid_digest(sha) && *sha == original.feature.source_sha256),
        "invalid target restore original reference"
    );
    Ok(Some(original))
}
/// New originals belong to their generation. An older global receipt can only
/// authorize its exact generation; it is never overwritten or borrowed by another.
fn preparation(root: &Path) -> Result<Option<Preparation>> {
    let generation: Uuid = root
        .file_name()
        .and_then(|name| name.to_str())
        .context("target generation name absent")?
        .parse()?;
    let local = read_preparation(&root.join(PREPARATION))?;
    let global = read_preparation(&data(root)?.join(PREPARATION))?
        .filter(|original| original.feature.generation == generation);
    if let Some(local) = &local {
        ensure!(
            local.feature.generation == generation,
            "target original belongs to another generation"
        );
        if let Some(global) = &global {
            ensure!(
                local.feature == global.feature
                    && local.restore_sha256 == global.restore_sha256
                    && (local.complete || !global.complete),
                "conflicting local/global target originals"
            );
        }
    }
    Ok(local.or(global))
}
fn feature(root: &Path) -> Result<Feature> {
    let value: Feature = files::json(&root.join(MARKER), MAX_RECORD as u64)?;
    value.validate()?;
    let original = preparation(root)?.context("target original preparation absent")?;
    ensure!(
        original.feature == value,
        "target feature differs from original preparation"
    );
    if let Some(sha) = &original.restore_sha256 {
        let restored = super::recovery::notebook_backup::target_restore::read_original(root)?;
        ensure!(
            restored.sha256()? == *sha && restore_feature(&restored)? == value,
            "target feature differs from original restore"
        );
    }
    ensure!(
        root.file_name().and_then(|p| p.to_str()) == Some(value.generation.to_string().as_str()),
        "target generation identity differs"
    );
    Ok(value)
}
/// Explicit inherited policy, never a guessed ordinary-vs-selected format test.
pub(crate) fn base_version(root: &Path) -> Result<u32> {
    let raw = feedback_generation::version(root)?;
    if raw == VERSION {
        Ok(feature(root)?.base_kind.version())
    } else {
        Ok(raw)
    }
}
pub(crate) fn is_target(root: &Path) -> Result<bool> {
    Ok(feedback_generation::version(root)? == VERSION)
}
pub(crate) fn validate_feature(root: &Path, actor: Uuid, generation: Uuid) -> Result<()> {
    let value = feature(root)?;
    ensure!(
        value.actor == actor && value.generation == generation && is_target(root)?,
        "source-target owner/family differs"
    );
    Ok(())
}
fn capabilities(paths: &StorePaths, actor: Uuid, generation: Uuid) -> Result<serde_json::Value> {
    let root = paths.data.join("generations").join(generation.to_string());
    let value = feature(&root)?;
    let mut result = match value.base_kind {
        BaseKind::OrdinaryNotebook => notebook_generation::capabilities(paths, actor, generation),
        BaseKind::CompoundSelected => compound_generation::capabilities(paths, actor, generation),
    };
    result["contract_version"] = serde_json::json!(5);
    result["schema_file"] = serde_json::json!("contract-v5.json");
    result["store_versions"] = serde_json::json!([1, 2, 3, 4, 5, 6]);
    result["source_target_feature"] = serde_json::json!({"procedure":"same-generation-original-preparation-v1","base_kind":value.base_kind});
    Ok(result)
}
pub(crate) fn validate_ready_root(root: &Path) -> Result<()> {
    let data = data(root)?;
    let cap: serde_json::Value = files::json(&data.join("capabilities.json"), MAX_RECORD as u64)?;
    let paths: StorePaths = serde_json::from_value(cap["paths"].clone())?;
    ensure!(paths.data == data, "target capabilities data root differs");
    let value = feature(root)?;
    validate_ready(&paths, value.actor, value.generation)
}
pub(crate) fn validate_ready(paths: &StorePaths, actor: Uuid, generation: Uuid) -> Result<()> {
    let root = paths.data.join("generations").join(generation.to_string());
    let original = preparation(&root)?.context("target original preparation absent")?;
    ensure!(
        original.complete,
        "source-target preparation has not completed"
    );
    validate_ready_contents(paths, actor, generation)
}
fn validate_ready_contents(paths: &StorePaths, actor: Uuid, generation: Uuid) -> Result<()> {
    ensure!(
        files::json::<Uuid>(&paths.data.join("CURRENT"), 128)? == generation,
        "target CURRENT changed"
    );
    let root = paths.data.join("generations").join(generation.to_string());
    validate_feature(&root, actor, generation)?;
    // These inherited validators read base-kind policy, without overwriting receipts.
    selection::validate_generation(&root, actor, generation)?;
    ensure!(
        files::read(&paths.data.join("contract-v5.json"), MAX_RECORD as u64)?
            == CONTRACT.as_bytes(),
        "target contract differs"
    );
    ensure!(
        files::read(&paths.data.join("capabilities.json"), MAX_RECORD as u64)?
            == serde_json::to_vec(&capabilities(paths, actor, generation)?)?,
        "incomplete target capabilities"
    );
    Ok(())
}
/// Detached archive provenance carries historical feature identity, never its
/// source generation's live owner files or a startup/model grant.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchiveOrigin {
    format: u32,
    feature: Feature,
}
impl ArchiveOrigin {
    pub(super) fn validate_actor(&self, actor: Uuid) -> Result<()> {
        self.validate(actor, self.feature.generation)
    }
    pub(super) fn validate(&self, actor: Uuid, generation: Uuid) -> Result<()> {
        self.feature.validate()?;
        ensure!(
            self.format == 1
                && self.feature.actor == actor
                && self.feature.generation == generation
                && self.feature.base_kind == BaseKind::OrdinaryNotebook,
            "ordinary target archive origin differs"
        );
        Ok(())
    }
}
pub(super) fn archive_origin(
    root: &Path,
    actor: Uuid,
    generation: Uuid,
) -> Result<Option<ArchiveOrigin>> {
    if !is_target(root)? {
        return Ok(None);
    }
    validate_ready_root(root)?;
    let origin = ArchiveOrigin {
        format: 1,
        feature: feature(root)?,
    };
    origin.validate(actor, generation)?;
    Ok(Some(origin))
}

fn mode_pin(store: &Store) -> Result<String> {
    let snapshot = store.sync_mode_snapshot()?;
    Ok(digest(&serde_json::to_vec(&(
        snapshot.device,
        snapshot.epoch,
        snapshot.mode,
    ))?))
}
pub(crate) fn source_pin(store: &Store) -> Result<String> {
    let inner = store
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("target preparation mutex unavailable"))?;
    let mut items = Vec::new();
    let mut bytes = 0u64;
    for (hash, manifest) in &inner.index.manifests {
        let encoded = serde_json::to_vec(manifest)?;
        bytes = bytes
            .checked_add(encoded.len() as u64)
            .context("target preparation byte overflow")?;
        ensure!(
            bytes <= MAX_METADATA as u64,
            "target preparation metadata bound"
        );
        items.push((hash, digest(&encoded)));
    }
    ensure!(
        items.len() <= MAX_ITEMS,
        "target preparation manifest bound"
    );
    // Include every original selected transaction and checkpoint byte. Interrupted
    // promotion must not bless a changed accepted document or absence witness.
    let mut selected = Vec::new();
    let root = store.generation(inner.generation);
    for dir in [root.join("selections"), root.join("selection-history")] {
        let mut pending = vec![dir];
        while let Some(path) = pending.pop() {
            files::safe_path(&path)?;
            if !path.exists() {
                continue;
            }
            if path.is_dir() {
                for entry in fs::read_dir(&path)? {
                    pending.push(entry?.path());
                }
                ensure!(
                    pending.len() + selected.len() <= MAX_ITEMS,
                    "target selected source work bound"
                );
            } else {
                let encoded = files::read(&path, MAX_METADATA as u64)?;
                bytes = bytes
                    .checked_add(encoded.len() as u64)
                    .context("target selected pin overflow")?;
                ensure!(
                    bytes <= MAX_METADATA as u64,
                    "target selected source byte bound"
                );
                selected.push((path.strip_prefix(&root)?.to_path_buf(), digest(&encoded)));
            }
        }
    }
    let checkpoint = store.paths.data.join("sync/state.json");
    let checkpoint = if checkpoint.exists() {
        let encoded = files::read(&checkpoint, MAX_METADATA as u64)?;
        bytes = bytes
            .checked_add(encoded.len() as u64)
            .context("target checkpoint pin overflow")?;
        ensure!(bytes <= MAX_METADATA as u64, "target checkpoint pin bound");
        Some(digest(&encoded))
    } else {
        None
    };
    selected.sort();
    Ok(digest(&serde_json::to_vec(&(items, selected, checkpoint))?))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Policy,
    Original,
    Notebook,
    Marker,
    Contract,
    Format,
    Capabilities,
}
pub(crate) fn prepare(
    store: &mut Store,
    config_sha256: &str,
    base_kind: BaseKind,
    hook: impl FnMut(Stage) -> Result<()>,
) -> Result<()> {
    prepare_original(store, config_sha256, base_kind, None, hook)
}
pub(crate) fn prepare_selected_original(
    store: &mut Store,
    pin: &str,
    operation: Uuid,
    hook: impl FnMut(Stage) -> Result<()>,
) -> Result<()> {
    ensure!(!operation.is_nil(), "original target operation absent");
    prepare_original(
        store,
        pin,
        BaseKind::CompoundSelected,
        Some(operation),
        hook,
    )
}
fn prepare_original(
    store: &mut Store,
    config_sha256: &str,
    base_kind: BaseKind,
    operation: Option<Uuid>,
    mut hook: impl FnMut(Stage) -> Result<()>,
) -> Result<()> {
    ensure!(valid_digest(config_sha256), "invalid startup Config pin");
    let generation = {
        let inner = store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("target preparation mutex unavailable"))?;
        ensure!(
            inner.index.unavailable == 0
                && inner.mode_handles == 0
                && inner.local_domain_handles == 0
                && inner.legacy_sync_handles == 0
                && inner.legacy_publisher_handles == 0
                && !store.writer_active.load(Ordering::SeqCst)
                && !store.export_active.load(Ordering::SeqCst),
            "source-target preparation requires unique settled custody"
        );
        inner.generation
    };
    let root = store.generation(generation);
    let path = root.join(PREPARATION);
    let existing = preparation(&root)?;
    if is_target(&root)? && existing.as_ref().is_some_and(|original| original.complete) {
        return validate_ready(&store.paths, store.actor_id, generation);
    }
    let pin = source_pin(store)?;
    let mode_sha = mode_pin(store)?;
    let original = if let Some(original) = existing {
        ensure!(
            !original.complete
                && original.feature.actor == store.actor_id
                && original.feature.generation == generation
                && original.feature.config_sha256 == config_sha256
                && original.feature.mode_sha256 == mode_sha
                && original.feature.source_sha256 == pin
                && original.feature.base_kind == base_kind
                && operation.is_none_or(|id| original.feature.operation == id),
            "original target preparation changed"
        );
        original
    } else {
        let source_family = feedback_generation::version(&root)?;
        if source_family == 4 {
            notebook_generation::validate_ready(&store.paths, store.actor_id, generation)?;
        }
        if source_family == 5 {
            compound_generation::validate_ready(&store.paths, store.actor_id, generation)?;
        }
        ensure!(
            match base_kind {
                BaseKind::OrdinaryNotebook =>
                    (1..=4).contains(&source_family) && sync::mode::ordinary_local(store)?,
                BaseKind::CompoundSelected =>
                    source_family == 5
                        && matches!(
                            store.sync_mode_snapshot()?.mode,
                            Some(sync::mode::SyncMode::SharedGroup { .. })
                        ),
            },
            "target preparation mode/base family differs"
        );
        let original = Preparation {
            feature: Feature {
                format: 1,
                operation: operation.unwrap_or_else(Uuid::new_v4),
                actor: store.actor_id,
                generation,
                source_family,
                base_kind,
                source_sha256: pin,
                config_sha256: config_sha256.into(),
                mode_sha256: mode_sha,
                contract_sha256: digest(CONTRACT.as_bytes()),
            },
            complete: false,
            restore_sha256: None,
        };
        files::atomic_json(&path, &original)?;
        original
    };
    original.feature.validate()?;
    if !path.exists() {
        // Compatibility recovery carries this SAME original forward locally.
        // The existing global receipt remains byte-for-byte untouched.
        files::atomic_json(&path, &original)?;
    }
    hook(Stage::Original)?;
    if base_kind == BaseKind::OrdinaryNotebook
        && feedback_generation::version(&root)? < notebook_generation::VERSION
    {
        notebook_generation::prepare(store, |_| Ok(()))?;
    }
    hook(Stage::Notebook)?;
    if !root.join(MARKER).exists() {
        files::atomic_json(&root.join(MARKER), &original.feature)?;
    }
    ensure!(
        files::json::<Feature>(&root.join(MARKER), MAX_RECORD as u64)? == original.feature,
        "target marker original differs"
    );
    hook(Stage::Marker)?;
    let contract = store.paths.data.join("contract-v5.json");
    if !contract.exists() {
        files::atomic(&contract, CONTRACT.as_bytes())?;
    }
    ensure!(
        files::read(&contract, MAX_RECORD as u64)? == CONTRACT.as_bytes(),
        "target contract changed"
    );
    hook(Stage::Contract)?;
    if !is_target(&root)? {
        files::atomic_json(&root.join("format.json"), &VERSION)?;
    }
    hook(Stage::Format)?;
    files::atomic_json(
        &store.paths.data.join("capabilities.json"),
        &capabilities(&store.paths, store.actor_id, generation)?,
    )?;
    validate_ready_contents(&store.paths, store.actor_id, generation)?;
    hook(Stage::Capabilities)?;
    files::atomic_json(
        &path,
        &Preparation {
            complete: true,
            ..original
        },
    )?;
    Ok(())
}

impl Store {
    pub(crate) fn open_source_target_startup(config: &crate::config::Config) -> Result<Self> {
        ensure!(
            !config.sync.enabled && config.reader.selection.is_none(),
            "target startup requires ordinary local policy"
        );
        let mut pin = digest(&serde_json::to_vec(&(
            config,
            config.source_snapshot.as_ref().map(|s| s.fingerprint()),
        ))?);
        let mut store =
            match super::recovery::notebook_backup::target_restore::resume_existing(&config.paths)?
            {
                Some(store) => store,
                None => Self::open_policy(config.paths.clone(), OpenPreparation::Targets)?,
            }
            .with_runtime_policy(config);
        let generation = store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("target Store unavailable"))?
            .generation;
        let root = store.generation(generation);
        // The delivered global original predates raw loaded-file provenance.
        // Accept its exact typed Config hash only for that known original; a new
        // generation-local intent never gets this compatibility fallback.
        if let Some(global) = read_preparation(&store.paths.data.join(PREPARATION))? {
            let legacy_pin = digest(&serde_json::to_vec(config)?);
            if global.feature.generation == generation && global.feature.config_sha256 == legacy_pin
            {
                ensure!(
                    preparation(&root)?.is_some_and(|original| original.feature == global.feature),
                    "legacy target original differs"
                );
                pin = legacy_pin;
            }
        }
        let mut reads = selection::SelectedReadBudget::new(0, 2, MAX_RECORD as u64 * 2, 0);
        store.check_runtime_policy(&mut reads)?;
        prepare(&mut store, &pin, BaseKind::OrdinaryNotebook, |_| Ok(()))?;
        Ok(store)
    }
}

/// Exact target closure over committed facts or this publication. No extra object
/// lookup, current-head substitution, allocation of budget or provider action.
pub(super) fn validate_records(
    index: &Index,
    records: &[(Envelope, String)],
    references: &[ObjectRef],
) -> Result<()> {
    use crate::conversation::{Ledger, Record};
    if !records
        .iter()
        .any(|(e, _)| e.domain_schema_version == crate::conversation::target::SCHEMA)
    {
        return Ok(());
    }
    let mut all = BTreeMap::new();
    for (e, hash) in index.records.values().chain(records.iter()) {
        if let Some((old, old_hash)) = all.insert(e.revision_id, (e, hash)) {
            ensure!(
                old == e && old_hash == hash,
                "target closure revision collision"
            );
        }
    }
    let exact = |reference: &crate::conversation::target::RecordRef| -> Result<Record> {
        let (e, hash) = all
            .get(&reference.revision)
            .context("target committed revision absent")?;
        let object = references
            .iter()
            .find(|r| r.sha256 == **hash)
            .or_else(|| index.record_refs.get(*hash))
            .context("target exact object descriptor absent")?;
        ensure!(
            e.namespace == reference.namespace
                && e.record_id == reference.record
                && *object == reference.object,
            "target committed reference differs"
        );
        Ledger::decode(e)
    };
    let turn = |id: Uuid| -> Result<crate::conversation::Turn> {
        let candidates = all
            .values()
            .filter(|(e, _)| e.namespace == Namespace::Conversation && e.record_id == id)
            .collect::<Vec<_>>();
        let mut heads = candidates
            .iter()
            .map(|(e, _)| e.revision_id)
            .collect::<BTreeSet<_>>();
        for (e, _) in &candidates {
            for parent in &e.parents {
                heads.remove(parent);
            }
        }
        ensure!(heads.len() == 1, "target user head absent or conflicted");
        let (e, _) = all.get(heads.first().unwrap()).unwrap();
        let Record::Turn(turn) = Ledger::decode(e)? else {
            anyhow::bail!("target user is not a live turn")
        };
        Ok(turn)
    };
    for (e, _) in records
        .iter()
        .filter(|(e, _)| e.domain_schema_version == crate::conversation::target::SCHEMA)
    {
        crate::conversation::target::validate_links(&Ledger::decode(e)?, &exact, &turn)?;
    }
    Ok(())
}
impl Store {
    pub(crate) fn source_target_generation(&self) -> Result<Option<Uuid>> {
        let generation = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("target Store mutex unavailable"))?
            .generation;
        if is_target(&self.generation(generation))? {
            Ok(Some(generation))
        } else {
            Ok(None)
        }
    }
}

impl Store {
    /// Existing selected generations only. The caller must qualify policy and
    /// transfer its original witness before calling selected promotion.
    #[cfg(target_os = "linux")]
    pub(crate) fn open_selected_target_preparation(config: &crate::config::Config) -> Result<Self> {
        ensure!(
            !config.sync.enabled
                && config.reader.selection.is_some()
                && config.paths.data.join("identity.json").exists(),
            "selected target startup requires existing selected policy"
        );
        let mut store = Self::open_policy(config.paths.clone(), OpenPreparation::Targets)?;
        crate::storage::sync::selected::ReaderPreferences::resume_target_preparation(
            &mut store, config,
        )?;
        let generation = store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("selected target Store unavailable"))?
            .generation;
        ensure!(
            base_version(&store.generation(generation))? == 5,
            "selected target startup requires compound base"
        );
        if !is_target(&store.generation(generation))? {
            compound_generation::validate_ready(&store.paths, store.actor_id, generation)?;
        } else {
            validate_ready(&store.paths, store.actor_id, generation)?;
        }
        Ok(store)
    }
}

pub(crate) fn selected_preparation_pending(store: &Store) -> Result<bool> {
    let generation = store
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("target Store unavailable"))?
        .generation;
    let Some(original) = preparation(&store.generation(generation))? else {
        return Ok(store
            .paths
            .data
            .join("source-target-startup-policy.json")
            .exists());
    };
    Ok(!original.complete && original.feature.base_kind == BaseKind::CompoundSelected)
}

impl Store {
    /// One caller-owned allowance covers every repeated policy/header check.
    /// Ledger updates do not change these immutable ready-generation facts.
    pub(crate) fn target_runtime_pin(
        &self,
        reads: &mut selection::SelectedReadBudget,
    ) -> Result<Option<String>> {
        let generation = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("target runtime Store unavailable"))?
            .generation;
        let root = self.generation(generation);
        reads.charge(0, 1, 0, 0)?;
        let format = files::read_with_budget(&root.join("format.json"), 64, Some(reads))?;
        if serde_json::from_slice::<u32>(&format)? != VERSION {
            return Ok(None);
        }
        let mut pins = Vec::new();
        for path in [
            self.paths.data.join("identity.json"),
            self.paths.data.join("CURRENT"),
            root.join("format.json"),
            root.join(MARKER),
            self.paths.data.join("capabilities.json"),
            self.paths.data.join("contract-v5.json"),
        ] {
            reads.charge(0, 1, 0, 0)?;
            let bytes = files::read_with_budget(&path, MAX_RECORD as u64, Some(reads))?;
            pins.push(Some(digest(&bytes)));
        }
        // Presence is pinned too: adding/removing a competing original cannot
        // evade a running request's immutable ready-generation correspondence.
        for path in [
            root.join(PREPARATION),
            self.paths.data.join(PREPARATION),
            root.join(super::recovery::notebook_backup::target_restore::ORIGINAL),
        ] {
            reads.charge(0, 1, 0, 0)?;
            files::safe_path(&path)?;
            pins.push(if path.exists() {
                Some(digest(&files::read_with_budget(
                    &path,
                    MAX_RECORD as u64,
                    Some(reads),
                )?))
            } else {
                None
            });
        }
        Ok(Some(digest(&serde_json::to_vec(&pins)?)))
    }
}

fn restore_feature(
    original: &super::recovery::notebook_backup::target_restore::Original,
) -> Result<Feature> {
    original.validate()?;
    Ok(Feature {
        format: 1,
        operation: original.operation,
        actor: original.actor,
        generation: original.conditions.target_generation,
        source_family: 4,
        base_kind: BaseKind::OrdinaryNotebook,
        source_sha256: original.sha256()?,
        config_sha256: original
            .conditions
            .staged_configuration_sha256
            .clone()
            .unwrap_or_else(|| digest(b"null")),
        mode_sha256: original.mode_sha256.clone(),
        contract_sha256: digest(CONTRACT.as_bytes()),
    })
}
pub(super) fn stage_restore(
    store: &Store,
    original: &super::recovery::notebook_backup::target_restore::Original,
) -> Result<()> {
    let value = restore_feature(original)?;
    let root = store.generation(value.generation);
    ensure!(
        store.actor_id == value.actor,
        "restore feature actor differs"
    );
    let path = root.join(PREPARATION);
    let intended = Preparation {
        feature: value.clone(),
        complete: false,
        restore_sha256: Some(original.sha256()?),
    };
    if let Some(existing) = read_preparation(&path)? {
        ensure!(
            existing.feature == value && existing.restore_sha256 == intended.restore_sha256,
            "original restore feature changed"
        );
    } else {
        files::atomic_json(&path, &intended)?;
    }
    if root.join(MARKER).exists() {
        ensure!(
            files::json::<Feature>(&root.join(MARKER), MAX_RECORD as u64)? == value,
            "restore marker differs"
        );
    } else {
        files::atomic_json(&root.join(MARKER), &value)?;
    }
    let contract = store.paths.data.join("contract-v5.json");
    if !contract.exists() {
        files::atomic(&contract, CONTRACT.as_bytes())?;
    }
    ensure!(
        files::read(&contract, MAX_RECORD as u64)? == CONTRACT.as_bytes(),
        "restore contract differs"
    );
    let family = feedback_generation::version(&root)?;
    ensure!(
        family == 4 || family == VERSION,
        "restore staging family differs"
    );
    if family != VERSION {
        files::atomic_json(&root.join("format.json"), &VERSION)?;
    }
    validate_feature(&root, value.actor, value.generation)
}
pub(super) fn publish_restore_capabilities(
    store: &Store,
    original: &super::recovery::notebook_backup::target_restore::Original,
) -> Result<()> {
    let generation = original.conditions.target_generation;
    ensure!(
        feature(&store.generation(generation))? == restore_feature(original)?,
        "original restore feature differs"
    );
    files::atomic_json(
        &store.paths.data.join("capabilities.json"),
        &capabilities(&store.paths, store.actor_id, generation)?,
    )?;
    validate_ready_contents(&store.paths, store.actor_id, generation)
}
pub(super) fn complete_restore(
    store: &Store,
    original: &super::recovery::notebook_backup::target_restore::Original,
) -> Result<()> {
    let generation = original.conditions.target_generation;
    validate_ready_contents(&store.paths, store.actor_id, generation)?;
    files::atomic_json(
        &store.generation(generation).join(PREPARATION),
        &Preparation {
            feature: restore_feature(original)?,
            complete: true,
            restore_sha256: Some(original.sha256()?),
        },
    )?;
    validate_ready(&store.paths, store.actor_id, generation)
}
pub(super) fn restore_completed(root: &Path) -> Result<bool> {
    Ok(preparation(root)?.is_some_and(|p| p.complete && p.restore_sha256.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("target-family-{}", Uuid::new_v4())))
        }
        fn paths(&self) -> StorePaths {
            StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn ordinary_preparation_recovers_every_original_boundary_without_relocking() {
        for stage in [
            Stage::Original,
            Stage::Notebook,
            Stage::Marker,
            Stage::Contract,
            Stage::Format,
            Stage::Capabilities,
        ] {
            let f = Fixture::new();
            let store = Store::open(f.paths()).unwrap();
            let actor = store.actor_id;
            let generation = store.inner.lock().unwrap().generation;
            drop(store);
            let pin = digest(b"same config");
            let mut store = Store::open_policy(f.paths(), OpenPreparation::Targets).unwrap();
            assert!(prepare(&mut store, &pin, BaseKind::OrdinaryNotebook, |at| {
                if at == stage {
                    anyhow::bail!("interrupted")
                }
                Ok(())
            })
            .is_err());
            let original_path = store.generation(generation).join(PREPARATION);
            let before = files::read(&original_path, MAX_RECORD as u64).unwrap();
            assert!(!f.paths().data.join(PREPARATION).exists());
            drop(store);
            let mut store = Store::open_policy(f.paths(), OpenPreparation::Targets).unwrap();
            assert!(prepare(
                &mut store,
                &digest(b"changed config"),
                BaseKind::OrdinaryNotebook,
                |_| Ok(())
            )
            .is_err());
            assert_eq!(
                files::read(&original_path, MAX_RECORD as u64).unwrap(),
                before
            );
            prepare(&mut store, &pin, BaseKind::OrdinaryNotebook, |_| Ok(())).unwrap();
            assert_eq!(store.actor_id, actor);
            assert_eq!(store.inner.lock().unwrap().generation, generation);
            assert!(store.ordinary_reader_family().unwrap());
            drop(store);
            let store = Store::open(f.paths()).unwrap();
            validate_ready(&store.paths, actor, generation).unwrap();
            assert_eq!(
                feedback_generation::version(&store.generation(generation)).unwrap(),
                6
            );
        }
    }
    #[test]
    fn target_global_original_recovers_locally_without_overwrite_or_replacement() {
        for stage in [
            Stage::Original,
            Stage::Notebook,
            Stage::Marker,
            Stage::Contract,
            Stage::Format,
            Stage::Capabilities,
        ] {
            let f = Fixture::new();
            let config = crate::config::Config {
                paths: f.paths(),
                ..Default::default()
            };
            let mut store = Store::open_policy(f.paths(), OpenPreparation::Targets).unwrap();
            let generation = store.inner.lock().unwrap().generation;
            let root = store.generation(generation);
            let legacy_pin = digest(&serde_json::to_vec(&config).unwrap());
            assert!(
                prepare(&mut store, &legacy_pin, BaseKind::OrdinaryNotebook, |at| {
                    ensure!(at != stage, "controlled legacy interruption");
                    Ok(())
                })
                .is_err()
            );
            let local = root.join(PREPARATION);
            let global = f.paths().data.join(PREPARATION);
            fs::rename(&local, &global).unwrap();
            let before = files::read(&global, MAX_RECORD as u64).unwrap();
            let original: Preparation = serde_json::from_slice(&before).unwrap();
            drop(store);
            let mut changed = config.clone();
            changed.model = Some("changed-model".into());
            assert!(Store::open_source_target_startup(&changed).is_err());
            assert!(!local.exists());
            assert_eq!(files::read(&global, MAX_RECORD as u64).unwrap(), before);
            let store = Store::open_source_target_startup(&config).unwrap();
            validate_ready(&store.paths, store.actor_id, generation).unwrap();
            assert_eq!(files::read(&global, MAX_RECORD as u64).unwrap(), before);
            let recovered: Preparation = files::json(&local, MAX_RECORD as u64).unwrap();
            assert!(
                recovered.complete && !original.complete && recovered.feature == original.feature
            );
            assert_eq!(store.inner.lock().unwrap().generation, generation);
            drop(store);
            let _ = Store::open(f.paths()).unwrap();
        }
    }

    #[test]
    fn target_ready_global_compatibility_is_validate_only_and_conflicting_originals_refuse() {
        let f = Fixture::new();
        let config = crate::config::Config {
            paths: f.paths(),
            ..Default::default()
        };
        let store = Store::open_source_target_startup(&config).unwrap();
        let generation = store.inner.lock().unwrap().generation;
        let root = store.generation(generation);
        let local = root.join(PREPARATION);
        let global = f.paths().data.join(PREPARATION);
        let original_bytes = files::read(&local, MAX_RECORD as u64).unwrap();
        fs::rename(&local, &global).unwrap();
        drop(store);
        let ready = Store::open_source_target_startup(&config).unwrap();
        assert!(
            !local.exists(),
            "ready compatibility open must not migrate metadata"
        );
        assert_eq!(
            files::read(&global, MAX_RECORD as u64).unwrap(),
            original_bytes
        );
        drop(ready);
        files::atomic(&local, &original_bytes).unwrap();
        let original: Preparation = serde_json::from_slice(&original_bytes).unwrap();
        for field in [
            "operation",
            "actor",
            "source_sha256",
            "config_sha256",
            "mode_sha256",
        ] {
            let mut conflict = serde_json::to_value(&original).unwrap();
            conflict["feature"][field] = if field == "operation" || field == "actor" {
                serde_json::json!(Uuid::new_v4())
            } else {
                serde_json::json!(digest(b"conflicting original"))
            };
            files::atomic_json(&global, &conflict).unwrap();
            let conflicting_bytes = files::read(&global, MAX_RECORD as u64).unwrap();
            assert!(
                Store::open(f.paths()).is_err(),
                "conflicting {field} must refuse"
            );
            assert_eq!(
                files::read(&global, MAX_RECORD as u64).unwrap(),
                conflicting_bytes
            );
            assert_eq!(
                files::read(&local, MAX_RECORD as u64).unwrap(),
                original_bytes
            );
        }
        files::atomic(&global, &original_bytes).unwrap();
        let _ = Store::open(f.paths()).unwrap();
    }

    #[test]
    fn target_global_original_cannot_authorize_a_different_generation() {
        let f = Fixture::new();
        let config = crate::config::Config {
            paths: f.paths(),
            ..Default::default()
        };
        let store = Store::open_source_target_startup(&config).unwrap();
        let generation = store.inner.lock().unwrap().generation;
        let original: Preparation = files::json(
            &store.generation(generation).join(PREPARATION),
            MAX_RECORD as u64,
        )
        .unwrap();
        let global = f.paths().data.join(PREPARATION);
        files::atomic_json(&global, &original).unwrap();
        let before = files::read(&global, MAX_RECORD as u64).unwrap();
        let next = Uuid::new_v4();
        let next_root = store.generation(next);
        Store::create_generation(&f.paths().data, next).unwrap();
        assert!(preparation(&next_root).unwrap().is_none());
        let mut next_original = original.clone();
        next_original.feature.generation = next;
        next_original.feature.operation = Uuid::new_v4();
        files::atomic_json(&next_root.join(PREPARATION), &next_original).unwrap();
        assert!(preparation(&next_root).unwrap().unwrap().feature == next_original.feature);
        assert_eq!(files::read(&global, MAX_RECORD as u64).unwrap(), before);
    }

    #[test]
    fn unprepared_domain_four_refuses_before_object_publication() {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        let generation = store.inner.lock().unwrap().generation;
        let root = store.generation(generation);
        let before = fs::read_dir(root.join("objects")).unwrap().count();
        let envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::Source,
            domain_schema_version: 4,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::json!({"record_kind":"source-target","record":{}}),
            media_descriptors: vec![],
        };
        envelope.validate().unwrap();
        assert!(store.commit(vec![envelope], BTreeMap::new()).is_err());
        assert_eq!(fs::read_dir(root.join("objects")).unwrap().count(), before);
    }
}
