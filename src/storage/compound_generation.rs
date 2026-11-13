//! Explicit compound notebook/selection preparation under existing-only custody.
//! An original actor/source/target operation is the sole capability-repair path.
use super::*;

pub(crate) const VERSION: u32 = selection::COMPOUND_STORE_FORMAT;
pub(crate) const MARKER: &str = "compound-feature.json";
const CONTRACT: &str = include_str!("contract-v4.json");
const PREPARATION: &str = "sync/compound-preparation.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeEvidence {
    pub actor: Uuid,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation: Uuid,
    pub actor: Uuid,
    pub source_generation: Uuid,
    pub group: Uuid,
    pub binding_sha256: String,
    pub expected_mode: ModeEvidence,
}
impl Request {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.operation.is_nil()
                && !self.actor.is_nil()
                && !self.source_generation.is_nil()
                && !self.group.is_nil()
                && valid_digest(&self.binding_sha256),
            "invalid compound preparation identity"
        );
        ensure!(
            self.expected_mode.actor == self.actor && valid_digest(&self.expected_mode.sha256),
            "invalid compound preparation mode evidence"
        );
        Ok(())
    }
    fn current(&self, store: &Store) -> Result<()> {
        self.validate()?;
        ensure!(
            store.actor_id == self.actor,
            "compound preparation actor changed"
        );
        let expected = sync::mode::SyncModeToken::from_maintenance_request(
            self.expected_mode.actor,
            self.expected_mode.sha256.clone(),
        )?;
        let snapshot = store.sync_mode_snapshot()?;
        ensure!(
            snapshot.token == expected
                && matches!(snapshot.mode, None | Some(sync::mode::SyncMode::LocalOnly)),
            "compound preparation requires the original ordinary mode"
        );
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    format: u32,
    request: Request,
    target_generation: Uuid,
    source_sha256: String,
    contract_sha256: String,
    feedback_feature: bool,
}
impl Preparation {
    fn validate(&self) -> Result<()> {
        self.request.validate()?;
        ensure!(
            self.format == 1
                && !self.target_generation.is_nil()
                && self.target_generation != self.request.source_generation
                && valid_digest(&self.source_sha256)
                && self.contract_sha256 == digest(CONTRACT.as_bytes()),
            "unsupported compound preparation"
        );
        Ok(())
    }
}

pub(crate) fn validate_feature(root: &Path, actor: Uuid, generation: Uuid) -> Result<()> {
    let feature: Preparation = files::json(&root.join(MARKER), MAX_RECORD as u64)?;
    feature.validate()?;
    ensure!(
        feature.request.actor == actor && feature.target_generation == generation,
        "compound generation owner differs"
    );
    let data = root
        .parent()
        .and_then(Path::parent)
        .context("invalid compound generation root")?;
    ensure!(
        files::json::<Preparation>(&data.join(PREPARATION), MAX_RECORD as u64)? == feature,
        "compound feature differs from its original preparation"
    );
    ensure!(
        target_generation::base_version(root)? == VERSION,
        "compound generation format differs"
    );
    ensure!(
        root.join(feedback_generation::MARKER).exists() == feature.feedback_feature,
        "compound feedback feature differs from original preparation"
    );
    Ok(())
}

pub(crate) fn validate_portable_generation(root: &Path) -> Result<()> {
    let data = root
        .parent()
        .and_then(Path::parent)
        .context("invalid compound generation root")?;
    let identity: Identity = files::json(&data.join("identity.json"), MAX_RECORD as u64)?;
    ensure!(identity.format == FORMAT, "compound owner identity differs");
    let generation = Uuid::parse_str(
        root.file_name()
            .and_then(|n| n.to_str())
            .context("invalid compound generation name")?,
    )?;
    validate_feature(root, identity.actor, generation)
}

pub(super) fn validate_owned_staging(root: &Path, actor: Uuid, generation: Uuid) -> Result<()> {
    let data = root
        .parent()
        .and_then(Path::parent)
        .context("invalid compound staging path")?;
    let original: Preparation = files::json(&data.join(PREPARATION), MAX_RECORD as u64)?;
    original.validate()?;
    ensure!(
        original.request.actor == actor
            && original.target_generation == generation
            && root.file_name().and_then(|n| n.to_str()) == Some(generation.to_string().as_str())
            && files::json::<Preparation>(&root.join("compound-staging.json"), MAX_RECORD as u64)?
                == original,
        "compound staging is not owned by this original preparation"
    );
    ensure!(
        files::json::<Uuid>(&data.join("CURRENT"), 128)? == original.request.source_generation,
        "compound staging cannot repair a published generation"
    );
    Ok(())
}

