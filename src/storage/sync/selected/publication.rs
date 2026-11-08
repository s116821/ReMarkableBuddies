use super::*;
use crate::storage::drive::slot::{AgreedSlotValue, MissingPreparedDescriptor};

impl Engine {
    pub(super) fn upload(
        &mut self,
        bytes: Vec<u8>,
        budget: &mut usize,
    ) -> Result<Option<(String, ObjectRef)>> {
        let reference = ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        let tag = Tag {
            collection: self.binding.group,
            kind: "object".into(),
            sha256: reference.sha256.clone(),
        };
        if let Some(upload) = self.checkpoint.uploads.get(&reference.sha256) {
            ensure!(upload.tag == tag, "original upload binding differs");
            if upload.complete {
                return Ok(Some((upload.id.clone(), reference)));
            }
        }
        if *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        if !self.checkpoint.uploads.contains_key(&reference.sha256) {
            let id = self.transport.allocate()?;
            self.checkpoint.uploads.insert(
                reference.sha256.clone(),
                Upload {
                    id,
                    tag: tag.clone(),
                    complete: false,
                },
            );
            // Original provider ID is durable before its first possible upload.
            self.save()?;
        }
        let id = self.checkpoint.uploads[&reference.sha256].id.clone();
        let session_path = self.store.paths.credentials.join(format!(
            "shared-{}-{}.session.json",
            self.checkpoint.epoch, reference.sha256
        ));
        let mut session = if session_path.exists() {
            files::json(&session_path, MAX_RECORD as u64)?
        } else {
            UploadSession::default()
        };
        let result =
            self.transport
                .upload(&id, &tag, &mut files::Source::Memory(bytes), &mut session)?;
        files::atomic_json(&session_path, &session)?;
        if matches!(result, UploadProgress::Complete) {
            self.checkpoint
                .uploads
                .get_mut(&reference.sha256)
                .unwrap()
                .complete = true;
            self.save()?;
            Ok(Some((id, reference)))
        } else {
            Ok(None)
        }
    }
    pub(super) fn publish_scope(
        &mut self,
        entry: &Entry,
        view: &CatalogView,
        slot: String,
        base: String,
        budget: &mut usize,
        snapshot: Option<crate::storage::selection::SelectedSnapshot>,
    ) -> Result<Status> {
        let Some(snapshot) = snapshot else {
            return Ok(Status::Joining);
        };
        let scope = snapshot.token.scope().clone();
        let key = scope.key_sha256.as_str();
        if let Some(pending) = &self.checkpoint.scopes[key].pending {
            ensure!(
                pending.slot == slot,
                "original attempt cannot move to a successor"
            );
            if !pending.pins.matches(&snapshot.token) {
                return Ok(Status::Pending);
            }
        }
        let admission = SelectedAdmission::new(self.store.clone(), scope.clone())?;
        // Even an unchanged applied membership is not product readiness when
        // its exact local registration or typed ownership is unavailable.
        admission.with_current(&snapshot.token, |current| {
            match &entry.owner {
                Owner::Memory(owner) => {
                    ensure!(
                        shared::shared_owner(current, view.root.profile)? == *owner,
                        "local producer owner differs"
                    );
                }
                Owner::Preferences(_) => {
                    let mut budget = crate::storage::selection::SelectedReadBudget::new(
                        64,
                        MAX_ITEMS as u64,
                        MAX_METADATA as u64,
                        MAX_MEDIA,
                    );
                    if self.checkpoint.scopes[key].applied.is_none() {
                        preferences::require_initial_publication(
                            &self.store,
                            &scope,
                            &view.root_sha,
                            &mut budget,
                        )?;
                    } else {
                        preferences::project(
                            &self.store,
                            &scope,
                            &current.transaction.selected,
                            &mut budget,
                        )?;
                    }
                }
            }
            Ok(())
        })?;
        if self.checkpoint.scopes[key].pending.is_none() {
            let original_pending = self
                .store
                .selected_attempt_inventory()?
                .into_iter()
                .any(|a| a.state == SelectedAttemptState::Pending && a.selection.scope == scope);
            ensure!(
                !original_pending,
                "unreconciled original outbox precedes new publication"
            );
            let local_membership = receiving::membership(&snapshot.transaction.selected)?;
            if let Some(applied) = &self.checkpoint.scopes[key].applied {
                ensure!(
                    snapshot.token.accepted_base_sha256() == applied.descriptor,
                    "local scope has no applied remote base"
                );
                if local_membership == applied.membership {
                    return Ok(
                        if snapshot
                            .transaction
                            .selected
                            .media_coverage
                            .iter()
                            .any(|coverage| matches!(coverage, Coverage::Omitted { .. }))
                        {
                            Status::IncompleteMedia
                        } else {
                            Status::Current
                        },
                    );
                }
                ensure!(
                    snapshot.transaction.mutation
                        == crate::storage::selection::SelectionMutation::Commit,
                    "publication requires a genuine edit from the applied base"
                );
            } else {
                // A new scope is explicit local genesis at this root, never an
                // automatic copy of a previous group/binding's selected data.
                ensure!(
                    self.checkpoint.scopes[key].chain.is_empty()
                        && snapshot.token.accepted_base_sha256()
                            == genesis_base(&view.root_sha, key),
                    "initial scope is not admitted at this root genesis"
                );
            }
            self.checkpoint.scopes.get_mut(key).unwrap().pending = Some(Pending {
                operation: Uuid::new_v4(),
                selection_operation: snapshot.transaction.operation,
                pins: Pins::from(&snapshot.token),
                slot: slot.clone(),
                next_slot: None,
                descriptor: None,
                outbox_prepared: Some(false),
            });
            self.save()?;
        }
        let pending = self.checkpoint.scopes[key]
            .pending
            .clone()
            .context("original pending absent")?;
        let original = self
            .store
            .selected_receipt(&scope, pending.selection_operation)?
            .context("original selected producer receipt absent")?;
        ensure!(
            original.operation == snapshot.transaction.operation,
            "captured producer receipt differs"
        );
        if pending.descriptor.is_none() {
            let media_total = original.selected.media.iter().try_fold(0u64, |n, r| {
                n.checked_add(r.bytes).context("outbound media overflow")
            })?;
            ensure!(media_total <= MAX_MEDIA, "outbound media bound");
            // Validate the exact full outgoing closure with live/reopen notebook
            // semantics before any commit can be created. Staging is not admission.
            let mut original_objects = BTreeMap::new();
            for reference in original
                .selected
                .records
                .iter()
                .chain(&original.selected.media)
            {
                original_objects
                    .insert(reference.sha256.clone(), self.store.read_object(reference)?);
            }
            self.store
                .stage_selected_candidate(&original.selected, &original_objects)?;
            drop(original_objects);
            // No envelope is reserialized: each original reference selects bytes.
            for reference in original
                .selected
                .records
                .iter()
                .chain(&original.selected.media)
            {
                if self
                    .upload(self.store.read_object(reference)?, budget)?
                    .is_none()
                {
                    return Ok(Status::Pending);
                }
            }
            let Some((manifest_id, manifest)) =
                self.upload(serde_json::to_vec(&original.selected)?, budget)?
            else {
                return Ok(Status::Pending);
            };
            if self.checkpoint.scopes[key]
                .pending
                .as_ref()
                .unwrap()
                .next_slot
                .is_none()
            {
                if *budget == 0 {
                    return Ok(Status::Pending);
                }
                *budget -= 1;
                let next = self.transport.allocate()?;
                self.checkpoint
                    .scopes
                    .get_mut(key)
                    .unwrap()
                    .pending
                    .as_mut()
                    .unwrap()
                    .next_slot = Some(next);
                self.save()?;
            }
            let commit = Commit {
                schema: WIRE,
                group: self.binding.group,
                root_sha256: view.root_sha.clone(),
                owner: entry.owner.clone(),
                key: key.into(),
                base_sha256: base.clone(),
                slot: slot.clone(),
                operation: pending.operation,
                actor: self.store.actor_id,
                manifest_id,
                manifest,
                next_slot: self.checkpoint.scopes[key]
                    .pending
                    .as_ref()
                    .unwrap()
                    .next_slot
                    .clone()
                    .unwrap(),
            };
            commit.validate(&view.root, &view.root_sha, key, entry, &slot, &base)?;
            let bytes = serde_json::to_vec(&commit)?;
            ensure!(bytes.len() <= 64 * 1024, "commit descriptor bound");
            let Some((id, reference)) = self.upload(bytes.clone(), budget)? else {
                return Ok(Status::Pending);
            };
            self.store
                .stage_blob(&reference, std::io::Cursor::new(bytes.clone()))?;
            self.checkpoint
                .scopes
                .get_mut(key)
                .unwrap()
                .pending
                .as_mut()
                .unwrap()
                .descriptor = Some(Recorded {
                slot: slot.clone(),
                descriptor_id: id,
                sha256: reference.sha256,
                bytes,
            });
            self.save()?;
        }
        let pending = self.checkpoint.scopes[key].pending.clone().unwrap();
        let descriptor = pending.descriptor.as_ref().unwrap();
        descriptor.check(&slot)?;
        let commit: Commit = serde_json::from_slice(&descriptor.bytes)?;
        commit.validate(&view.root, &view.root_sha, key, entry, &slot, &base)?;
        ensure!(
            commit.operation == pending.operation && commit.actor == self.store.actor_id,
            "prepared descriptor does not identify the original local operation"
        );
        let request = SelectedAttemptRequest {
            operation: pending.operation,
            slot_id: slot.clone(),
            descriptor_id: descriptor.descriptor_id.clone(),
            descriptor: ObjectRef {
                sha256: descriptor.sha256.clone(),
                bytes: descriptor.bytes.len() as u64,
            },
        };
        // Durable original preparation precedes any possible commit create.
        admission.with_current(&snapshot.token, |_| {
            let phase = self.checkpoint.scopes[key]
                .pending
                .as_ref()
                .unwrap()
                .outbox_prepared;
            if phase != Some(false) {
                ensure!(
                    self.store
                        .inspect_selected_attempt(pending.operation)?
                        .is_some(),
                    "prepared original outbox missing"
                );
            }
            if phase == Some(false) {
                // Conservative intent is durable BEFORE entering Store's write.
                // An interrupted write can never leave a false claim that
                // preparation was never attempted. None includes legacy unknown.
                self.checkpoint
                    .scopes
                    .get_mut(key)
                    .unwrap()
                    .pending
                    .as_mut()
                    .unwrap()
                    .outbox_prepared = None;
                self.save()?;
            }
            self.store
                .prepare_selected_attempt(&snapshot.token, request.clone())
                .map(|_| ())
        })?;
        self.checkpoint
            .scopes
            .get_mut(key)
            .unwrap()
            .pending
            .as_mut()
            .unwrap()
            .outbox_prepared = Some(true);
        self.save()?;
        let binding = self.slot(key.into(), slot.clone());
        let prepared =
            match self
                .transport
                .prepare_slot(binding, request.descriptor_id, request.descriptor)
            {
                Ok(prepared) => prepared,
                Err(error) if error.is::<MissingPreparedDescriptor>() => {
                    admission.with_current(&snapshot.token, |current| {
                        self.store
                            .current_selected_attempt(&current.token, pending.operation)?;
                        Ok(())
                    })?;
                    let original = self
                        .checkpoint
                        .uploads
                        .get_mut(&descriptor.sha256)
                        .context("original descriptor upload custody absent")?;
                    ensure!(
                        original.id == descriptor.descriptor_id,
                        "original descriptor upload ID differs"
                    );
                    original.complete = false;
                    self.save()?;
                    // Restore only the already allocated object from its exact bytes.
                    // A subsequent step revalidates current admission before create.
                    self.upload(descriptor.bytes.clone(), budget)?;
                    return Ok(Status::Pending);
                }
                Err(error) => return Err(error),
            };
        let result = admission.with_current(&snapshot.token, |current| {
            self.store
                .current_selected_attempt(&current.token, pending.operation)?;
            // The domain gate remains held through this synchronous HTTP call;
            // the Store mutex was released by each actual validation above.
            self.transport.create_or_read_slot(&prepared)
        });
        let recorded = match result {
            Ok(observation) => Recorded::from(AgreedSlotValue {
                pointer: observation.pointer,
                descriptor: observation.descriptor,
            })?,
            Err(error) => {
                let now = self
                    .store
                    .selected_snapshot(&scope, MAX_ITEMS)?
                    .context("current scope disappeared")?;
                if pending.pins.matches(&now.token) {
                    return Err(error);
                }
                // A local edit won the gate race. Only the original binding is read.
                let Some(observed) = self.observe(key.into(), slot.clone())? else {
                    return Ok(Status::Pending);
                };
                observed
            }
        };
        let winner: Commit = serde_json::from_slice(&recorded.bytes)?;
        winner.validate(&view.root, &view.root_sha, key, entry, &slot, &base)?;
        let now = self.store.selected_snapshot(&scope, MAX_ITEMS)?;
        self.checkpoint.scopes.get_mut(key).unwrap().receive = Some(Receive {
            operation: Uuid::new_v4(),
            expected: now.as_ref().map(|s| Pins::from(&s.token)),
            descriptor: recorded,
            manifest: None,
            manifest_reference: None,
            original_attempt: Some(pending.operation),
            applied: false,
            verified: false,
        });
        self.save()?;
        Ok(Status::Pending)
    }
}
