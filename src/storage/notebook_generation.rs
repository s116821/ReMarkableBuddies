//! Explicit identity-preserving notebook adoption. No native or model authority.
use super::*;
use std::sync::atomic::Ordering;

pub const VERSION: u32 = 4;
pub const MARKER: &str = "notebook-feature.json";
const CONTRACT: &str = include_str!("contract-v3.json");

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Feature {
    format: u32,
    actor: Uuid,
    generation: Uuid,
}

pub(super) fn create(
    data: &Path,
    actor: Uuid,
    generation: Uuid,
    feedback: bool,
    selected_feature: bool,
) -> Result<()> {
    Store::create_generation(data, generation)?;
    let root = data.join("generations").join(generation.to_string());
    if selected_feature {
        selection::prepare_generation(&root, actor, generation)?;
    }
    if feedback {
        feedback_generation::create(data, generation, actor)?;
    }
    files::atomic_json(
        &root.join(MARKER),
        &Feature {
            format: 1,
            actor,
            generation,
        },
    )?;
    files::atomic_json(&root.join("format.json"), &VERSION)
}

pub(crate) fn validate_version(
    root: &Path,
    actor: Uuid,
    generation: Uuid,
    version: u32,
    preparing: bool,
) -> Result<()> {
    let path = root.join(MARKER);
    files::safe_path(&path)?;
    if version == VERSION || path.exists() {
        ensure!(
            files::json::<Feature>(&path, MAX_RECORD as u64)?
                == Feature {
                    format: 1,
                    actor,
                    generation
                },
            "unsupported notebook feature"
        );
        ensure!(
            version == VERSION || preparing,
            "interrupted notebook preparation"
        );
    }
    Ok(())
}

pub(crate) fn capabilities(paths: &StorePaths, actor: Uuid, generation: Uuid) -> serde_json::Value {
    let root = paths.data.join("generations").join(generation.to_string());
    let mut namespaces = vec![
        "conversation",
        "source",
        "export-association",
        "subject-memory",
        "handwriting",
    ];
    if root.join(feedback_generation::MARKER).exists() {
        namespaces.push("unassociated-feedback");
    }
    let mut result = serde_json::json!({
        "contract_version": 3, "schema_file": "contract-v3.json",
        "store_versions": [1,2,3,4], "envelope_versions": [1], "config_versions": [1],
        "owner_actor": actor, "current_generation": generation,
        "notebook_feature": "actor-generation-bound-v1", "paths": paths,
        "lock": "stable-inode-exclusive-os-lease", "linux_lock": "flock",
        "maintenance": "stop-confirm-lock-stage-commit-release-restart",
        "namespaces": namespaces, "sync": "unsupported-notebook-generation"
    });
    if root.join(feedback_generation::MARKER).exists() {
        result["unassociated_feedback_feature"] = serde_json::json!("actor-generation-bound-v1");
    }
    if root.join("selection-feature.json").exists() {
        result["retained_selected_feature"] = serde_json::json!("actor-generation-bound-v1");
    }
    result
}

fn contract(paths: &StorePaths, publish: bool) -> Result<()> {
    let path = paths.data.join("contract-v3.json");
    files::safe_path(&path)?;
    if !path.exists() && publish {
        files::atomic(&path, CONTRACT.as_bytes())?;
    }
    ensure!(
        files::read(&path, MAX_RECORD as u64)? == CONTRACT.as_bytes(),
        "notebook contract mismatch"
    );
    Ok(())
}

pub(crate) fn publish_capabilities(store: &Store, generation: Uuid) -> Result<()> {
    contract(&store.paths, true)?;
    let expected = capabilities(&store.paths, store.actor_id, generation);
    files::atomic_json(&store.paths.data.join("capabilities.json"), &expected)?;
    validate_ready(&store.paths, store.actor_id, generation)
}

