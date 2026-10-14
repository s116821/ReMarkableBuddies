//! Local custody for a fixed-slot attempt, never cloud/native publication authority.
use super::*;
use crate::storage::drive::slot::{SlotBinding, SlotPointer};

const MAX_DESCRIPTOR: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedAttemptRequest {
    pub operation: Uuid,
    pub slot_id: String,
    pub descriptor_id: String,
    pub descriptor: ObjectRef,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectedAttemptState {
    Pending,
    Withdrawn,
}

/// Original intent only. Contains no live token, credentials, mode or permissions.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedAttempt {
    pub format: u32,
    pub actor: Uuid,
    pub request: SelectedAttemptRequest,
    pub selection: SelectionTransaction,
    pub request_sha256: String,
    pub state: SelectedAttemptState,
    /// Private mode epoch only; absence preserves original unconfigured journals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_epoch: Option<Uuid>,
}
impl SelectedAttempt {
    fn fingerprint(&self) -> Result<String> {
        match self.mode_epoch {
            Some(epoch) => Ok(digest(&serde_json::to_vec(&(
                self.actor,
                &self.request,
                &self.selection,
                epoch,
            ))?)),
            None => Ok(digest(&serde_json::to_vec(&(
                self.actor,
                &self.request,
                &self.selection,
            ))?)),
        }
    }
    fn validate(&self, actor: Uuid) -> Result<()> {
        ensure!(
            self.format == FORMAT && self.actor == actor && !actor.is_nil(),
            "foreign or unsupported selected intent"
        );
        self.selection.validate(MAX_ITEMS)?;
        ensure!(
            !self.request.operation.is_nil()
                && self.request.descriptor.bytes > 0
                && self.request.descriptor.bytes <= MAX_DESCRIPTOR,
            "invalid selected intent request"
        );
        SlotPointer {
            binding: SlotBinding {
                group: self.selection.scope.group,
                scope_sha256: self.selection.scope.key_sha256.clone(),
                slot_id: self.request.slot_id.clone(),
            },
            descriptor_id: self.request.descriptor_id.clone(),
            descriptor_sha256: self.request.descriptor.sha256.clone(),
        }
        .validate()?;
        ensure!(
            self.request_sha256 == self.fingerprint()?,
            "selected intent identity mismatch"
        );
        Ok(())
    }
}
fn directory(store: &Store) -> PathBuf {
    store.paths.data.join("sync/selected-outbox")
}
fn inventory(store: &Store) -> Result<BTreeMap<Uuid, SelectedAttempt>> {
    let directory = directory(store);
    files::safe_path(&directory)?;
    let mut entries = BTreeMap::new();
    if !directory.exists() {
        return Ok(entries);
    }
    let mut total = 0u64;
    for entry in fs::read_dir(&directory)? {
        ensure!(
            entries.len() < MAX_ITEMS,
            "selected outbox inventory exceeds bound"
        );
        let path = entry?.path();
        let bytes = files::read(&path, MAX_METADATA as u64)?;
        total = total
            .checked_add(bytes.len() as u64)
            .context("selected outbox size overflow")?;
        ensure!(
            total <= MAX_METADATA as u64,
            "selected outbox metadata exceeds bound"
        );
        let attempt: SelectedAttempt = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("unsupported selected intent metadata"))?;
        attempt.validate(store.actor_id)?;
        ensure!(
            path.file_name().and_then(|p| p.to_str())
                == Some(&format!("{}.json", attempt.request.operation))
                && entries.insert(attempt.request.operation, attempt).is_none(),
            "selected intent filename/identity mismatch"
        );
    }
    Ok(entries)
}
fn persist(
    store: &Store,
    attempt: &SelectedAttempt,
    entries: &BTreeMap<Uuid, SelectedAttempt>,
) -> Result<()> {
    attempt.validate(store.actor_id)?;
    ensure!(
        entries.contains_key(&attempt.request.operation) || entries.len() < MAX_ITEMS,
        "selected outbox inventory exceeds bound"
    );
    let bytes = serde_json::to_vec(attempt)?;
    let total = entries
        .iter()
        .filter(|(id, _)| **id != attempt.request.operation)
        .try_fold(bytes.len() as u64, |n, (id, _)| {
            n.checked_add(fs::metadata(directory(store).join(format!("{id}.json")))?.len())
                .context("selected outbox size overflow")
        })?;
    ensure!(
        total <= MAX_METADATA as u64,
        "selected outbox metadata exceeds bound"
    );
    files::directory(&directory(store))?;
    files::atomic(
        &directory(store).join(format!("{}.json", attempt.request.operation)),
        &bytes,
    )
}
fn verify(store: &Store, generation: Uuid, attempt: &SelectedAttempt) -> Result<()> {
    ensure!(
        attempt.selection.store_generation == generation,
        "selected intent generation mismatch"
    );
    let root = store.generation(generation);
    validate_generation(&root, store.actor_id, generation)?;
    for transaction in publication::history(
        &root,
        generation,
        &attempt.selection.scope,
        attempt.selection.clone(),
    )? {
        publication::validate_closure(&root, &transaction)?;
    }
    let _ = files::Source::open(
        &root
            .join("objects")
            .join(&attempt.request.descriptor.sha256),
        &attempt.request.descriptor,
    )?;
    Ok(())
}
impl Store {
    /// Persist an explicitly prepared fixed-slot intent. Descriptor meaning and
    /// domain membership remain caller-owned; this performs no provider operation.
    pub fn prepare_selected_attempt(
        &self,
        expected: &SelectionToken,
        request: SelectedAttemptRequest,
    ) -> Result<SelectedAttempt> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            inner.legacy_sync_handles == 0,
            "stop legacy sync before selected intent preparation"
        );
        let entries = inventory(self)?;
        if let Some(original) = entries.get(&request.operation) {
            ensure!(
                original.request == request
                    && publication::token(&original.selection)? == *expected,
                "selected intent operation collision"
            );
            verify(self, original.selection.store_generation, original)?;
            return Ok(original.clone()); // Original custody, never a fresh current token.
        }
        ensure!(
            expected.store_generation() == inner.generation,
            "stale selected intent generation"
        );
        let selection = publication::current(
            &self.generation(inner.generation),
            inner.generation,
            expected.scope(),
        )?
        .context("selected intent scope is absent")?;
        ensure!(
            publication::token(&selection)? == *expected,
            "stale selected intent token"
        );
        let mut attempt = SelectedAttempt {
            format: FORMAT,
            actor: self.actor_id,
            request,
            selection,
            request_sha256: String::new(),
            state: SelectedAttemptState::Pending,
            mode_epoch: sync::mode::shared_epoch(self, expected.scope())?,
        };
        attempt.request_sha256 = attempt.fingerprint()?;
        attempt.validate(self.actor_id)?;
        verify(self, inner.generation, &attempt)?;
        Self::trip(&mut inner, Fault::BeforeCommit)?;
        persist(self, &attempt, &entries)?;
        Self::trip(&mut inner, Fault::AfterCommit)?;
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = &*wake {
                let _ = wake.try_send(());
            }
        }
        Ok(attempt)
    }
    /// Enumerate complete bounded journal custody in operation UUID order.
    /// Stale and withdrawn intents remain historical evidence; this validates
    /// metadata only and grants no current, provider or native authority.
    pub fn selected_attempt_inventory(&self) -> Result<Vec<SelectedAttempt>> {
        let _inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        Ok(inventory(self)?.into_values().collect())
    }

    /// Journal inspection is historical custody only, including withdrawn intents.
    pub fn inspect_selected_attempt(&self, operation: Uuid) -> Result<Option<SelectedAttempt>> {
        ensure!(!operation.is_nil(), "nil selected intent operation");
        let _inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        Ok(inventory(self)?.remove(&operation))
    }
    /// Match original pins and verify bytes. This is not an unguarded permission
    /// to send: a worker must hold current mode/binding/domain admission at handoff.
    pub fn current_selected_attempt(
        &self,
        expected: &SelectionToken,
        operation: Uuid,
    ) -> Result<SelectedAttempt> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let attempt = inventory(self)?
            .remove(&operation)
            .context("selected intent is absent")?;
        ensure!(
            attempt.state == SelectedAttemptState::Pending,
            "selected intent is withdrawn"
        );
        ensure!(
            publication::token(&attempt.selection)? == *expected
                && expected.store_generation() == inner.generation,
            "selected intent binding/generation mismatch"
        );
        let current = publication::current(
            &self.generation(inner.generation),
            inner.generation,
            expected.scope(),
        )?
        .context("selected intent scope is absent")?;
        ensure!(
            publication::token(&current)? == *expected,
            "stale selected intent token"
        );
        ensure!(
            sync::mode::shared_epoch(self, expected.scope())? == attempt.mode_epoch,
            "selected intent mode epoch mismatch"
        );
        verify(self, inner.generation, &attempt)?;
        Ok(attempt)
    }
    /// Explicit local withdrawal, never an inference of remote acceptance or loss.
    pub fn withdraw_selected_attempt(&self, operation: Uuid) -> Result<SelectedAttempt> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let entries = inventory(self)?;
        let mut attempt = entries
            .get(&operation)
            .context("selected intent is absent")?
            .clone();
        if attempt.state != SelectedAttemptState::Withdrawn {
            attempt.state = SelectedAttemptState::Withdrawn;
            Self::trip(&mut inner, Fault::BeforeCommit)?;
            persist(self, &attempt, &entries)?;
            Self::trip(&mut inner, Fault::AfterCommit)?;
        }
        Ok(attempt)
    }
}
