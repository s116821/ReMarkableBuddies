//! Original-only local recovery for a qualified selected promotion. Saved facts
//! admit no model/HTTP request and never mint a SelectionToken or policy owner.
use super::*;
use crate::conversation::IntentSelectionEvidence as Evidence;
use serde::{Deserialize, Serialize};
const FILE: &str = "source-target-startup-policy.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Correspondence {
    operation: Uuid,
    source_pin: String,
    guard: record::Condition,
    preference: Option<Evidence>,
    absence: Option<(String, String, String)>,
    identity: Identity,
    registration: Registration,
    entry: Entry,
    root: String,
    reader: Evidence,
    mode_actor: Uuid,
    mode_sha: String,
    binding_root: String,
    config_path: PathBuf,
    created_ms: u64,
    expires_ms: u64,
    initial_allowance: crate::storage::selection::SelectedReadCheckpoint,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    correspondence: Correspondence,
    remaining: SelectedReadBudget,
}
fn now() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}
impl Original {
    fn pin(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(&self.correspondence)?))
    }
    fn save(&self, store: &Store) -> Result<()> {
        files::atomic_json(&store.paths.data.join(FILE), self)
    }
    fn basic(&self, store: &Store, config: &Config) -> Result<Binding> {
        let c = &self.correspondence;
        ensure!(
            now()? >= c.created_ms && now()? < c.expires_ms,
            "original target startup expired"
        );
        ensure!(
            self.remaining.is_monotonic_successor(&c.initial_allowance),
            "original target allowance changed"
        );
        ensure!(
            catalog::current_generation(store)? == c.guard.store_generation
                && store.actor_id == c.mode_actor,
            "original target owner changed"
        );
        let current = store.sync_mode_snapshot()?;
        let token = serde_json::to_value(&current.token)?;
        ensure!(
            token["actor"] == c.mode_actor.to_string()
                && token["sha256"] == c.mode_sha
                && current.epoch == Some(c.guard.mode_epoch),
            "original target mode changed"
        );
        let Some(mode::SyncMode::SharedGroup {
            group,
            binding_sha256,
            root_id,
        }) = current.mode
        else {
            anyhow::bail!("original target mode is no longer shared")
        };
        ensure!(
            group == c.identity.group
                && binding_sha256 == c.identity.binding_sha256
                && root_id == c.binding_root,
            "original target binding changed"
        );
        let scope = SelectionScope {
            group: c.reader.group,
            key_sha256: c.reader.key_sha256.clone(),
            binding_sha256: c.reader.binding_sha256.clone(),
        };
        ensure!(
            !config.sync.enabled && config.reader.selection.as_ref() == Some(&scope),
            "original target Config selection changed"
        );
        ensure!(
            digest(&files::read(&c.config_path, MAX_RECORD as u64)?) == c.guard.local_config_sha256,
            "original target Config bytes changed"
        );
        Ok(Binding {
            group,
            root_id,
            digest: binding_sha256,
        })
    }
    fn verify_ready(
        &self,
        store: &Store,
        binding: &Binding,
        reads: &mut SelectedReadBudget,
    ) -> Result<()> {
        let c = &self.correspondence;
        let scope = SelectionScope {
            group: binding.group,
            key_sha256: record::key_sha256(),
            binding_sha256: binding.digest.clone(),
        };
        reads.charge(0, 1, MAX_RECORD as u64, 0)?;
        ensure!(
            digest(&files::read(&c.config_path, MAX_RECORD as u64)?) == c.guard.local_config_sha256,
            "original target Config changed"
        );
        let current = store.selected_snapshot_with_budget(&scope, MAX_ITEMS, reads)?;
        match (&c.preference, current) {
            (Some(expected), Some(read)) => ensure!(
                Evidence::from_token(&read.snapshot.token) == *expected,
                "original target preference changed"
            ),
            (None, None) => {
                let (root, base, slot) = c
                    .absence
                    .as_ref()
                    .context("qualified original absence absent")?;
                let cp = command::checkpoint(store, binding, c.guard.mode_epoch, reads)?
                    .context("original target checkpoint absent")?;
                let view = catalog_view(binding, &cp)?.context("original target catalog absent")?;
                ensure!(
                    !cp.catalog.is_empty()
                        && view.root_sha == *root
                        && view.base == *base
                        && view.slot == *slot
                        && !view.entries.contains_key(&record::key_sha256())
                        && cp
                            .catalog_pending
                            .as_ref()
                            .map(CatalogPending::key)
                            .transpose()?
                            .as_deref()
                            != Some(record::key_sha256().as_str())
                        && !cp.scopes.contains_key(&record::key_sha256())
                        && !cp
                            .retired_catalog
                            .iter()
                            .any(|p| p.key == record::key_sha256()),
                    "original target absence changed"
                );
            }
            _ => anyhow::bail!("original target selection/absence changed"),
        }
        let scope = SelectionScope {
            group: c.reader.group,
            key_sha256: c.reader.key_sha256.clone(),
            binding_sha256: c.reader.binding_sha256.clone(),
        };
        let (registration, entry, token) = ReaderPreferences::registered_bound(
            binding,
            c.guard.mode_epoch,
            &c.identity,
            &scope,
            store,
            &c.root,
            reads,
        )?;
        ensure!(
            registration == c.registration
                && entry == c.entry
                && Evidence::from_token(&token) == c.reader,
            "original target registered document changed"
        );
        Ok(())
    }
}
/// Reserve recovery proof from the already partitioned original admission, before
/// writing the promotion intent. No counter or deadline is created on reopen.
pub(super) fn freeze(
    store: &Store,
    inner: &DetachedPreferences,
    identity: &Identity,
    registration: &Registration,
    entry: &Entry,
    root: &str,
    token: &SelectionToken,
) -> Result<(String, Uuid)> {
    let created_ms = now()?;
    let expires_ms = created_ms
        .checked_add(u64::try_from(
            inner
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis(),
        )?)
        .context("target deadline overflow")?;
    let mut reads = inner
        .reads
        .admission
        .lock()
        .map_err(|_| anyhow::anyhow!("target original allowance unavailable"))?;
    let remaining = reads.reserve(64, MAX_ITEMS as u64 * 8, MAX_METADATA as u64 * 32, 0)?;
    let initial_allowance = remaining.checkpoint();
    let mode = serde_json::to_value(&inner.mode_token)?;
    let original = Original {
        correspondence: Correspondence {
            operation: Uuid::new_v4(),
            source_pin: crate::storage::target_generation::source_pin(store)?,
            guard: inner.guard.clone(),
            preference: inner.selected.as_ref().map(Evidence::from_token),
            absence: inner.absence.clone(),
            identity: identity.clone(),
            registration: registration.clone(),
            entry: entry.clone(),
            root: root.into(),
            reader: Evidence::from_token(token),
            mode_actor: store.actor_id,
            mode_sha: mode["sha256"]
                .as_str()
                .context("target mode hash absent")?
                .into(),
            binding_root: inner.binding.root_id.clone(),
            config_path: inner.capture.path.clone(),
            created_ms,
            expires_ms,
            initial_allowance,
        },
        remaining,
    };
    let pin = original.pin()?;
    if store.paths.data.join(FILE).exists() {
        // A ready existing store consumes no new migration intent/allowance.
        anyhow::bail!("original selected target startup already exists");
    }
    original.save(store)?;
    Ok((pin, original.correspondence.operation))
}
pub(super) fn resume(store: &mut Store, config: &Config) -> Result<bool> {
    if !crate::storage::target_generation::selected_preparation_pending(store)? {
        return Ok(false);
    }
    let mut original: Original = files::json(&store.paths.data.join(FILE), MAX_RECORD as u64)?;
    let binding = original.basic(store, config)?;
    // Reserve the complete proof attempt and persist its spent parent BEFORE
    // any repeated history read. A crash cannot renew a proof allowance.
    let mut proof =
        original
            .remaining
            .reserve(8, MAX_ITEMS as u64 * 4, MAX_METADATA as u64 * 8, 0)?;
    original.save(store)?;
    ensure!(
        crate::storage::target_generation::source_pin(store)? == original.correspondence.source_pin,
        "original selected source/checkpoint changed"
    );
    crate::storage::target_generation::prepare_selected_original(
        store,
        &original.pin()?,
        original.correspondence.operation,
        |_| Ok(()),
    )?;
    original.verify_ready(store, &binding, &mut proof)?;
    Ok(true)
}
