//! Conditional maintenance around the existing same-actor notebook writer.
use super::super::ordinary_restore::{
    Conditions, History, Plan, Preview, Publication, Refused, Summary,
};
use super::*;

fn ensure_condition(value: bool, refusal: Refused) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(refusal.into())
    }
}
fn absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}
fn config_hash(backup: &NotebookBackup) -> Result<Option<String>> {
    backup
        .config_snapshot
        .as_ref()
        .map(|c| serde_json::to_vec(c).map(|b| digest(&b)))
        .transpose()
        .map_err(Into::into)
}
pub(super) fn read_archive(source: &Path) -> Result<(NotebookBackup, String)> {
    let (_, mut witness) = command::Detached::inspect(source).map_err(|_| Refused::Unsupported)?;
    let backup = NotebookBackup::read(source).map_err(|_| Refused::Unsupported)?;
    let hash = digest(&files::read(
        &source.join("backup.json"),
        MAX_METADATA as u64,
    )?);
    witness.verify().map_err(|_| Refused::Conflict)?;
    Ok((backup, hash))
}
pub(super) fn features(root: &Path) -> Result<(bool, bool)> {
    Ok((
        command::regular_if_present(&root.join(feedback_generation::MARKER))?,
        command::regular_if_present(&root.join("selection-feature.json"))?,
    ))
}
fn history_hash(history: &History, flags: (bool, bool), actor: Uuid) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        "ordinary-notebook-restore-history-v1",
        actor,
        flags,
        history.hash()?,
    ))?))
}
pub(super) fn target_base_hash(
    history: &History,
    flags: (bool, bool),
    actor: Uuid,
    origin: &Option<target_generation::ArchiveOrigin>,
) -> Result<String> {
    target_base_digest(&history.hash()?, flags, actor, origin)
}
pub(super) fn target_base_digest(
    hash: &str,
    flags: (bool, bool),
    actor: Uuid,
    origin: &Option<target_generation::ArchiveOrigin>,
) -> Result<String> {
    Ok(digest(&if let Some(origin) = origin {
        serde_json::to_vec(&(
            "ordinary-target-base-history-v1",
            actor,
            flags,
            6u32,
            origin,
            hash,
        ))?
    } else {
        serde_json::to_vec(&("ordinary-notebook-restore-history-v1", actor, flags, hash))?
    }))
}
pub(super) fn target_history_hash(
    history: &History,
    flags: (bool, bool),
    actor: Uuid,
    target: Uuid,
    base: &Option<target_generation::ArchiveOrigin>,
    archive: &Option<target_generation::ArchiveOrigin>,
) -> Result<String> {
    if base.is_some() || archive.is_some() {
        Ok(digest(&serde_json::to_vec(&(
            "ordinary-target-restore-history-v1",
            actor,
            flags,
            if base.is_some() { 6u32 } else { 4u32 },
            if archive.is_some() { 6u32 } else { 4u32 },
            6u32,
            target,
            base,
            archive,
            history.hash()?,
        ))?))
    } else {
        history_hash(history, flags, actor)
    }
}
impl Store {
    pub(super) fn notebook_restore_history(&self, inner: &Inner) -> Result<History> {
        self.notebook_archive_custody(inner)?;
        let root = self.generation(inner.generation);
        let mut history = History::empty();
        let actor = if target_generation::is_target(&root)? {
            None
        } else {
            Some(self.actor_id)
        };
        for manifest in inner.index.manifests.values() {
            // A live target base keeps ordinary ready-family admission. Only the
            // detached archive gets its explicit closed-header context below.
            history.add_with_actor(&root, manifest, actor)?;
        }
        history.validate()?;
        Ok(history)
    }
    pub(super) fn notebook_restore_plan(
        &self,
        inner: &Inner,
        source: &Path,
        backup: &NotebookBackup,
        hash: String,
        target: Uuid,
    ) -> Result<Plan> {
        ensure_condition(backup.origin_actor == self.actor_id, Refused::Unsupported)?;
        let root = self.generation(inner.generation);
        let mut history = self.notebook_restore_history(inner)?;
        let base_flags = features(&root)?;
        let base_origin =
            target_generation::archive_origin(&root, self.actor_id, inner.generation)?;
        let base_hash = target_base_hash(&history, base_flags, self.actor_id, &base_origin)?;
        let before = history.manifests.len();
        let archive_context = backup.target_context(source)?;
        for manifest in &backup.manifests {
            history
                .add_with_archive(
                    source,
                    manifest,
                    Some(self.actor_id),
                    archive_context.as_ref(),
                )
                .map_err(|_| Refused::Conflict)?;
        }
        history.validate().map_err(|_| Refused::Conflict)?;
        let target_flags = (
            base_flags.0 || backup.feedback_feature,
            base_flags.1 || backup.selected_feature,
        );
        let summary = history.summary(backup.config_snapshot.is_some());
        ensure_condition(summary.complete_media, Refused::Unsupported)?;
        Ok(Plan {
            conditions: Conditions {
                archive_sha256: hash,
                base_generation: inner.generation,
                base_history_sha256: base_hash,
                target_generation: target,
                target_history_sha256: target_history_hash(
                    &history,
                    target_flags,
                    self.actor_id,
                    target,
                    &base_origin,
                    &backup.target_feature,
                )?,
                staged_configuration_sha256: config_hash(backup)?,
                complete_media: true,
            },
            preview: Preview {
                source: Summary {
                    manifests: backup.manifests.len(),
                    objects: backup.objects.len(),
                    complete_media: true,
                    configuration_included: backup.config_snapshot.is_some(),
                },
                additional_manifests: summary.manifests - before,
                target: summary,
                additive: true,
                configuration_applied: false,
            },
        })
    }
    pub(crate) fn prepare_notebook_restore(
        &self,
        source: &Path,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Plan> {
        let inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        target_restore::refuse_pending(&self.paths)?;
        let (backup, hash) = read_archive(source)?;
        let next = Uuid::new_v4();
        ensure_condition(
            next != inner.generation && absent(&self.generation(next)),
            Refused::Conflict,
        )?;
        self.notebook_restore_plan(&inner, source, &backup, hash, next)
    }
    pub(crate) fn apply_notebook_restore(
        &self,
        source: &Path,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Publication> {
        let mut inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        ensure_condition(
            inner.generation == expected.base_generation
                && absent(&self.generation(expected.target_generation)),
            Refused::Conflict,
        )?;
        let (backup, hash) = read_archive(source)?;
        ensure_condition(hash == expected.archive_sha256, Refused::Conflict)?;
        let actual = self
            .notebook_restore_plan(&inner, source, &backup, hash, expected.target_generation)?
            .conditions;
        ensure_condition(
            actual.base_history_sha256 == expected.base_history_sha256
                && actual.target_history_sha256 == expected.target_history_sha256
                && actual.staged_configuration_sha256 == expected.staged_configuration_sha256
                && actual.complete_media == expected.complete_media,
            Refused::Conflict,
        )?;
        let mut attempted = false;
        let target = backup.target_feature.is_some()
            || target_generation::is_target(&self.generation(inner.generation))?;
        let result = if target {
            self.restore_target_locked(&mut inner, source, backup, expected, &mut attempted)
        } else {
            self.restore_notebook_locked(
                &mut inner,
                source,
                backup,
                expected.target_generation,
                true,
                &mut attempted,
            )
        };
        if result.is_err() {
            return Ok(if attempted {
                Publication::Unknown
            } else {
                Publication::NotActivated
            });
        }
        Ok(
            match self.observe_notebook_restore_locked(&inner, expected) {
                Ok(Some(summary)) => Publication::Verified(summary),
                _ => Publication::Unknown,
            },
        )
    }
    fn observe_notebook_restore_locked(
        &self,
        inner: &Inner,
        expected: &Conditions,
    ) -> Result<Option<Summary>> {
        let current: Uuid = files::json(&self.paths.data.join("CURRENT"), 128)?;
        if current != inner.generation || current != expected.target_generation {
            return Ok(None);
        }
        let root = self.generation(current);
        if target_generation::is_target(&root)? {
            return target_restore::observe(self, inner, expected);
        }
        let history = self.notebook_restore_history(inner)?;
        if history_hash(&history, features(&root)?, self.actor_id)?
            != expected.target_history_sha256
        {
            return Ok(None);
        }
        let path = root.join("restored-config.json");
        let config = if command::regular_if_present(&path)? {
            Some(digest(&files::read(&path, MAX_RECORD as u64)?))
        } else {
            None
        };
        let summary = history.summary(config.is_some());
        if config != expected.staged_configuration_sha256
            || summary.complete_media != expected.complete_media
        {
            return Ok(None);
        }
        Ok(Some(summary))
    }
    pub(crate) fn observe_notebook_restore(
        &self,
        expected: &Conditions,
        binding: impl FnOnce() -> Result<()>,
    ) -> Result<Option<Summary>> {
        let inner = self.inner.lock().map_err(|_| Refused::Unavailable)?;
        binding()?;
        self.observe_notebook_restore_locked(&inner, expected)
    }
}
