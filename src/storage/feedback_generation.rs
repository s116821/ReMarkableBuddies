//! Explicit ordinary feedback feature admission; no native or display authority.
use super::*;
use crate::unassociated_feedback::{self, Description, Observation};
pub(crate) const VERSION: u32 = 3;
pub(crate) const MARKER: &str = "feedback-feature.json";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Feature {
    pub format: u32,
    pub actor: Uuid,
    pub generation: Uuid,
}
pub(crate) fn version(root: &Path) -> Result<u32> {
    files::json(&root.join("format.json"), 64)
}
pub(crate) fn validate(
    root: &Path,
    actor: Uuid,
    generation: Uuid,
    allow_pending: bool,
) -> Result<()> {
    validate_version(root, actor, generation, version(root)?, allow_pending)
}
pub(crate) fn validate_version(
    root: &Path,
    actor: Uuid,
    generation: Uuid,
    version: u32,
    allow_pending: bool,
) -> Result<()> {
    let marker = root.join(MARKER);
    files::safe_path(&marker)?;
    if version == VERSION || marker.exists() {
        ensure!(
            files::json::<Feature>(&marker, MAX_RECORD as u64)
                .map_err(|_| anyhow::anyhow!("invalid feedback feature"))?
                == Feature {
                    format: 1,
                    actor,
                    generation
                },
            "unsupported feedback feature"
        );
        ensure!(
            version == VERSION
                || allow_pending
                || (version == notebook_generation::VERSION
                    && notebook_generation::validate_version(
                        root, actor, generation, version, false
                    )
                    .is_ok()),
            "interrupted feedback preparation"
        );
    }
    Ok(())
}
pub(crate) fn stage(root: &Path, actor: Uuid, generation: Uuid) -> Result<()> {
    files::atomic_json(
        &root.join(MARKER),
        &Feature {
            format: 1,
            actor,
            generation,
        },
    )?;
    files::atomic_json(&root.join("format.json"), &VERSION)?;
    validate(root, actor, generation, false)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparationStage {
    BeforeMarker,
    AfterMarker,
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
            && inner.local_domain_handles == 0,
        "feedback preparation requires unique complete custody"
    );
    ensure!(
        if version(&store.generation(inner.generation))? == notebook_generation::VERSION {
            sync::mode::ordinary_local(store)?
        } else {
            sync::mode::load(store)?.is_none()
        },
        "feedback requires ordinary local mode"
    );
    let generation = inner.generation;
    let root = store.generation(generation);
    selection::validate_generation_preparing(&root, store.actor_id, generation)?;
    if version(&root)? == notebook_generation::VERSION {
        ensure!(
            root.join(MARKER).exists(),
            "feedback enablement on notebook storage unsupported"
        );
        return notebook_generation::validate_ready(&store.paths, store.actor_id, generation);
    }
    if version(&root)? != VERSION {
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
        files::atomic_json(&root.join("format.json"), &VERSION)?;
    }
    hook(PreparationStage::AfterFormat)?;
    validate(&root, store.actor_id, generation, false)?;
    store.publish_capabilities(&root)?;
    hook(PreparationStage::AfterCapabilities)?;
    Ok(())
}
pub(super) fn admit(
    root: &Path,
    actor: Uuid,
    generation: Uuid,
    index: &Index,
    records: &[(Envelope, String)],
    scope: &Scope,
) -> Result<()> {
    let feedback: Vec<_> = records
        .iter()
        .filter(|(e, _)| e.namespace == Namespace::UnassociatedFeedback)
        .collect();
    if feedback.is_empty() {
        return Ok(());
    }
    ensure!(
        *scope == Scope::LocalTransaction,
        "feedback is not selected/group data"
    );
    ensure!(
        matches!(version(root)?, VERSION | notebook_generation::VERSION) && index.unavailable == 0,
        "feedback requires prepared complete generation"
    );
    validate(root, actor, generation, false)?;
    let mut ids: BTreeSet<_> = index
        .heads
        .keys()
        .filter(|(n, _)| *n == Namespace::UnassociatedFeedback)
        .map(|(_, id)| *id)
        .collect();
    for (envelope, hash) in feedback {
        unassociated_feedback::validate_envelope(envelope, actor)?;
        if let Some(heads) = index
            .heads
            .get(&(Namespace::UnassociatedFeedback, envelope.record_id))
        {
            ensure!(
                heads.len() == 1
                    && heads.contains(&envelope.revision_id)
                    && index
                        .records
                        .get(&envelope.revision_id)
                        .is_some_and(|(_, previous)| previous == hash),
                "immutable feedback conflict"
            );
        }
        ids.insert(envelope.record_id);
    }
    ensure!(ids.len() <= MAX_ITEMS, "feedback capacity exceeded");
    Ok(())
}
impl Store {
    pub(super) fn check_feedback(&self, inner: &Inner) -> Result<()> {
        let notebook = version(&self.generation(inner.generation))? == notebook_generation::VERSION;
        ensure!(
            inner.index.unavailable == 0
                && if notebook {
                    sync::mode::ordinary_local(self)?
                } else {
                    inner.mode_handles == 0 && sync::mode::load(self)?.is_none()
                },
            "feedback unavailable in current mode/history"
        );
        let root = self.generation(inner.generation);
        ensure!(
            matches!(version(&root)?, VERSION | notebook_generation::VERSION),
            "feedback feature not prepared"
        );
        if version(&root)? == notebook_generation::VERSION {
            ensure!(root.join(MARKER).exists(), "feedback feature absent");
            notebook_generation::validate_ready(&self.paths, self.actor_id, inner.generation)?;
        }
        validate(&root, self.actor_id, inner.generation, false)
    }
    pub(crate) fn retain_feedback(&self, observation: Observation) -> Result<Description> {
        let envelope = observation.envelope()?;
        let (manifest, objects) =
            Self::ordinary_transaction(vec![envelope.clone()], BTreeMap::new())?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        self.check_feedback(&inner)?;
        let result = self.publish_locked(&mut inner, manifest.clone(), objects, true);
        let changed = result.as_ref().map_or(true, |(_, changed)| *changed);
        if let Err(error) = result {
            // AfterCommit can return an error after the literal commit was applied.
            let root = self.generation(inner.generation);
            let verified = inner.index.manifests.get(&manifest.transaction_id) == Some(&manifest)
                && files::json::<Manifest>(
                    &root
                        .join("commits")
                        .join(format!("{}.json", manifest.transaction_id)),
                    MAX_METADATA as u64,
                )
                .is_ok_and(|m| m == manifest)
                && Self::validate_objects(&root, &manifest)
                    .is_ok_and(|records| records.len() == 1 && records[0].0 == envelope);
            if !verified {
                return Err(error);
            }
        }
        drop(inner);
        if changed {
            self.notify_commit();
        }
        observation.description()
    }
    pub(crate) fn feedback_snapshot(&self) -> Result<Vec<Description>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        self.check_feedback(&inner)?;
        let mut result = Vec::new();
        let mut bytes = 0usize;
        for ((namespace, _), heads) in &inner.index.heads {
            if *namespace != Namespace::UnassociatedFeedback {
                continue;
            }
            ensure!(
                heads.len() == 1 && result.len() < MAX_ITEMS,
                "feedback snapshot conflict/bound"
            );
            let (record, _) = inner
                .index
                .records
                .get(heads.iter().next().unwrap())
                .context("feedback head unavailable")?;
            let observation = unassociated_feedback::validate_envelope(record, self.actor_id)?;
            let description = observation.description()?;
            bytes += serde_json::to_vec(&description)?.len();
            ensure!(bytes <= MAX_METADATA, "feedback snapshot metadata bound");
            result.push(description);
        }
        Ok(result)
    }
}

