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
}
fn data(root: &Path) -> Result<&Path> {
    root.parent()
        .and_then(Path::parent)
        .context("invalid target generation path")
}
fn feature(root: &Path) -> Result<Feature> {
    let value: Feature = files::json(&root.join(MARKER), MAX_RECORD as u64)?;
    value.validate()?;
    let original: Preparation = files::json(&data(root)?.join(PREPARATION), MAX_RECORD as u64)?;
    ensure!(
        original.feature == value,
        "target feature differs from original preparation"
    );
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
    let original: Preparation = files::json(&paths.data.join(PREPARATION), MAX_RECORD as u64)?;
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
    let path = store.paths.data.join(PREPARATION);
    if is_target(&root)? && files::json::<Preparation>(&path, MAX_RECORD as u64)?.complete {
        return validate_ready(&store.paths, store.actor_id, generation);
    }
    let pin = source_pin(store)?;
    let mode_sha = mode_pin(store)?;
    let original = if path.exists() {
        let original: Preparation = files::json(&path, MAX_RECORD as u64)?;
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
        };
        files::atomic_json(&path, &original)?;
        original
    };
    original.feature.validate()?;
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
            let before = files::read(&f.paths().data.join(PREPARATION), MAX_RECORD as u64).unwrap();
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
                files::read(&f.paths().data.join(PREPARATION), MAX_RECORD as u64).unwrap(),
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

impl Store {
    pub(crate) fn open_source_target_startup(config: &crate::config::Config) -> Result<Self> {
        ensure!(
            !config.sync.enabled && config.reader.selection.is_none(),
            "target startup requires ordinary local policy"
        );
        let pin = digest(&serde_json::to_vec(config)?);
        let mut store = Self::open_policy(config.paths.clone(), OpenPreparation::Targets)?;
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
    let path = store.paths.data.join(PREPARATION);
    if !path.exists() {
        return Ok(store
            .paths
            .data
            .join("source-target-startup-policy.json")
            .exists());
    }
    let original: Preparation = files::json(&path, MAX_RECORD as u64)?;
    original.feature.validate()?;
    Ok(!original.complete && original.feature.base_kind == BaseKind::CompoundSelected)
}