pub(crate) fn capabilities(paths: &StorePaths, actor: Uuid, generation: Uuid) -> serde_json::Value {
    let mut result = notebook_generation::capabilities(paths, actor, generation);
    result["contract_version"] = serde_json::json!(4);
    result["schema_file"] = serde_json::json!("contract-v4.json");
    result["store_versions"] = serde_json::json!([1, 2, 3, 4, 5]);
    result["compound_feature"] = serde_json::json!("actor-source-target-operation-bound-v1");
    result["sync"] = serde_json::json!("qualified-selected-fixed-slot-v1");
    result
}
pub(crate) fn validate_ready(paths: &StorePaths, actor: Uuid, generation: Uuid) -> Result<()> {
    let target_root = paths.data.join("generations").join(generation.to_string());
    if target_generation::is_target(&target_root)? {
        return target_generation::validate_ready(paths, actor, generation);
    }
    ensure!(
        files::json::<Uuid>(&paths.data.join("CURRENT"), 128)? == generation,
        "compound current changed"
    );
    let root = paths.data.join("generations").join(generation.to_string());
    validate_feature(&root, actor, generation)?;
    selection::validate_generation(&root, actor, generation)?;
    ensure!(
        files::read(&paths.data.join("contract-v4.json"), MAX_RECORD as u64)?
            == CONTRACT.as_bytes(),
        "compound contract differs"
    );
    ensure!(
        files::read(&paths.data.join("capabilities.json"), MAX_RECORD as u64)?
            == serde_json::to_vec(&capabilities(paths, actor, generation))?,
        "incomplete compound capabilities"
    );
    Ok(())
}
fn publish_capabilities(store: &Store, generation: Uuid) -> Result<()> {
    files::atomic(
        &store.paths.data.join("contract-v4.json"),
        CONTRACT.as_bytes(),
    )?;
    files::atomic_json(
        &store.paths.data.join("capabilities.json"),
        &capabilities(&store.paths, store.actor_id, generation),
    )?;
    validate_ready(&store.paths, store.actor_id, generation)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    BeforeTarget,
    AfterMarkers,
    AfterObjects,
    AfterContract,
    BeforeCurrent,
    AfterCurrent,
    AfterCapabilities,
}

