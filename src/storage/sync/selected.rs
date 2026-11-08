//! Selected branch of the existing worker; fixed-slot custody, not another loop.
use super::*;
use crate::conversation::{shared, SelectedAdmission};
use crate::storage::drive::slot::{
    AgreedSlotObservation, AgreedSlotValue, SlotBinding, SlotInvalid,
};
use crate::storage::selection::{
    SelectedAttemptRequest, SelectedAttemptState, SelectionScope, SelectionToken,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
mod catalog;
mod control;
mod corrections;
pub(crate) use corrections::run as run_handwriting;
mod protocol;
pub(crate) use control::{create_group, CreateGroupRequest, GroupOutcome};
mod preference_command;
mod preference_consumer;
pub(crate) use preference_consumer::{NotebookPhase, NotebookSelectionChanged, RequestPreferences};
#[cfg(target_os = "linux")]
pub(crate) use preference_consumer::{ReaderPhase, ReaderPreferences};
mod preferences;
pub use preference_command::{
    run as run_preferences, Operation as PreferenceOperation, Report as PreferenceReport,
};
mod publication;
mod receiving;
pub(crate) use receiving::verify_received_intents;
#[cfg(test)]
mod tests;
use protocol::*;

/// Cannot be deserialized from Config or an operator payload. Production
/// construction awaits separately accepted actual provider qualification.
#[derive(Clone)]
pub(super) struct Binding {
    group: Uuid,
    root_id: String,
    digest: String,
}
impl Binding {
    pub(super) fn qualified(_mode: &mode::SyncMode) -> Result<Self> {
        anyhow::bail!("shared provider application binding is not qualified")
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SelectedState {
    pub format: u32,
    pub epochs: BTreeMap<Uuid, Checkpoint>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub controls: BTreeMap<Uuid, control::GroupControl>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    format: u32,
    actor: Uuid,
    epoch: Uuid,
    group: Uuid,
    root_id: String,
    binding: String,
    root: Option<Recorded>,
    catalog: Vec<Recorded>,
    scopes: BTreeMap<String, Progress>,
    uploads: BTreeMap<String, Upload>,
    catalog_pending: Option<CatalogPending>,
    retired_catalog: Vec<CatalogProposal>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    slot: String,
    descriptor_id: String,
    sha256: String,
    bytes: Vec<u8>,
}
impl Recorded {
    fn from(value: AgreedSlotValue) -> Result<Self> {
        ensure!(
            digest(&value.descriptor) == value.pointer.descriptor_sha256,
            "recorded pointer digest differs"
        );
        Ok(Self {
            slot: value.pointer.binding.slot_id,
            descriptor_id: value.pointer.descriptor_id,
            sha256: value.pointer.descriptor_sha256,
            bytes: value.descriptor,
        })
    }
    fn check(&self, slot: &str) -> Result<()> {
        ensure!(
            self.slot == slot
                && !self.bytes.is_empty()
                && self.bytes.len() <= 64 * 1024
                && digest(&self.bytes) == self.sha256,
            "checkpoint descriptor identity differs"
        );
        drive::id(&self.descriptor_id)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogPending {
    operation: Uuid,
    source_generation: Uuid,
    registration: Option<crate::memory::catalog::Registration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    portable: Option<PortableRegistration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handwriting: Option<HandwritingRegistration>,
    genesis_slot: Option<String>,
    proposal: Option<CatalogProposal>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableRegistration {
    genesis_operation: Uuid,
    genesis_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandwritingRegistration {
    owner: HandwritingOwner,
    genesis_operation: Uuid,
    genesis_sha256: String,
}
impl CatalogPending {
    fn owner(&self) -> Result<Owner> {
        match (&self.registration, &self.portable, &self.handwriting) {
            (Some(memory), None, None) => Ok(Owner::Memory(memory.owner.clone())),
            (None, Some(_), None) => Ok(Owner::preferences()),
            (None, None, Some(correction)) => Ok(Owner::Handwriting(correction.owner)),
            _ => anyhow::bail!("catalog original source is ambiguous"),
        }
    }
    fn key(&self) -> Result<String> {
        self.owner()?.key_sha256()
    }
    fn genesis_operation(&self) -> Result<Uuid> {
        match (&self.registration, &self.portable, &self.handwriting) {
            (Some(memory), None, None) => Ok(memory.genesis_operation),
            (None, Some(portable), None) => Ok(portable.genesis_operation),
            (None, None, Some(correction)) => Ok(correction.genesis_operation),
            _ => anyhow::bail!("catalog original source is ambiguous"),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogProposal {
    key: String,
    slot: String,
    base: String,
    bytes: Vec<u8>,
    descriptor: Option<Recorded>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    chain: Vec<Recorded>,
    pending: Option<Pending>,
    receive: Option<Receive>,
    applied: Option<Applied>,
    receipts: Vec<Receive>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_pending: Vec<Pending>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pins {
    store: Uuid,
    aggregate: Uuid,
    selection: String,
    base: String,
}
impl Pins {
    fn from(token: &SelectionToken) -> Self {
        Self {
            store: token.store_generation(),
            aggregate: token.aggregate_generation(),
            selection: token.selection_sha256().into(),
            base: token.accepted_base_sha256().into(),
        }
    }
    fn matches(&self, token: &SelectionToken) -> bool {
        self.store == token.store_generation()
            && self.aggregate == token.aggregate_generation()
            && self.selection == token.selection_sha256()
            && self.base == token.accepted_base_sha256()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    operation: Uuid,
    selection_operation: Uuid,
    pins: Pins,
    slot: String,
    next_slot: Option<String>,
    descriptor: Option<Recorded>,
    // None is entered-but-unacknowledged preparation or legacy unknown; neither
    // proves no submission. Only persisted Some(false) means never entered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outbox_prepared: Option<bool>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receive {
    operation: Uuid,
    expected: Option<Pins>,
    descriptor: Recorded,
    manifest: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manifest_reference: Option<ObjectRef>,
    original_attempt: Option<Uuid>,
    applied: bool,
    #[serde(default)]
    verified: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Applied {
    descriptor: String,
    operation: Uuid,
    membership: String,
}

pub(super) struct Engine {
    store: Arc<Store>,
    policy: SyncPolicy,
    transport: GoogleDrive,
    binding: Binding,
    cancel: Arc<AtomicBool>,
    state: State,
    checkpoint: Checkpoint,
}
struct CatalogView {
    root: Root,
    root_sha: String,
    entries: BTreeMap<String, Entry>,
    base: String,
    slot: String,
    complete: bool,
}
fn stronger_scope_status(current: Status, next: Status) -> Status {
    let priority = |status| match status {
        Status::Current => 0,
        Status::Joining => 1,
        Status::Pending => 2,
        Status::IncompleteMedia => 3,
        Status::CommittedNotApplied => 4,
        _ => 5,
    };
    if priority(next) > priority(current) {
        next
    } else {
        current
    }
}

impl Engine {
    pub(super) fn step(&mut self) -> Result<Status> {
        let _mode = self.current_lease()?;
        self.transport.set_request_window(
            Some(Instant::now() + Duration::from_secs(60)),
            Some(self.cancel.clone()),
        );
        let result = match self.step_inner() {
            Err(error) if error.is::<SlotInvalid>() => Ok(Status::UnsupportedOrCorrupt),
            result => result,
        };
        self.transport.set_request_window(None, None);
        result
    }
    fn step_inner(&mut self) -> Result<Status> {
        let mut budget = self.policy.batch_items;
        let Some(view) = self.refresh_root_catalog(&mut budget)? else {
            return Ok(Status::Joining);
        };
        if self.checkpoint.catalog.is_empty() {
            return Ok(Status::Joining);
        }
        if !view.complete {
            return Ok(Status::Pending);
        }
        if self.catalog_step(&view, &mut budget)? {
            return Ok(Status::Pending);
        }
        let mut status = Status::Current;
        for (key, entry) in &view.entries {
            if budget == 0 {
                return Ok(stronger_scope_status(status, Status::Pending));
            }
            self.checkpoint.scopes.entry(key.clone()).or_default();
            let next = self.scope_step(key, entry, &view, &mut budget)?;
            status = stronger_scope_status(status, next);
        }
        self.save()?;
        Ok(status)
    }
    pub(super) fn health(&self, status: Status) -> serde_json::Value {
        let mut status = status;
        let scopes: Vec<_> = self.checkpoint.scopes.iter().map(|(key, progress)| {
            let scope = SelectionScope {
                group: self.binding.group,
                key_sha256: key.clone(),
                binding_sha256: self.binding.digest.clone(),
            };
            let applied = progress.applied.as_ref().and_then(|applied| {
                let receipt = self.store.selected_receipt(&scope, applied.operation).ok()??;
                (receipt.accepted_base_sha256 == applied.descriptor
                    && receiving::membership(&receipt.selected).ok().as_ref() == Some(&applied.membership))
                    .then_some(receipt)
            });
            if status == Status::Current && progress.applied.is_some() && applied.is_none() {
                status = Status::RecoveryRequired;
            }
            let candidate = progress.receive.as_ref();
            if candidate.is_some_and(|receive| receive.verified && !receive.applied)
                && matches!(status, Status::Current | Status::Joining | Status::Pending)
            {
                status = Status::CommittedNotApplied;
            }
            let committed = candidate.filter(|receive| receive.verified || receive.applied)
                .map(|receive| &receive.descriptor.sha256)
                .or_else(|| progress.chain.last().map(|record| &record.sha256));
            let media = candidate.and_then(|receive| receive.manifest.as_ref())
                .or_else(|| applied.as_ref().map(|receipt| &receipt.selected))
                .map(|manifest| json!({
                    "included": manifest.media.len(),
                    "omitted": manifest.media_coverage.iter().filter(|coverage| matches!(coverage, Coverage::Omitted { .. })).count(),
                }));
            json!({
                "key": key,
                "observed_candidate": candidate.map(|receive| &receive.descriptor.sha256),
                "committed": committed,
                "applied": applied.as_ref().map(|receipt| &receipt.accepted_base_sha256),
                "applied_local": applied.as_ref().map(|receipt| json!({
                    "operation": receipt.operation,
                    "store_generation": receipt.store_generation,
                    "aggregate_generation": receipt.aggregate_generation,
                    "manifest": receipt.selected.transaction_id,
                })),
                "pending_original": progress.pending.as_ref().map(|pending| pending.operation),
                "receiving_original": candidate.map(|receive| receive.operation),
                "media": media,
            })
        }).collect();
        json!({"report_version":1,"status":status,"mode":"shared-group","group":self.binding.group,
            "root":self.binding.root_id,"binding_sha256":self.binding.digest,"scopes":scopes,
            "production_qualification":"unexecuted"})
    }
    pub(super) fn new(
        store: Arc<Store>,
        policy: SyncPolicy,
        transport: GoogleDrive,
        binding: Binding,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        policy.validate()?;
        ensure!(
            transport.binding_digest() == binding.digest,
            "selected provider context differs"
        );
        let current = store.sync_mode_snapshot()?;
        let epoch = current.epoch.context("shared mode epoch absent")?;
        require_mode(&current, &binding)?;
        let path = store.paths.data.join("sync/state.json");
        let state: State = if path.exists() {
            files::json(&path, MAX_METADATA as u64)?
        } else {
            State {
                format: FORMAT,
                ..Default::default()
            }
        };
        ensure!(state.format == FORMAT, "worker checkpoint format differs");
        if let Some(selected) = &state.selected {
            control::validate_controls(selected, store.actor_id)?;
        }
        let checkpoint = state
            .selected
            .as_ref()
            .and_then(|s| s.epochs.get(&epoch))
            .cloned()
            .unwrap_or_else(|| Checkpoint {
                format: 1,
                actor: store.actor_id,
                epoch,
                group: binding.group,
                root_id: binding.root_id.clone(),
                binding: binding.digest.clone(),
                root: None,
                catalog: vec![],
                scopes: BTreeMap::new(),
                uploads: BTreeMap::new(),
                catalog_pending: None,
                retired_catalog: vec![],
            });
        ensure!(
            checkpoint.format == 1
                && checkpoint.actor == store.actor_id
                && checkpoint.epoch == epoch
                && checkpoint.group == binding.group
                && checkpoint.root_id == binding.root_id
                && checkpoint.binding == binding.digest,
            "selected checkpoint binding differs"
        );
        let engine = Self {
            store,
            policy,
            transport,
            binding,
            cancel,
            state,
            checkpoint,
        };
        engine.check_bounds()?;
        Ok(engine)
    }
    fn check_bounds(&self) -> Result<()> {
        ensure!(
            self.checkpoint.scopes.len() <= crate::memory::catalog::MAX_AGGREGATES
                && self.checkpoint.uploads.len() <= MAX_ITEMS
                && self.checkpoint.catalog.len() <= MAX_ITEMS
                && self.checkpoint.retired_catalog.len() <= MAX_ITEMS
                && self
                    .checkpoint
                    .scopes
                    .values()
                    .all(|p| p.chain.len() <= MAX_ITEMS && p.retired_pending.len() <= MAX_ITEMS),
            "selected checkpoint item bound"
        );
        ensure!(
            serde_json::to_vec(&self.checkpoint)?.len() <= MAX_METADATA,
            "selected checkpoint byte bound"
        );
        catalog::validate_checkpoint(&self.checkpoint, &self.binding)?;
        Ok(())
    }
    fn save(&mut self) -> Result<()> {
        self.check_bounds()?;
        let selected = self.state.selected.get_or_insert_with(|| SelectedState {
            format: 1,
            ..Default::default()
        });
        ensure!(
            selected.epochs.len() < MAX_ITEMS
                || selected.epochs.contains_key(&self.checkpoint.epoch),
            "selected checkpoint epoch bound"
        );
        selected
            .epochs
            .insert(self.checkpoint.epoch, self.checkpoint.clone());
        ensure!(
            serde_json::to_vec(&self.state)?.len() <= MAX_METADATA,
            "worker checkpoint byte bound"
        );
        files::atomic_json(&self.store.paths.data.join("sync/state.json"), &self.state)
    }
    fn current_lease(&self) -> Result<mode::SyncModeLease> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "selected worker canceled"
        );
        let snapshot = self.store.sync_mode_snapshot()?;
        require_mode(&snapshot, &self.binding)?;
        ensure!(
            snapshot.epoch == Some(self.checkpoint.epoch),
            "selected epoch changed"
        );
        ensure!(
            self.transport.binding_digest() == self.binding.digest,
            "selected provider changed"
        );
        self.store.sync_mode_lease(&snapshot.token)
    }
    fn slot(&self, key: String, slot: String) -> SlotBinding {
        SlotBinding {
            group: self.binding.group,
            scope_sha256: key,
            slot_id: slot,
        }
    }
    fn observe(&mut self, key: String, slot: String) -> Result<Option<Recorded>> {
        match self.transport.observe_agreed_slot(&self.slot(key, slot))? {
            AgreedSlotObservation::Present(value) => Ok(Some(Recorded::from(value)?)),
            AgreedSlotObservation::Absent => Ok(None),
            AgreedSlotObservation::Invalid(reason) => Err(reason.into()),
        }
    }
    fn view(&self) -> Result<Option<CatalogView>> {
        catalog_view(&self.binding, &self.checkpoint)
    }
    fn refresh_root_catalog(&mut self, budget: &mut usize) -> Result<Option<CatalogView>> {
        self.refresh_root_catalog_bounded(budget, None)
    }
    fn observe_bounded(
        &mut self,
        key: String,
        slot: String,
        reads: &mut Option<&mut crate::storage::selection::SelectedReadBudget>,
    ) -> Result<Option<Recorded>> {
        if let Some(reads) = reads {
            // Account identity and two metadata responses plus the 64 KiB descriptor. Charge
            // their worst case before any GET, including failed observations.
            reads.charge(0, 1, 3 * MAX_METADATA as u64 + 64 * 1024, 0)?;
        }
        self.observe(key, slot)
    }
    fn refresh_root_catalog_bounded(
        &mut self,
        budget: &mut usize,
        mut reads: Option<&mut crate::storage::selection::SelectedReadBudget>,
    ) -> Result<Option<CatalogView>> {
        let Some(root) = self.observe_bounded(
            root_key(self.binding.group),
            self.binding.root_id.clone(),
            &mut reads,
        )?
        else {
            return Ok(None);
        };
        let parsed: Root = serde_json::from_slice(&root.bytes)?;
        parsed.validate(&self.binding)?;
        if let Some(prior) = &self.checkpoint.root {
            ensure!(
                prior.sha256 == root.sha256 && prior.bytes == root.bytes,
                "selected root changed"
            );
        } else {
            self.checkpoint.root = Some(root);
            self.save()?;
        }
        let mut view = self.view()?.context("validated root absent")?;
        if let Some(previous) = self.checkpoint.catalog.last().cloned() {
            let current = self
                .observe_bounded(
                    catalog_key(self.binding.group),
                    previous.slot.clone(),
                    &mut reads,
                )?
                .context("accepted catalog pointer not visible")?;
            ensure!(
                current.sha256 == previous.sha256 && current.bytes == previous.bytes,
                "accepted catalog changed"
            );
        }
        while *budget > 0 {
            let Some(recorded) = self.observe_bounded(
                catalog_key(self.binding.group),
                view.slot.clone(),
                &mut reads,
            )?
            else {
                view.complete = true;
                break;
            };
            *budget -= 1;
            let catalog: Catalog = serde_json::from_slice(&recorded.bytes)?;
            catalog.validate(
                &view.root,
                &view.root_sha,
                &view.slot,
                &view.base,
                &view.entries,
            )?;
            self.checkpoint.catalog.push(recorded);
            self.save()?;
            view = self.view()?.context("catalog view absent")?;
        }
        Ok(Some(view))
    }
}
fn require_mode(snapshot: &mode::SyncModeSnapshot, binding: &Binding) -> Result<()> {
    ensure!(
        matches!(&snapshot.mode, Some(mode::SyncMode::SharedGroup { group, binding_sha256, root_id })
        if *group == binding.group && *binding_sha256 == binding.digest && *root_id == binding.root_id),
        "shared mode context differs"
    );
    Ok(())
}

fn catalog_view(binding: &Binding, checkpoint: &Checkpoint) -> Result<Option<CatalogView>> {
    let Some(recorded) = &checkpoint.root else {
        return Ok(None);
    };
    recorded.check(&binding.root_id)?;
    let root: Root = serde_json::from_slice(&recorded.bytes)?;
    root.validate(binding)?;
    let root_sha = recorded.sha256.clone();
    let mut base = root_sha.clone();
    let mut slot = root.catalog_slot.clone();
    let mut entries = BTreeMap::new();
    let mut slots = BTreeSet::new();
    for recorded in &checkpoint.catalog {
        ensure!(slots.insert(slot.clone()), "catalog slot cycle");
        recorded.check(&slot)?;
        let catalog: Catalog = serde_json::from_slice(&recorded.bytes)?;
        catalog.validate(&root, &root_sha, &slot, &base, &entries)?;
        entries = catalog.entries;
        base = recorded.sha256.clone();
        slot = catalog.next_slot;
    }
    Ok(Some(CatalogView {
        root,
        root_sha,
        entries,
        base,
        slot,
        complete: false,
    }))
}
