use super::*;

#[derive(Debug)]
struct MissingSharedObject;

impl std::fmt::Display for MissingSharedObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("promised shared object not visible")
    }
}

impl std::error::Error for MissingSharedObject {}

impl Engine {
    fn incoming(&self, key: &str) -> PathBuf {
        self.store
            .paths
            .data
            .join("sync/incoming")
            .join(self.checkpoint.epoch.to_string())
            .join(key)
    }
    fn content_id(&mut self, reference: &ObjectRef) -> Result<Option<String>> {
        let mut page = None;
        let mut seen = BTreeSet::new();
        let mut count = 0usize;
        let mut metadata_bytes = 0usize;
        let mut found = None;
        loop {
            let result = self.transport.list(page.as_deref())?;
            for file in result.items {
                count += 1;
                metadata_bytes = metadata_bytes
                    .checked_add(serde_json::to_vec(&file)?.len())
                    .context("content metadata overflow")?;
                ensure!(metadata_bytes <= MAX_METADATA, "content metadata budget");
                ensure!(count <= MAX_ITEMS, "shared content inventory bound");
                if file.trashed {
                    continue;
                }
                if let Ok(tag) = file.tag() {
                    if tag.collection == self.binding.group
                        && tag.kind == "object"
                        && tag.sha256 == reference.sha256
                    {
                        // Metadata is a locator only; the exact downloaded bytes
                        // and complete domain closure are validated afterward.
                        found.get_or_insert(file.id);
                    }
                }
            }
            let Some(next) = result.next else {
                break;
            };
            ensure!(seen.insert(next.clone()), "shared content pagination cycle");
            page = Some(next);
        }
        Ok(found)
    }
    fn fetch(
        &mut self,
        key: &str,
        reference: &ObjectRef,
        id: Option<&str>,
        budget: &mut usize,
    ) -> Result<Option<Vec<u8>>> {
        ensure!(
            valid_digest(&reference.sha256) && reference.bytes > 0 && reference.bytes <= MAX_MEDIA,
            "shared object reference bounds"
        );
        let directory = self.incoming(key);
        files::directory(&directory)?;
        let path = directory.join(&reference.sha256);
        files::safe_path(&path)?;
        if path.exists() {
            let bytes = files::read(&path, reference.bytes)?;
            ensure!(
                bytes.len() as u64 == reference.bytes && digest(&bytes) == reference.sha256,
                "incoming object changed"
            );
            return Ok(Some(bytes));
        }
        if *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        let id = match id {
            Some(id) => id.into(),
            None => self.content_id(reference)?.ok_or(MissingSharedObject)?,
        };
        let bytes = self.transport.download(&id, reference.bytes)?;
        ensure!(
            bytes.len() as u64 == reference.bytes && digest(&bytes) == reference.sha256,
            "shared object integrity differs"
        );
        files::object(&path, &reference.sha256, &bytes)?;
        Ok(Some(bytes))
    }
    pub(super) fn scope_view(
        &self,
        key: &str,
        entry: &Entry,
        view: &CatalogView,
    ) -> Result<(String, String)> {
        let mut slot = entry.genesis_slot.clone();
        let mut base = genesis_base(&view.root_sha, key);
        let mut seen = BTreeSet::new();
        if let Some(progress) = self.checkpoint.scopes.get(key) {
            for recorded in &progress.chain {
                ensure!(seen.insert(slot.clone()), "shared scope slot cycle");
                recorded.check(&slot)?;
                let commit: Commit = serde_json::from_slice(&recorded.bytes)?;
                commit.validate(&view.root, &view.root_sha, key, entry, &slot, &base)?;
                slot = commit.next_slot;
                base = recorded.sha256.clone();
            }
        }
        Ok((slot, base))
    }
    pub(super) fn scope_step(
        &mut self,
        key: &str,
        entry: &Entry,
        view: &CatalogView,
        budget: &mut usize,
    ) -> Result<Status> {
        if self.checkpoint.scopes[key].receive.is_some() {
            return self.finish_receive(key, entry, view, budget);
        }
        let (slot, base) = self.scope_view(key, entry, view)?;
        if let Some(previous) = self.checkpoint.scopes[key].chain.last().cloned() {
            let current = self
                .observe(key.into(), previous.slot.clone())?
                .context("applied upstream pointer not visible")?;
            ensure!(
                current.sha256 == previous.sha256 && current.bytes == previous.bytes,
                "applied upstream pointer changed"
            );
        }
        if *budget == 0 {
            return Ok(Status::Pending);
        }
        let scope = SelectionScope {
            group: self.binding.group,
            key_sha256: key.into(),
            binding_sha256: self.binding.digest.clone(),
        };
        // A current original with staged descriptor bytes can retry its exact
        // create. An observation of absence is neither required nor a grant.
        let current = self.store.selected_snapshot(&scope, MAX_ITEMS)?;
        if let Some(pending) = &self.checkpoint.scopes[key].pending {
            let current_original = pending.descriptor.is_some()
                && current
                    .as_ref()
                    .is_some_and(|snapshot| pending.pins.matches(&snapshot.token));
            if current_original
                && (pending.outbox_prepared == Some(false)
                    || self
                        .store
                        .inspect_selected_attempt(pending.operation)?
                        .is_some())
            {
                return self.publish_scope(entry, view, slot, base, budget, current);
            }
        }
        if let Some(winner) = self.observe(key.into(), slot.clone())? {
            *budget -= 1;
            let commit: Commit = serde_json::from_slice(&winner.bytes)?;
            commit.validate(&view.root, &view.root_sha, key, entry, &slot, &base)?;
            let current = self.store.selected_snapshot(&scope, MAX_ITEMS)?;
            let receive = Receive {
                operation: Uuid::new_v4(),
                expected: current.as_ref().map(|s| Pins::from(&s.token)),
                descriptor: winner,
                manifest: None,
                manifest_reference: None,
                original_attempt: self.checkpoint.scopes[key]
                    .pending
                    .as_ref()
                    .map(|p| p.operation),
                applied: false,
                verified: false,
            };
            self.checkpoint.scopes.get_mut(key).unwrap().receive = Some(receive);
            self.save()?;
            return self.finish_receive(key, entry, view, budget);
        }
        self.publish_scope(entry, view, slot, base, budget, current)
    }
    fn finish_receive(
        &mut self,
        key: &str,
        entry: &Entry,
        view: &CatalogView,
        budget: &mut usize,
    ) -> Result<Status> {
        let mut receive = self.checkpoint.scopes[key]
            .receive
            .clone()
            .context("original receive absent")?;
        if receive.applied {
            return self.retire_applied_receive(key, receive);
        }
        let (slot, base) = self.scope_view(key, entry, view)?;
        receive.descriptor.check(&slot)?;
        let commit: Commit = serde_json::from_slice(&receive.descriptor.bytes)?;
        commit.validate(&view.root, &view.root_sha, key, entry, &slot, &base)?;
        let scope = SelectionScope {
            group: self.binding.group,
            key_sha256: key.into(),
            binding_sha256: self.binding.digest.clone(),
        };
        let receipt = self.store.selected_receipt(&scope, receive.operation)?;
        let expected = if let Some(receipt) = &receipt {
            let previous = if receive.expected.is_some() {
                self.store
                    .historical_predecessor_token(&scope, receive.operation)?
            } else {
                ensure!(
                    receipt.mutation == crate::storage::selection::SelectionMutation::Initialize,
                    "original receive initialization differs"
                );
                None
            };
            ensure!(
                match (&receive.expected, &previous) {
                    (None, None) => true,
                    (Some(p), Some(t)) => p.matches(t),
                    _ => false,
                },
                "receiving original predecessor differs"
            );
            previous
        } else {
            let current = self.store.selected_snapshot(&scope, MAX_ITEMS)?;
            if !match (&receive.expected, &current) {
                (None, None) => true,
                (Some(p), Some(s)) => p.matches(&s.token),
                _ => false,
            } {
                return Ok(Status::RecoveryRequired);
            }
            current.map(|s| s.token)
        };
        if receive.manifest_reference.is_none() {
            let Some(raw) = self.fetch(key, &commit.manifest, Some(&commit.manifest_id), budget)?
            else {
                return Ok(Status::Pending);
            };
            let manifest: Manifest = serde_json::from_slice(&raw)?;
            manifest.validate()?;
            ensure!(
                manifest.scope == Scope::SelectedRecords,
                "shared manifest is not a selected closure"
            );
            ensure!(
                receive
                    .manifest
                    .as_ref()
                    .is_none_or(|saved| *saved == manifest),
                "original received manifest differs from saved manifest"
            );
            let path =
                manifest_custody_path(&self.store, self.checkpoint.epoch, key, &commit.manifest);
            files::directory(path.parent().context("manifest custody directory absent")?)?;
            files::object(&path, &commit.manifest.sha256, &raw)?;
            receive.manifest_reference = Some(commit.manifest.clone());
            receive.manifest = Some(manifest);
            self.checkpoint.scopes.get_mut(key).unwrap().receive = Some(receive.clone());
            self.save()?;
        }
        let manifest = receive
            .manifest
            .as_ref()
            .context("receiving manifest absent")?;
        verify_manifest_custody(
            &self.store,
            self.checkpoint.epoch,
            key,
            &commit.manifest,
            &receive,
            manifest,
        )?;
        let media_total = manifest.media.iter().try_fold(0u64, |n, r| {
            n.checked_add(r.bytes).context("receiving media overflow")
        })?;
        ensure!(media_total <= MAX_MEDIA, "receiving media bound");
        let mut objects = BTreeMap::new();
        let mut notebook_media = BTreeSet::new();
        for reference in &manifest.records {
            let Some(bytes) = self.fetch(key, reference, None, budget)? else {
                return Ok(Status::Pending);
            };
            if manifest.record_namespaces.get(&reference.sha256) == Some(&Namespace::Source) {
                let envelope: Envelope = serde_json::from_slice(&bytes)?;
                if envelope.domain_schema_version
                    == crate::conversation::notebook::NOTEBOOK_SOURCE_SCHEMA
                {
                    let crate::conversation::Record::ArchivedNormalizedSource(source) =
                        crate::conversation::Ledger::decode(&envelope)?
                    else {
                        anyhow::bail!("invalid shared notebook archive");
                    };
                    let required = ObjectRef {
                        sha256: source.media.sha256.clone(),
                        bytes: source.media.bytes,
                    };
                    if !manifest.media.contains(&required) {
                        return Ok(Status::IncompleteMedia);
                    }
                    notebook_media.insert(required.sha256);
                }
            }
            objects.insert(reference.sha256.clone(), bytes);
        }
        for reference in &manifest.media {
            match self.fetch(key, reference, None, budget) {
                Ok(Some(bytes)) => {
                    objects.insert(reference.sha256.clone(), bytes);
                }
                Ok(None) => return Ok(Status::Pending),
                Err(error)
                    if notebook_media.contains(&reference.sha256)
                        && error.is::<MissingSharedObject>() =>
                {
                    return Ok(Status::IncompleteMedia);
                }
                Err(error) => return Err(error),
            }
        }
        let adoption = match &entry.owner {
            Owner::Memory(owner) => {
                let winner = shared::SharedWinner::validate(
                    &self.store,
                    scope,
                    owner.clone(),
                    view.root.profile,
                    receive.descriptor.sha256.clone(),
                    manifest.clone(),
                    objects,
                )?;
                self.mark_receive_verified(key)?;
                winner.apply(&self.store, receive.operation, expected.as_ref())?
            }
            Owner::Preferences(_) | Owner::Handwriting(_) => {
                let correction = match entry.owner {
                    Owner::Handwriting(owner) => Some(owner),
                    _ => None,
                };
                let winner = preferences::Winner::validate(
                    &self.store,
                    scope,
                    receive.descriptor.sha256.clone(),
                    manifest.clone(),
                    objects,
                    correction,
                )?;
                self.mark_receive_verified(key)?;
                let publication =
                    winner.apply(&self.store, receive.operation, expected.as_ref())?;
                shared::SharedAdoption::Applied(Box::new(publication.transaction))
            }
        };
        match adoption {
            shared::SharedAdoption::CommittedNotApplied => Ok(Status::CommittedNotApplied),
            shared::SharedAdoption::Applied(transaction) => {
                ensure!(
                    transaction.operation == receive.operation
                        && transaction.accepted_base_sha256 == receive.descriptor.sha256,
                    "receiving receipt differs"
                );
                let progress = self.checkpoint.scopes.get_mut(key).unwrap();
                progress.chain.push(receive.descriptor.clone());
                progress.applied = Some(Applied {
                    descriptor: receive.descriptor.sha256.clone(),
                    operation: receive.operation,
                    membership: membership(manifest)?,
                });
                let mut completed = receive.clone();
                completed.applied = true;
                completed.verified = true;
                progress.receipts.push(completed.clone());
                progress.receive = Some(completed.clone());
                // Receipt/checkpoint first, then local-only outbound withdrawal.
                self.save()?;
                let status = self.retire_applied_receive(key, completed)?;
                for reference in manifest
                    .records
                    .iter()
                    .chain(&manifest.media)
                    .chain(std::iter::once(&commit.manifest))
                {
                    let path = self.incoming(key).join(&reference.sha256);
                    files::safe_path(&path)?;
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(status)
            }
        }
    }
    fn mark_receive_verified(&mut self, key: &str) -> Result<()> {
        let receive = self
            .checkpoint
            .scopes
            .get_mut(key)
            .unwrap()
            .receive
            .as_mut()
            .context("verified original receive absent")?;
        if !receive.verified {
            receive.verified = true;
            self.save()?;
        }
        Ok(())
    }

    fn retire_applied_receive(&mut self, key: &str, receive: Receive) -> Result<Status> {
        let scope = SelectionScope {
            group: self.binding.group,
            key_sha256: key.into(),
            binding_sha256: self.binding.digest.clone(),
        };
        let manifest = receive
            .manifest
            .as_ref()
            .context("applied receiving manifest absent")?;
        let receipt = self
            .store
            .selected_receipt(&scope, receive.operation)?
            .context("applied receiving receipt absent")?;
        ensure!(
            receipt.accepted_base_sha256 == receive.descriptor.sha256
                && shared::same_membership(&receipt.selected, manifest)
                && self.checkpoint.scopes[key]
                    .chain
                    .last()
                    .is_some_and(|r| r.sha256 == receive.descriptor.sha256
                        && r.bytes == receive.descriptor.bytes),
            "applied receiving checkpoint differs from original receipt"
        );
        if let Some(original) = receive.original_attempt {
            let pending = self.checkpoint.scopes[key]
                .pending
                .as_ref()
                .context("receiving original staging absent")?
                .clone();
            ensure!(
                pending.operation == original && pending.slot == receive.descriptor.slot,
                "receiving original staging differs"
            );
            let token = self
                .store
                .historical_selection_token(&scope, pending.selection_operation)?
                .context("receiving original selection receipt absent")?;
            ensure!(
                pending.pins.matches(&token),
                "receiving original staged pins differ"
            );
            if let Some(attempt) = self.store.inspect_selected_attempt(original)? {
                let selected = self
                    .store
                    .selected_receipt(&scope, pending.selection_operation)?
                    .context("receiving original selected receipt absent")?;
                let descriptor = pending
                    .descriptor
                    .as_ref()
                    .context("receiving original descriptor absent")?;
                ensure!(
                    serde_json::to_vec(&attempt.selection)? == serde_json::to_vec(&selected)?
                        && attempt.mode_epoch == Some(self.checkpoint.epoch)
                        && attempt.request.slot_id == pending.slot
                        && attempt.request.descriptor_id == descriptor.descriptor_id
                        && attempt.request.descriptor.sha256 == descriptor.sha256
                        && attempt.request.descriptor.bytes == descriptor.bytes.len() as u64,
                    "receiving original outbox differs"
                );
                self.store.withdraw_selected_attempt(original)?;
            } else {
                ensure!(
                    pending.outbox_prepared == Some(false),
                    "receiving original outbox missing or preparation unknown"
                );
            }
            ensure!(
                self.checkpoint.scopes[key].retired_pending.len() < MAX_ITEMS,
                "retired original staging bound"
            );
            self.checkpoint
                .scopes
                .get_mut(key)
                .unwrap()
                .retired_pending
                .push(pending);
        }
        let progress = self.checkpoint.scopes.get_mut(key).unwrap();
        progress.pending = None;
        progress.receive = None;
        self.save()?;
        let current = self.store.selected_snapshot(&scope, MAX_ITEMS)?;
        let Some(current) = current else {
            return Ok(Status::RecoveryRequired);
        };
        if current.token.accepted_base_sha256() != receive.descriptor.sha256 {
            return Ok(Status::RecoveryRequired);
        }
        if !shared::same_membership(&current.transaction.selected, manifest) {
            return Ok(Status::Pending);
        }
        Ok(
            if manifest
                .media_coverage
                .iter()
                .any(|c| matches!(c, Coverage::Omitted { .. }))
            {
                Status::IncompleteMedia
            } else {
                Status::Current
            },
        )
    }
}
pub(super) fn membership(manifest: &Manifest) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        &manifest.records,
        &manifest.record_namespaces,
        &manifest.media,
        &manifest.media_coverage,
    ))?))
}