pub(crate) fn validate_ready(paths: &StorePaths, actor: Uuid, generation: Uuid) -> Result<()> {
    ensure!(
        files::json::<Uuid>(&paths.data.join("CURRENT"), 128)? == generation,
        "notebook current changed"
    );
    let root = paths.data.join("generations").join(generation.to_string());
    ensure!(
        feedback_generation::version(&root)? == VERSION,
        "notebook generation not adopted"
    );
    selection::validate_generation(&root, actor, generation)?;
    contract(paths, false)?;
    ensure!(
        files::read(&paths.data.join("capabilities.json"), MAX_RECORD as u64)?
            == serde_json::to_vec(&capabilities(paths, actor, generation))?,
        "incomplete notebook capabilities"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparationStage {
    BeforeMarker,
    AfterMarker,
    AfterContract,
    AfterFormat,
    AfterCapabilities,
}

pub(crate) fn prepare(
    store: &mut Store,
    mut hook: impl FnMut(PreparationStage) -> Result<()>,
) -> Result<()> {
    let inner = store
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
    ensure!(
        inner.index.unavailable == 0
            && inner.mode_handles == 0
            && inner.legacy_sync_handles == 0
            && inner.local_domain_handles == 0
            && inner.legacy_publisher_handles == 0
            && !store.writer_active.load(Ordering::SeqCst)
            && !store.export_active.load(Ordering::SeqCst),
        "notebook preparation requires unique complete custody"
    );
    ensure!(
        sync::mode::ordinary_local(store)?,
        "notebook requires ordinary local mode"
    );
    let generation = inner.generation;
    ensure!(
        files::json::<Uuid>(&store.paths.data.join("CURRENT"), 128)? == generation,
        "notebook current changed"
    );
    let root = store.generation(generation);
    selection::validate_generation_preparing_notebook(&root, store.actor_id, generation)?;
    selection::refuse_selected_maintenance(&root)?;
    let retained = root.join("selection-history");
    files::safe_path(&retained)?;
    ensure!(
        !retained.exists() || fs::read_dir(&retained)?.next().is_none(),
        "retained selected history prevents notebook preparation"
    );
    for ((namespace, _), heads) in &inner.index.heads {
        if *namespace != Namespace::ExportAssociation {
            continue;
        }
        ensure!(
            heads.len() == 1,
            "ambiguous export prevents notebook preparation"
        );
        let envelope = &inner.index.records[heads.first().expect("one export head")].0;
        let association: crate::conversation::ExportAssociation =
            serde_json::from_value(envelope.payload["record"].clone())
                .map_err(|_| anyhow::anyhow!("unsupported export prevents notebook preparation"))?;
        association.validate_attempt()?;
        use crate::conversation::Outcome;
        ensure!(
            matches!(association.outcome, Outcome::Completed)
                || (matches!(association.outcome, Outcome::Failed | Outcome::Canceled)
                    && association
                        .attempt
                        .as_ref()
                        .is_some_and(|attempt| attempt.no_send)),
            "unresolved export prevents notebook preparation"
        );
    }
    // Reopening rebuilds this index from actual current history. Never trust a
    // pre-format pending marker's stale heads after an old writer ran.
    let mut verified = BTreeMap::new();
    for (envelope, _) in inner.index.records.values() {
        if envelope.namespace == Namespace::UnassociatedFeedback {
            ensure!(
                root.join(feedback_generation::MARKER).exists(),
                "feedback feature absent"
            );
        }
        for media in &envelope.media_descriptors {
            if let Some(previous) = verified.insert(media.sha256.clone(), media.bytes) {
                ensure!(previous == media.bytes, "inconsistent retained media size");
            } else {
                files::Source::open(
                    &root.join("objects").join(&media.sha256),
                    &ObjectRef {
                        sha256: media.sha256.clone(),
                        bytes: media.bytes,
                    },
                )?;
            }
        }
    }
    if feedback_generation::version(&root)? != VERSION {
        hook(PreparationStage::BeforeMarker)?;
        files::atomic_json(
            &root.join(MARKER),
            &Feature {
                format: 1,
                actor: store.actor_id,
                generation,
            },
        )?;
        hook(PreparationStage::AfterMarker)?;
        contract(&store.paths, true)?;
        hook(PreparationStage::AfterContract)?;
        files::atomic_json(&root.join("format.json"), &VERSION)?;
    } else {
        contract(&store.paths, true)?;
    }
    hook(PreparationStage::AfterFormat)?;
    selection::validate_generation(&root, store.actor_id, generation)?;
    publish_capabilities(store, generation)?;
    hook(PreparationStage::AfterCapabilities)?;
    Ok(())
}

impl Store {
    /// Explicit preparation owns a fresh Store before any Arc/client escapes.
    /// Ordinary open never repairs or adopts an incomplete notebook feature.
    pub fn prepare_notebook(paths: StorePaths) -> Result<Self> {
        let mut store = Self::open_policy(paths, OpenPreparation::Notebook)?;
        prepare(&mut store, |_| Ok(()))?;
        Ok(store)
    }
}

#[cfg(test)]
mod tests;