/// The ordinary source remains intact. Live selected metadata cannot be silently
/// re-rooted: it needs its separately admitted receiving-local genesis bridge.
struct SourceSnapshot {
    sha256: String,
    manifests: Vec<(Manifest, Vec<u8>)>,
    feedback_feature: bool,
}
fn source(store: &Store, generation: Uuid) -> Result<SourceSnapshot> {
    let root = store.generation(generation);
    selection::validate_generation(&root, store.actor_id, generation)?;
    ensure!(
        (FORMAT..=notebook_generation::VERSION).contains(&feedback_generation::version(&root)?),
        "compound preparation source family differs"
    );
    selection::refuse_selected_maintenance(&root)?;
    let history = root.join("selection-history");
    files::safe_path(&history)?;
    ensure!(
        !history.exists() || fs::read_dir(&history)?.next().is_none(),
        "selected history requires explicit receiving admission before compound preparation"
    );
    ensure!(
        !store
            .selected_attempt_inventory()?
            .iter()
            .any(|a| a.state == selection::SelectedAttemptState::Pending),
        "pending original attempt prevents compound preparation"
    );
    let mut budget =
        selection::SelectedReadBudget::new(0, MAX_ITEMS as u64, MAX_METADATA as u64, MAX_MEDIA);
    let (contract_name, contract) = match feedback_generation::version(&root)? {
        notebook_generation::VERSION => ("contract-v3.json", include_str!("contract-v3.json")),
        feedback_generation::VERSION => ("contract-v2.json", include_str!("contract-v2.json")),
        _ => ("contract-v1.json", CONTRACT_V1),
    };
    let actual_contract = files::read_with_budget(
        &store.paths.data.join(contract_name),
        MAX_RECORD as u64,
        Some(&mut budget),
    )?;
    ensure!(
        actual_contract == contract.as_bytes(),
        "compound source contract differs"
    );
    let mut feature_hashes = BTreeMap::new();
    for name in [
        "selection-feature.json",
        feedback_generation::MARKER,
        notebook_generation::MARKER,
    ] {
        let path = root.join(name);
        files::safe_path(&path)?;
        feature_hashes.insert(
            name,
            if path.exists() {
                Some(digest(&files::read_with_budget(
                    &path,
                    MAX_RECORD as u64,
                    Some(&mut budget),
                )?))
            } else {
                None
            },
        );
    }
    let mut manifests = Vec::new();
    let mut objects = BTreeMap::new();
    for item in fs::read_dir(root.join("commits"))? {
        let path = item?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".stage-"))
        {
            continue;
        }
        budget.charge(0, 1, 0, 0)?;
        let raw = files::read_with_budget(&path, MAX_METADATA as u64, Some(&mut budget))?;
        let manifest: Manifest = serde_json::from_slice(&raw)?;
        manifest.validate()?;
        ensure!(
            manifest.scope == Scope::LocalTransaction
                && path.file_name().and_then(|s| s.to_str())
                    == Some(format!("{}.json", manifest.transaction_id).as_str()),
            "compound source manifest custody differs"
        );
        for (reference, media) in manifest
            .records
            .iter()
            .map(|r| (r, false))
            .chain(manifest.media.iter().map(|r| (r, true)))
        {
            if let Some(previous) = objects.insert(reference.sha256.clone(), reference.clone()) {
                ensure!(
                    previous == *reference,
                    "compound source reference collision"
                );
            } else {
                budget.charge(0, 1, 0, 0)?;
                files::Source::open_with_budget(
                    &root.join("objects").join(&reference.sha256),
                    reference,
                    Some(&mut budget),
                    media,
                )?;
            }
        }
        manifests.push((manifest, raw));
    }
    manifests.sort_by_key(|(m, _)| m.transaction_id);
    let fingerprint = digest(&serde_json::to_vec(&(
        generation,
        feedback_generation::version(&root)?,
        digest(&actual_contract),
        feature_hashes,
        manifests
            .iter()
            .map(|(m, raw)| (m.transaction_id, digest(raw)))
            .collect::<Vec<_>>(),
        objects,
    ))?);
    ensure!(
        store.recover(generation)?.unavailable == 0,
        "compound source has unavailable history"
    );
    Ok(SourceSnapshot {
        sha256: fingerprint,
        manifests,
        feedback_feature: root.join(feedback_generation::MARKER).exists(),
    })
}