pub(super) fn create(root: &Path, generation: Uuid, actor: Uuid) -> Result<()> {
    let next = root.join("generations").join(generation.to_string());
    for child in ["objects", "commits"] {
        files::directory(&next.join(child))?;
    }
    stage(&next, actor, generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_crash_boundaries_resume_under_unique_lease() {
        for fault in [
            PreparationStage::BeforeMarker,
            PreparationStage::AfterMarker,
            PreparationStage::AfterFormat,
            PreparationStage::AfterCapabilities,
        ] {
            let base = std::env::temp_dir().join(format!("feedback-prepare-{}", Uuid::new_v4()));
            let paths = StorePaths {
                data: base.join("data"),
                cache: base.join("cache"),
                credentials: base.join("credentials"),
            };
            let mut store = Store::open_for_feedback(paths.clone()).unwrap();
            assert!(prepare(&mut store, |stage| {
                if stage == fault {
                    anyhow::bail!("controlled preparation interruption")
                }
                Ok(())
            })
            .is_err());
            drop(store);
            if fault == PreparationStage::AfterMarker {
                assert!(Store::open(paths.clone()).is_err());
            }
            let ready = crate::unassociated_feedback::Feedback::prepare(paths).unwrap();
            assert!(ready.snapshot().unwrap().is_empty());
            drop(ready);
            fs::remove_dir_all(base).unwrap();
        }
    }
    #[test]
    fn selected_feature_bytes_survive_feedback_preparation_and_no_downgrade() {
        let base = std::env::temp_dir().join(format!("feedback-selected-{}", Uuid::new_v4()));
        let paths = StorePaths {
            data: base.join("data"),
            cache: base.join("cache"),
            credentials: base.join("credentials"),
        };
        let mut store = Store::open_for_feedback(paths).unwrap();
        let generation = store.inner.lock().unwrap().generation;
        let root = store.generation(generation);
        selection::prepare_generation(&root, store.actor_id, generation).unwrap();
        let original = fs::read(root.join("selection-feature.json")).unwrap();
        prepare(&mut store, |_| Ok(())).unwrap();
        selection::prepare_generation(&root, store.actor_id, generation).unwrap();
        assert_eq!(version(&root).unwrap(), 3);
        assert_eq!(
            fs::read(root.join("selection-feature.json")).unwrap(),
            original
        );
        drop(store);
        fs::remove_dir_all(base).unwrap();
    }
}