fn manifest_custody_path(store: &Store, epoch: Uuid, key: &str, reference: &ObjectRef) -> PathBuf {
    store
        .paths
        .data
        .join("sync/incoming")
        .join(epoch.to_string())
        .join("manifest-custody")
        .join(key)
        .join(&reference.sha256)
}

fn verify_manifest_custody(
    store: &Store,
    epoch: Uuid,
    key: &str,
    expected: &ObjectRef,
    receive: &Receive,
    manifest: &Manifest,
) -> Result<()> {
    ensure!(
        valid_digest(key)
            && valid_digest(&expected.sha256)
            && (1..=MAX_METADATA as u64).contains(&expected.bytes),
        "original manifest custody bounds differ"
    );
    let raw = if let Some(reference) = &receive.manifest_reference {
        ensure!(reference == expected, "original manifest reference differs");
        let path = manifest_custody_path(store, epoch, key, reference);
        files::read(&path, reference.bytes).context("original manifest custody unavailable")?
    } else {
        let incoming = store
            .paths
            .data
            .join("sync/incoming")
            .join(epoch.to_string())
            .join(key)
            .join(&expected.sha256);
        files::safe_path(&incoming)?;
        if incoming.try_exists()? {
            files::read(&incoming, expected.bytes)?
        } else {
            // Older checkpoints may have cleaned their incoming bytes.
            // Only an exact digest match can prove the old encoding.
            serde_json::to_vec(manifest)?
        }
    };
    ensure!(
        raw.len() as u64 == expected.bytes && digest(&raw) == expected.sha256,
        "foreign intent manifest does not match the original descriptor"
    );
    let original: Manifest = serde_json::from_slice(&raw)?;
    ensure!(
        original == *manifest,
        "original manifest differs from saved parsed correspondence"
    );
    Ok(())
}