impl Store {
    pub(crate) fn ordinary_reader_family(&self) -> Result<bool> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Store mutex unavailable"))?;
        match target_generation::base_version(&self.generation(inner.generation))? {
            1..=4 => Ok(true),
            5 => Ok(false),
            _ => anyhow::bail!("unsupported runtime Store family"),
        }
    }
    pub fn prepare_shared_notebook(paths: StorePaths, request: Request) -> Result<Self> {
        Self::prepare_shared_notebook_with(paths, request, |_| Ok(()))
    }
    fn prepare_shared_notebook_with(
        paths: StorePaths,
        request: Request,
        mut hook: impl FnMut(Stage) -> Result<()>,
    ) -> Result<Self> {
        request.validate()?;
        // ExistingCustody does not initialize paths, clean staging or repair a
        // marker. It also preserves the stable cross-process Store lock.
        let custody = maintenance::ExistingCustody::open(paths, |_, _| Ok(()), None)?;
        let current = custody.generation;
        let mut store = Self {
            paths: custody.paths,
            runtime_policy: None,
            actor_id: custody.actor,
            _lease: custody.lease,
            inner: Mutex::new(Inner {
                generation: current,
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
        request.current(&store)?;
        let path = store.paths.data.join(PREPARATION);
        files::safe_path(&path)?;
        let SourceSnapshot {
            sha256: source_sha,
            manifests,
            feedback_feature: feedback,
        } = source(&store, request.source_generation)?;
        let preparation = if path.exists() {
            let original: Preparation = files::json(&path, MAX_RECORD as u64)?;
            original.validate()?;
            ensure!(
                original.request == request && original.source_sha256 == source_sha,
                "compound preparation differs from original"
            );
            original
        } else {
            ensure!(
                current == request.source_generation,
                "compound source is not current"
            );
            if feedback_generation::version(&store.generation(current))?
                == notebook_generation::VERSION
            {
                notebook_generation::validate_ready(&store.paths, store.actor_id, current)?;
            }
            let target_generation = loop {
                let proposed = Uuid::new_v4();
                if proposed != request.source_generation && !store.generation(proposed).exists() {
                    break proposed;
                }
            };
            let original = Preparation {
                format: 1,
                request: request.clone(),
                target_generation,
                source_sha256: source_sha,
                contract_sha256: digest(CONTRACT.as_bytes()),
                feedback_feature: feedback,
            };
            files::atomic_json(&path, &original)?;
            original
        };
        let target = preparation.target_generation;
        ensure!(
            current == request.source_generation || current == target,
            "compound current left original source/target"
        );
        let target_root = store.generation(target);
        if current == request.source_generation {
            hook(Stage::BeforeTarget)?;
            if !target_root.exists() {
                fs::create_dir(&target_root)?;
            }
            files::safe_path(&target_root)?;
            let staging = target_root.join("compound-staging.json");
            files::safe_path(&staging)?;
            if staging.exists() {
                ensure!(
                    files::json::<Preparation>(&staging, MAX_RECORD as u64)? == preparation,
                    "compound staging owner differs"
                );
            } else {
                ensure!(
                    fs::read_dir(&target_root)?.next().is_none(),
                    "compound target contains unowned data"
                );
                files::atomic_json(&staging, &preparation)?;
            }
            for child in ["objects", "commits", "selections", "selection-history"] {
                files::directory(&target_root.join(child))?;
            }
            selection::stage_compound_feature(&target_root, store.actor_id, target)?;
            if feedback {
                feedback_generation::stage(&target_root, store.actor_id, target)?;
            } else {
                ensure!(
                    !target_root.join(feedback_generation::MARKER).exists(),
                    "unexpected compound feedback feature"
                );
            }
            notebook_generation::stage_compound_feature(&target_root, store.actor_id, target)?;
            files::atomic_json(&target_root.join(MARKER), &preparation)?;
            files::atomic_json(&target_root.join("format.json"), &VERSION)?;
            hook(Stage::AfterMarkers)?;
            let source_root = store.generation(request.source_generation);
            for (manifest, raw) in &manifests {
                for reference in manifest.records.iter().chain(&manifest.media) {
                    files::copy_verified(
                        &target_root.join("objects").join(&reference.sha256),
                        File::open(source_root.join("objects").join(&reference.sha256))?,
                        reference,
                    )?;
                }
                let destination = target_root
                    .join("commits")
                    .join(format!("{}.json", manifest.transaction_id));
                if destination.exists() {
                    ensure!(
                        files::read(&destination, MAX_METADATA as u64)? == *raw,
                        "compound staged manifest differs"
                    );
                } else {
                    files::atomic(&destination, raw)?;
                }
            }
            hook(Stage::AfterObjects)?;
            files::atomic(
                &store.paths.data.join("contract-v4.json"),
                CONTRACT.as_bytes(),
            )?;
            hook(Stage::AfterContract)?;
            selection::validate_generation(&target_root, store.actor_id, target)?;
            let rebuilt = store.recover(target)?;
            ensure!(
                rebuilt.unavailable == 0
                    && rebuilt.manifests.keys().copied().collect::<BTreeSet<_>>()
                        == manifests.iter().map(|(m, _)| m.transaction_id).collect(),
                "compound target has unavailable history"
            );
            request.current(&store)?;
            ensure!(
                files::json::<Uuid>(&store.paths.data.join("CURRENT"), 128)?
                    == request.source_generation
                    && source(&store, request.source_generation)?.sha256
                        == preparation.source_sha256,
                "compound source changed before CURRENT"
            );
            hook(Stage::BeforeCurrent)?;
            files::atomic_json(&store.paths.data.join("CURRENT"), &target)?;
            hook(Stage::AfterCurrent)?;
        }
        validate_feature(&target_root, store.actor_id, target)?;
        selection::validate_generation(&target_root, store.actor_id, target)?;
        let rebuilt = store.recover(target)?;
        ensure!(
            rebuilt.unavailable == 0
                && rebuilt.manifests.keys().copied().collect::<BTreeSet<_>>()
                    == manifests.iter().map(|(m, _)| m.transaction_id).collect(),
            "compound target has unavailable history"
        );
        {
            let inner = store
                .inner
                .get_mut()
                .map_err(|_| anyhow::anyhow!("compound Store mutex unavailable"))?;
            inner.generation = target;
            inner.index = rebuilt;
        }
        publish_capabilities(&store, target)?;
        hook(Stage::AfterCapabilities)?;
        Ok(store)
    }
}

#[cfg(test)]
pub(in crate::storage) mod tests;
