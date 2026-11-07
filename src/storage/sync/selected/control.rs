//! Explicit root control. Ordinary admission permits allocation, not publication.
//! Original custody stays in the existing worker checkpoint, never another journal.
use super::*;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedMode {
    pub actor: Uuid,
    pub sha256: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateGroupRequest {
    pub control_version: u32,
    pub operation: Uuid,
    pub expected: ExpectedMode,
    pub group: Uuid,
    pub binding_sha256: String,
    pub profile: Uuid,
}
impl CreateGroupRequest {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.control_version == 1
                && !self.operation.is_nil()
                && !self.group.is_nil()
                && !self.profile.is_nil()
                && !self.expected.actor.is_nil()
                && valid_digest(&self.expected.sha256)
                && valid_digest(&self.binding_sha256),
            "invalid CreateGroup request"
        );
        Ok(())
    }
    fn token(&self) -> Result<mode::SyncModeToken> {
        mode::SyncModeToken::from_maintenance_request(
            self.expected.actor,
            self.expected.sha256.clone(),
        )
    }
}

/// This cannot be supplied by JSON, a saved mode or production fixture option.
struct Application {
    group: Uuid,
    digest: String,
}
impl Application {
    fn qualified(_request: &CreateGroupRequest) -> Result<Self> {
        anyhow::bail!("shared provider application binding is not qualified")
    }
    fn check(&self, request: &CreateGroupRequest, transport: &GoogleDrive) -> Result<()> {
        ensure!(
            self.group == request.group
                && self.digest == request.binding_sha256
                && transport.binding_digest() == self.digest,
            "CreateGroup application binding differs"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::storage::sync) struct GroupControl {
    format: u32,
    actor: Uuid,
    source_generation: Uuid,
    request: CreateGroupRequest,
    request_sha256: String,
    ids: Vec<String>,
    root: Option<Recorded>,
    catalog: Option<Recorded>,
    mode: Option<mode::SyncModeReceipt>,
    root_verified: bool,
    accepted_catalog: Option<Recorded>,
}
impl GroupControl {
    fn validate(&self, actor: Uuid) -> Result<()> {
        self.request.validate()?;
        ensure!(
            self.format == 1
                && self.actor == actor
                && self.request.expected.actor == actor
                && !self.source_generation.is_nil()
                && self.request_sha256 == digest(&serde_json::to_vec(&self.request)?)
                && self.ids.len() <= 5,
            "CreateGroup original identity differs"
        );
        let mut ids = BTreeSet::new();
        for id in &self.ids {
            drive::id(id)?;
            ensure!(ids.insert(id), "CreateGroup repeated provider ID");
        }
        ensure!(
            self.root.is_some() == self.catalog.is_some()
                && (self.mode.is_none() || self.root.is_some())
                && (!self.root_verified || self.mode.is_some())
                && (self.accepted_catalog.is_none() || self.root_verified),
            "CreateGroup phase differs"
        );
        if let Some(root) = &self.root {
            ensure!(
                self.ids.len() == 5,
                "CreateGroup body precedes complete IDs"
            );
            root.check(&self.ids[0])?;
            ensure!(
                root.descriptor_id == self.ids[1],
                "CreateGroup original descriptor differs"
            );
            let body: Root = serde_json::from_slice(&root.bytes)?;
            body.validate(&Binding {
                group: self.request.group,
                root_id: self.ids[0].clone(),
                digest: self.request.binding_sha256.clone(),
            })?;
            ensure!(
                body.profile == self.request.profile && body.catalog_slot == self.ids[2],
                "CreateGroup body differs"
            );
            let catalog = self
                .catalog
                .as_ref()
                .context("CreateGroup catalog body absent")?;
            catalog.check(&self.ids[2])?;
            let initial: Catalog = serde_json::from_slice(&catalog.bytes)?;
            initial.validate(
                &body,
                &root.sha256,
                &self.ids[2],
                &root.sha256,
                &BTreeMap::new(),
            )?;
            ensure!(
                catalog.descriptor_id == self.ids[3]
                    && initial.entries.is_empty()
                    && initial.next_slot == self.ids[4]
                    && initial.operation
                        == Uuid::new_v5(&self.request.operation, b"shared-initial-catalog:v1"),
                "CreateGroup initial catalog differs"
            );
            if let Some(accepted) = &self.accepted_catalog {
                accepted.check(&self.ids[2])?;
                let accepted_body: Catalog = serde_json::from_slice(&accepted.bytes)?;
                accepted_body.validate(
                    &body,
                    &root.sha256,
                    &self.ids[2],
                    &root.sha256,
                    &BTreeMap::new(),
                )?;
            }
        }
        if let Some(receipt) = &self.mode {
            ensure!(
                self.ids.len() == 5
                    && receipt.operation == self.request.operation
                    && !receipt.epoch.is_nil()
                    && matches!(&receipt.mode,mode::SyncMode::SharedGroup {group,binding_sha256,root_id}
                    if *group==self.request.group && *binding_sha256==self.request.binding_sha256 && *root_id==self.ids[0]),
                "CreateGroup selected receipt differs"
            );
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub(crate) struct GroupOutcome {
    pub operation: Uuid,
    pub root_id: String,
    pub root_sha256: String,
    pub bootstrap_complete: bool,
    #[serde(skip)]
    pub mode_receipt: mode::SyncModeReceipt,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Original,
    Allocated,
    Bodies,
    BeforeMode,
    Mode,
    Root,
    Catalog,
}

fn ready(store: &Store) -> Result<()> {
    let inner = store
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("Store mutex unavailable"))?;
    crate::storage::compound_generation::validate_ready(
        &store.paths,
        store.actor_id,
        inner.generation,
    )
}
pub(crate) fn create_group(store: Arc<Store>, request: CreateGroupRequest) -> Result<GroupOutcome> {
    request.validate()?;
    ready(&store)?;
    // Qualification is before credential loading or checkpoint mutation.
    let application = Application::qualified(&request)?;
    let transport = GoogleDrive::open(store.paths.credentials.join("drive.json"))?;
    Controller::new(store, transport, application)?.run(request, |_| Ok(()))
}

struct Controller {
    store: Arc<Store>,
    transport: GoogleDrive,
    application: Application,
    state: State,
}
impl Controller {
    fn new(store: Arc<Store>, transport: GoogleDrive, application: Application) -> Result<Self> {
        ready(&store)?;
        let path = store.paths.data.join("sync/state.json");
        let state: State = if path.exists() {
            files::json(&path, MAX_METADATA as u64)?
        } else {
            State {
                format: FORMAT,
                ..Default::default()
            }
        };
        ensure!(
            state.format == FORMAT,
            "CreateGroup worker checkpoint format differs"
        );
        if let Some(selected) = &state.selected {
            validate_controls(selected, store.actor_id)?;
        }
        Ok(Self {
            store,
            transport,
            application,
            state,
        })
    }
    fn generation(&self) -> Result<Uuid> {
        let inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Store mutex unavailable"))?;
        Ok(inner.generation)
    }
    fn persist(&mut self, control: &GroupControl) -> Result<()> {
        control.validate(self.store.actor_id)?;
        let selected = self.state.selected.get_or_insert_with(|| SelectedState {
            format: 1,
            ..Default::default()
        });
        ensure!(
            selected.controls.len() < MAX_ITEMS
                || selected.controls.contains_key(&control.request.operation),
            "CreateGroup control bound"
        );
        selected
            .controls
            .insert(control.request.operation, control.clone());
        ensure!(
            serde_json::to_vec(&self.state)?.len() <= MAX_METADATA,
            "CreateGroup checkpoint byte bound"
        );
        files::atomic_json(&self.store.paths.data.join("sync/state.json"), &self.state)
    }
    fn run(
        &mut self,
        request: CreateGroupRequest,
        hook: impl FnMut(Stage) -> Result<()>,
    ) -> Result<GroupOutcome> {
        self.transport.set_request_window(
            Some(Instant::now() + Duration::from_secs(60)),
            Some(Arc::new(AtomicBool::new(false))),
        );
        let result = self.run_inner(request, hook);
        self.transport.set_request_window(None, None);
        result
    }
    fn run_inner(
        &mut self,
        request: CreateGroupRequest,
        mut hook: impl FnMut(Stage) -> Result<()>,
    ) -> Result<GroupOutcome> {
        request.validate()?;
        self.application.check(&request, &self.transport)?;
        ensure!(
            request.expected.actor == self.store.actor_id,
            "CreateGroup actor differs"
        );
        let expected = request.token()?;
        let mut control = match self
            .state
            .selected
            .as_ref()
            .and_then(|s| s.controls.get(&request.operation))
            .cloned()
        {
            Some(original) => {
                ensure!(
                    original.request == request,
                    "CreateGroup operation collision"
                );
                original
            }
            None => {
                let _ordinary = self.store.ordinary_mode_lease(&expected)?;
                let original = GroupControl {
                    format: 1,
                    actor: self.store.actor_id,
                    source_generation: self.generation()?,
                    request: request.clone(),
                    request_sha256: digest(&serde_json::to_vec(&request)?),
                    ids: vec![],
                    root: None,
                    catalog: None,
                    mode: None,
                    root_verified: false,
                    accepted_catalog: None,
                };
                self.persist(&original)?;
                hook(Stage::Original)?;
                original
            }
        };
        control.validate(self.store.actor_id)?;
        ensure!(
            self.generation()? == control.source_generation,
            "CreateGroup source generation changed"
        );
        if control.ids.len() < 5 {
            let _ordinary = self.store.ordinary_mode_lease(&expected)?;
            self.application.check(&request, &self.transport)?;
            self.transport.authorize(request.group)?;
            while control.ids.len() < 5 {
                let id = self.transport.allocate()?;
                ensure!(
                    !control.ids.contains(&id),
                    "CreateGroup allocator repeated ID"
                );
                control.ids.push(id);
                self.persist(&control)?;
                hook(Stage::Allocated)?;
            }
        }
        if control.root.is_none() {
            let root = Root {
                schema: WIRE,
                group: request.group,
                protocol: "buddy-shared-fixed-slot-v1".into(),
                binding_sha256: request.binding_sha256.clone(),
                profile: request.profile,
                catalog_slot: control.ids[2].clone(),
            };
            let bytes = serde_json::to_vec(&root)?;
            let sha = digest(&bytes);
            control.root = Some(Recorded {
                slot: control.ids[0].clone(),
                descriptor_id: control.ids[1].clone(),
                sha256: sha.clone(),
                bytes,
            });
            let catalog = Catalog {
                schema: WIRE,
                group: request.group,
                root_sha256: sha.clone(),
                base_sha256: sha,
                slot: control.ids[2].clone(),
                operation: Uuid::new_v5(&request.operation, b"shared-initial-catalog:v1"),
                entries: BTreeMap::new(),
                next_slot: control.ids[4].clone(),
            };
            let bytes = serde_json::to_vec(&catalog)?;
            control.catalog = Some(Recorded {
                slot: control.ids[2].clone(),
                descriptor_id: control.ids[3].clone(),
                sha256: digest(&bytes),
                bytes,
            });
            self.persist(&control)?;
            hook(Stage::Bodies)?;
        }
        ensure!(
            self.generation()? == control.source_generation,
            "CreateGroup source changed before mode"
        );
        // Ordinary allocation custody ended before conditional mode selection.
        hook(Stage::BeforeMode)?;
        let receipt = self.store.set_sync_mode(
            &expected,
            request.operation,
            mode::SyncModeChoice::SharedGroup {
                group: request.group,
                binding_sha256: request.binding_sha256.clone(),
                root_id: control.ids[0].clone(),
            },
        )?;
        let current = self.store.sync_mode_snapshot()?;
        ensure!(
            current.epoch == Some(receipt.epoch) && current.mode.as_ref() == Some(&receipt.mode),
            "CreateGroup original mode is historical"
        );
        control.mode = Some(receipt.clone());
        self.persist(&control)?;
        hook(Stage::Mode)?;
        let _shared = self.store.sync_mode_lease(&current.token)?;
        self.application.check(&request, &self.transport)?;
        let root = control
            .root
            .as_ref()
            .context("CreateGroup root body absent")?
            .clone();
        let observed = self.publish(root_key(request.group), &root)?;
        ensure!(
            observed.sha256 == root.sha256 && observed.bytes == root.bytes,
            "CreateGroup original root differs"
        );
        control.root_verified = true;
        self.persist(&control)?;
        hook(Stage::Root)?;
        let catalog = control
            .catalog
            .as_ref()
            .context("CreateGroup initial catalog absent")?
            .clone();
        let observed = self.publish(catalog_key(request.group), &catalog)?;
        let body: Root = serde_json::from_slice(&root.bytes)?;
        let accepted: Catalog = serde_json::from_slice(&observed.bytes)?;
        accepted.validate(
            &body,
            &root.sha256,
            &catalog.slot,
            &root.sha256,
            &BTreeMap::new(),
        )?;
        control.accepted_catalog = Some(observed);
        self.persist(&control)?;
        hook(Stage::Catalog)?;
        Ok(GroupOutcome {
            operation: request.operation,
            root_id: root.slot,
            root_sha256: root.sha256,
            bootstrap_complete: true,
            mode_receipt: receipt,
        })
    }
    fn publish(&mut self, key: String, original: &Recorded) -> Result<Recorded> {
        let binding = SlotBinding {
            group: self.application.group,
            scope_sha256: key,
            slot_id: original.slot.clone(),
        };
        match self.transport.observe_agreed_slot(&binding)? {
            AgreedSlotObservation::Present(value) => return Recorded::from(value),
            AgreedSlotObservation::Invalid(_) => anyhow::bail!("CreateGroup occupied slot invalid"),
            AgreedSlotObservation::Absent => {}
        }
        let reference = ObjectRef {
            sha256: original.sha256.clone(),
            bytes: original.bytes.len() as u64,
        };
        let tag = Tag {
            collection: self.application.group,
            kind: "object".into(),
            sha256: reference.sha256.clone(),
        };
        let progress = self.transport.upload(
            &original.descriptor_id,
            &tag,
            &mut files::Source::Memory(original.bytes.clone()),
            &mut UploadSession::default(),
        )?;
        ensure!(
            matches!(progress, UploadProgress::Complete),
            "CreateGroup descriptor upload incomplete"
        );
        let prepared =
            self.transport
                .prepare_slot(binding, original.descriptor_id.clone(), reference)?;
        let observed = self.transport.create_or_read_slot(&prepared)?;
        Recorded::from(AgreedSlotValue {
            pointer: observed.pointer,
            descriptor: observed.descriptor,
        })
    }
}
pub(super) fn validate_controls(selected: &SelectedState, actor: Uuid) -> Result<()> {
    ensure!(
        selected.format == 1
            && selected.controls.len() <= MAX_ITEMS
            && selected.epochs.len() <= MAX_ITEMS,
        "CreateGroup checkpoint bounds differ"
    );
    for (operation, control) in &selected.controls {
        control.validate(actor)?;
        ensure!(
            *operation == control.request.operation,
            "CreateGroup checkpoint key differs"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