/// Exact historical Worker custody only. A generic imported manifest or a
/// foreign actor ID is insufficient, and this check grants no local authority.
pub(crate) fn verify_received_intents(
    store: &Store,
    scope: &SelectionScope,
    pairs: &[(&ObjectRef, &ObjectRef)],
) -> Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let path = store.paths.data.join("sync/state.json");
    let state: State = files::json(&path, MAX_METADATA as u64)
        .context("foreign pending intent has no verified Worker custody")?;
    ensure!(
        state.format == FORMAT,
        "foreign intent checkpoint format differs"
    );
    let selected = state.selected.context("foreign intent checkpoint absent")?;
    ensure!(
        selected.format == 1,
        "foreign intent selected checkpoint format differs"
    );
    let mut unproven = pairs.to_vec();
    for (epoch, checkpoint) in selected.epochs {
        if checkpoint.actor != store.actor_id
            || checkpoint.group != scope.group
            || checkpoint.binding != scope.binding_sha256
        {
            continue;
        }
        ensure!(
            checkpoint.format == 1 && checkpoint.epoch == epoch,
            "foreign intent checkpoint identity differs"
        );
        let Some(progress) = checkpoint.scopes.get(&scope.key_sha256) else {
            continue;
        };
        let binding = Binding {
            group: scope.group,
            root_id: checkpoint.root_id.clone(),
            digest: scope.binding_sha256.clone(),
        };
        let view =
            catalog_view(&binding, &checkpoint)?.context("foreign intent root custody absent")?;
        let entry = view
            .entries
            .get(&scope.key_sha256)
            .context("foreign intent scope not registered")?;
        let mut slot = entry.genesis_slot.clone();
        let mut base = genesis_base(&view.root_sha, &scope.key_sha256);
        let mut seen = BTreeSet::new();
        for recorded in &progress.chain {
            ensure!(
                seen.insert(slot.clone()),
                "foreign intent receiving ancestry cycle"
            );
            recorded.check(&slot)?;
            let commit: Commit = serde_json::from_slice(&recorded.bytes)?;
            commit.validate(
                &view.root,
                &view.root_sha,
                &scope.key_sha256,
                entry,
                &slot,
                &base,
            )?;
            slot = commit.next_slot;
            base.clone_from(&recorded.sha256);
            for receive in progress.receipts.iter().filter(|receive| {
                receive.applied
                    && receive.descriptor.sha256 == recorded.sha256
                    && receive.descriptor.bytes == recorded.bytes
                    && receive.descriptor.slot == recorded.slot
                    && receive.descriptor.descriptor_id == recorded.descriptor_id
            }) {
                let manifest = receive
                    .manifest
                    .as_ref()
                    .context("foreign intent applied manifest absent")?;
                if !unproven.iter().any(|(receipt, fact)| {
                    manifest.records.contains(receipt) && manifest.records.contains(fact)
                }) {
                    continue;
                }
                verify_manifest_custody(
                    store,
                    epoch,
                    &scope.key_sha256,
                    &commit.manifest,
                    receive,
                    manifest,
                )?;
                let local = store
                    .selected_receipt(scope, receive.operation)?
                    .context("foreign intent applied receipt absent")?;
                ensure!(
                    local.accepted_base_sha256 == recorded.sha256
                        && shared::same_membership(&local.selected, manifest),
                    "foreign intent applied closure differs"
                );
                match &receive.expected {
                    Some(pins) => {
                        let predecessor = store
                            .historical_predecessor_token(scope, receive.operation)?
                            .context("foreign intent original predecessor absent")?;
                        ensure!(
                            local.mutation
                                == crate::storage::selection::SelectionMutation::Activate
                                && pins.matches(&predecessor),
                            "foreign intent activation predecessor differs"
                        );
                    }
                    None => ensure!(
                        local.mutation == crate::storage::selection::SelectionMutation::Initialize,
                        "foreign intent initialization receipt differs"
                    ),
                }
                unproven.retain(|(receipt, fact)| {
                    !manifest.records.contains(receipt) || !manifest.records.contains(fact)
                });
                if unproven.is_empty() {
                    return Ok(());
                }
            }
        }
    }
    anyhow::bail!("foreign pending intent lacks exact applied Worker custody")
}
