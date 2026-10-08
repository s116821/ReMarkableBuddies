use super::*;

/// Caller supplies a domain-validated winner and unresolved evidence closure.
/// This storage primitive does not establish document ownership or native authority.
#[derive(Clone, Serialize)]
pub struct SelectionChange {
    pub operation: Uuid,
    pub accepted_base_sha256: String,
    pub selected: Manifest,
    pub retained: Vec<Manifest>,
}

/// Replay returns the ORIGINAL transaction and token, which may now be stale.
/// Read a fresh snapshot for further mutations; replay never mints current authority.
pub struct SelectionPublication {
    pub transaction: SelectionTransaction,
    pub token: SelectionToken,
    pub replayed: bool,
}
fn token(transaction: &SelectionTransaction) -> Result<SelectionToken> {
    Ok(SelectionToken {
        scope: transaction.scope.clone(),
        store_generation: transaction.store_generation,
        aggregate_generation: transaction.aggregate_generation,
        accepted_base_sha256: transaction.accepted_base_sha256.clone(),
        selection_sha256: digest(&serde_json::to_vec(transaction)?),
    })
}
fn receipt(transaction: SelectionTransaction, replayed: bool) -> Result<SelectionPublication> {
    Ok(SelectionPublication {
        token: token(&transaction)?,
        transaction,
        replayed,
    })
}
fn path(root: &Path, scope: &SelectionScope) -> Result<PathBuf> {
    Ok(root.join("selections").join(scope.filename()?))
}
fn current(
    root: &Path,
    generation: Uuid,
    scope: &SelectionScope,
) -> Result<Option<SelectionTransaction>> {
    let path = path(root, scope)?;
    files::safe_path(&path)?;
    if !path.exists() {
        return Ok(None);
    }
    let transaction: SelectionTransaction = files::json(&path, MAX_METADATA as u64)?;
    transaction.validate(MAX_ITEMS)?;
    ensure!(
        transaction.store_generation == generation && &transaction.scope == scope,
        "selection scope/store generation mismatch"
    );
    Ok(Some(transaction))
}

/// Follow only metadata reachable from the published current transaction. Orphan
/// staging never counts as a receipt. Bound the entire scan, not only each node.
pub(super) fn history(
    root: &Path,
    generation: Uuid,
    scope: &SelectionScope,
    transaction: SelectionTransaction,
) -> Result<Vec<SelectionTransaction>> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut operations = BTreeSet::new();
    let mut bytes = 0usize;
    let mut next = transaction;
    loop {
        next.validate(MAX_ITEMS)?;
        ensure!(
            next.store_generation == generation && &next.scope == scope,
            "selection history scope/generation mismatch"
        );
        let encoded = serde_json::to_vec(&next)?;
        bytes = bytes
            .checked_add(encoded.len())
            .context("selection history overflow")?;
        ensure!(
            bytes <= MAX_METADATA && chain.len() < MAX_ITEMS,
            "selection history exceeds bound; explicit compaction required"
        );
        ensure!(
            seen.insert(digest(&encoded)) && operations.insert(next.operation),
            "selection history identity collision"
        );
        let previous = next.previous_sha256.clone();
        let depth = next.history_depth;
        chain.push(next);
        let Some(hash) = previous else {
            break;
        };
        let encoded = files::read(
            &root.join("selection-history").join(&hash),
            MAX_METADATA as u64,
        )?;
        ensure!(
            digest(&encoded) == hash,
            "selection history integrity failure"
        );
        next = serde_json::from_slice(&encoded).context("unsupported selection history")?;
        ensure!(
            next.history_depth + 1 == depth,
            "selection history depth mismatch"
        );
    }
    Ok(chain)
}
pub(super) fn closure_index(records: &[(Envelope, String)], unique_heads: bool) -> Result<Index> {
    let mut index = Index::default();
    Store::can_apply(&index, records)?;
    // Build the same causal index without giving the closure a local commit ID.
    for (record, hash) in records {
        index
            .records
            .insert(record.revision_id, (record.clone(), hash.clone()));
        index.operations.insert(record.operation_id, hash.clone());
        index
            .heads
            .entry((record.namespace, record.record_id))
            .or_default()
            .insert(record.revision_id);
    }
    for (record, _) in records {
        for parent in &record.parents {
            index
                .heads
                .get_mut(&(record.namespace, record.record_id))
                .unwrap()
                .remove(parent);
        }
    }
    ensure!(
        !unique_heads || index.heads.values().all(|heads| heads.len() == 1),
        "conflicted selected record membership"
    );
    Ok(index)
}
pub(super) fn validate_closure(
    root: &Path,
    transaction: &SelectionTransaction,
) -> Result<Vec<ValidatedManifest>> {
    transaction.validate(MAX_ITEMS)?;
    let active = Store::validate_objects(root, &transaction.selected)?;
    closure_index(&active, true)?;
    let mut combined = active.clone();
    let mut parts = vec![(transaction.selected.clone(), active)];
    for manifest in &transaction.retained {
        let records = Store::validate_objects(root, manifest)?;
        combined.extend(records.clone());
        parts.push((manifest.clone(), records));
    }
    let mut unique = BTreeMap::new();
    for (record, hash) in combined {
        if let Some((_, prior)) = unique.get(&record.revision_id) {
            ensure!(prior == &hash, "retained revision identity collision");
        }
        unique.insert(record.revision_id, (record, hash));
    }
    closure_index(&unique.into_values().collect::<Vec<_>>(), false)?;
    Ok(parts)
}
fn request(
    kind: &str,
    scope: &SelectionScope,
    expected: Option<&SelectionToken>,
    value: impl Serialize,
) -> Result<String> {
    // Live tokens are not serializable. Only their exact identity digest enters
    // this storage request fingerprint; it cannot manufacture a live token.
    Ok(digest(&serde_json::to_vec(&(
        kind,
        scope,
        expected.map(SelectionToken::selection_sha256),
        value,
    ))?))
}
fn replay(
    chain: &[SelectionTransaction],
    operation: Uuid,
    request: &str,
) -> Result<Option<SelectionPublication>> {
    if let Some(original) = chain.iter().find(|t| t.operation == operation) {
        ensure!(
            original.request_sha256 == request,
            "selection operation identity collision"
        );
        return Ok(Some(receipt(original.clone(), true)?));
    }
    Ok(None)
}

impl Store {
    /// Read accepted publication evidence after process restart. This does not
    /// mint a live token or publish a mutation. Persisted intent pins are compared
    /// by the domain owner; an unaccepted operation returns None, not permission.
    pub fn selected_receipt(
        &self,
        scope: &SelectionScope,
        operation: Uuid,
    ) -> Result<Option<SelectionTransaction>> {
        scope.validate()?;
        ensure!(!operation.is_nil(), "nil selection operation");
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let root = self.generation(inner.generation);
        let Some(current) = current(&root, inner.generation, scope)? else {
            return Ok(None);
        };
        let chain = history(&root, inner.generation, scope, current)?;
        let original = chain.into_iter().find(|t| t.operation == operation);
        if let Some(transaction) = &original {
            validate_closure(&root, transaction)?;
        }
        Ok(original)
    }
    /// Explicit bootstrap only. Snapshot absence does not authorize this call;
    /// the domain/coordinator must separately qualify the initial winner.
    pub fn initialize_selected(
        &self,
        scope: &SelectionScope,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        let fingerprint = request("initialize", scope, None, &change)?;
        self.publish_selection(scope, None, change, objects, fingerprint)
    }
    pub fn activate_selected(
        &self,
        expected: &SelectionToken,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        let fingerprint = request("activate", &expected.scope, Some(expected), &change)?;
        self.publish_selection(
            &expected.scope,
            Some(expected),
            change,
            objects,
            fingerprint,
        )
    }
    fn publish_selection(
        &self,
        scope: &SelectionScope,
        expected: Option<&SelectionToken>,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
        fingerprint: String,
    ) -> Result<SelectionPublication> {
        scope.validate()?;
        ensure!(!change.operation.is_nil(), "nil selection operation");
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let root = self.generation(inner.generation);
        let previous = current(&root, inner.generation, scope)?;
        let chain = previous
            .clone()
            .map(|t| history(&root, inner.generation, scope, t))
            .transpose()?
            .unwrap_or_default();
        if let Some(receipt) = replay(&chain, change.operation, &fingerprint)? {
            return Ok(receipt);
        }
        if previous.is_none() {
            let directory = root.join("selections");
            files::safe_path(&directory)?;
            ensure!(
                !directory.exists() || fs::read_dir(directory)?.count() < MAX_ITEMS,
                "selection count exceeds bound"
            );
        }
        match (&previous, expected) {
            (None, None) => (),
            (Some(previous), Some(expected)) => {
                ensure!(token(previous)? == *expected, "stale selection token")
            }
            _ => anyhow::bail!("explicit selection initialization or captured token required"),
        }
        let transaction = self.next_selection(
            inner.generation,
            scope,
            previous.as_ref(),
            change,
            fingerprint,
        )?;
        self.write_selection(
            &mut inner,
            &root,
            previous.as_ref(),
            transaction,
            objects,
            &chain,
        )
    }
    fn next_selection(
        &self,
        generation: Uuid,
        scope: &SelectionScope,
        previous: Option<&SelectionTransaction>,
        change: SelectionChange,
        fingerprint: String,
    ) -> Result<SelectionTransaction> {
        let mut selected = change.selected;
        // A closure is not the source's all-history commit. Give each accepted
        // publication its own manifest identity so it never overwrites that commit.
        selected.transaction_id = change.operation;
        let retained = change
            .retained
            .into_iter()
            .map(|mut manifest| {
                if !previous.is_some_and(|t| t.retained.contains(&manifest)) {
                    manifest.transaction_id = Uuid::new_v4();
                }
                manifest
            })
            .collect();
        let transaction = SelectionTransaction {
            format: FORMAT,
            operation: change.operation,
            store_generation: generation,
            aggregate_generation: Uuid::new_v4(),
            scope: scope.clone(),
            accepted_base_sha256: change.accepted_base_sha256,
            request_sha256: fingerprint,
            previous_sha256: previous.map(token).transpose()?.map(|t| t.selection_sha256),
            history_depth: previous.map_or(1, |t| t.history_depth + 1),
            selected,
            retained,
        };
        transaction.validate(MAX_ITEMS)?;
        Ok(transaction)
    }
    fn write_selection(
        &self,
        inner: &mut Inner,
        root: &Path,
        previous: Option<&SelectionTransaction>,
        transaction: SelectionTransaction,
        objects: BTreeMap<String, Vec<u8>>,
        chain: &[SelectionTransaction],
    ) -> Result<SelectionPublication> {
        ensure!(
            inner.legacy_sync_handles == 0,
            "stop all legacy sync handles before selected publication"
        );
        let total = chain
            .iter()
            .try_fold(serde_json::to_vec(&transaction)?.len(), |n, t| {
                n.checked_add(serde_json::to_vec(t)?.len())
                    .context("selection history overflow")
            })?;
        ensure!(
            total <= MAX_METADATA,
            "selection history exceeds bound; explicit compaction required"
        );
        let required: BTreeMap<_, _> = std::iter::once(&transaction.selected)
            .chain(&transaction.retained)
            .flat_map(|m| m.records.iter().chain(&m.media))
            .map(|r| (r.sha256.clone(), r.bytes))
            .collect();
        ensure!(
            objects.keys().all(|h| required.contains_key(h)),
            "unreferenced selection object"
        );
        Self::trip(inner, Fault::BeforeObjects)?;
        for (hash, bytes) in objects {
            ensure!(
                required.get(&hash) == Some(&(bytes.len() as u64)),
                "selection object size mismatch"
            );
            files::object(&root.join("objects").join(&hash), &hash, &bytes)?;
        }
        Self::trip(inner, Fault::AfterObjects)?;
        let parts = validate_closure(root, &transaction)?;
        let records: Vec<_> = parts
            .iter()
            .flat_map(|(_, rs)| rs.iter().cloned())
            .collect();
        // Deduplicate identical retained references before global identity checks.
        let unique: BTreeMap<_, _> = records
            .iter()
            .cloned()
            .map(|r| (r.0.revision_id, r))
            .collect();
        Self::can_apply(&inner.index, &unique.into_values().collect::<Vec<_>>())?;
        for (manifest, _) in &parts {
            if let Some(prior) = inner.index.manifests.get(&manifest.transaction_id) {
                ensure!(prior == manifest, "selection manifest identity collision");
            }
        }
        files::directory(&root.join("selections"))?;
        files::directory(&root.join("selection-history"))?;
        if let Some(previous) = previous {
            let bytes = serde_json::to_vec(previous)?;
            files::object(
                &root.join("selection-history").join(digest(&bytes)),
                &digest(&bytes),
                &bytes,
            )?;
        }
        Self::trip(inner, Fault::BeforeCommit)?;
        Self::trip(inner, Fault::BeforeActivation)?;
        // One fsync/rename publication commits BOTH winner and retained closure.
        // The lock still protects the token and causal checks performed above.
        let result = files::atomic_json(&path(root, &transaction.scope)?, &transaction);
        if let Err(error) = result {
            // rename may have succeeded before a directory-sync failure. Rebuild
            // evidence from actual durable metadata; never continue with a stale index.
            inner.index = self.recover(inner.generation)?;
            return Err(error);
        }
        for (manifest, records) in parts {
            Self::apply_index(&mut inner.index, manifest, records);
        }
        Self::trip(inner, Fault::AfterCommit)?;
        Self::trip(inner, Fault::AfterActivation)?;
        receipt(transaction, false)
    }

    /// A selected append is one full-closure selection publication. It does not
    /// first publish an independent ordinary commit then switch an overlay.
    pub fn commit_selected(
        &self,
        expected: &SelectionToken,
        operation: Uuid,
        records: Vec<Envelope>,
        media: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        ensure!(
            !operation.is_nil() && !records.is_empty(),
            "invalid selected commit"
        );
        let fingerprint = request(
            "commit",
            &expected.scope,
            Some(expected),
            (&operation, &records),
        )?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let root = self.generation(inner.generation);
        let previous = current(&root, inner.generation, &expected.scope)?
            .context("selected scope is absent")?;
        let chain = history(&root, inner.generation, &expected.scope, previous.clone())?;
        if let Some(receipt) = replay(&chain, operation, &fingerprint)? {
            return Ok(receipt);
        }
        ensure!(token(&previous)? == *expected, "stale selection token");
        let selected_records = Self::validate_objects(&root, &previous.selected)?;
        let selected_index = closure_index(&selected_records, true)?;
        let mut objects = media;
        let mut selected = previous.selected.clone();
        let mut new_keys = BTreeSet::new();
        for record in records {
            record.validate()?;
            ensure!(
                new_keys.insert((record.namespace, record.record_id)),
                "multiple selected edits of one record"
            );
            let heads = selected_index
                .heads
                .get(&(record.namespace, record.record_id))
                .cloned()
                .unwrap_or_default();
            if heads != record.parents {
                return Err(Conflict { heads }.into());
            }
            ensure!(
                !selected_index.records.contains_key(&record.revision_id)
                    && !inner.index.operations.contains_key(&record.operation_id),
                "selected record identity already committed"
            );
            let encoded = serde_json::to_vec(&record)?;
            let hash = digest(&encoded);
            selected
                .record_namespaces
                .insert(hash.clone(), record.namespace);
            selected.records.push(ObjectRef {
                sha256: hash.clone(),
                bytes: encoded.len() as u64,
            });
            objects.insert(hash, encoded);
            for descriptor in record.media_descriptors {
                let reference = ObjectRef {
                    sha256: descriptor.sha256.clone(),
                    bytes: descriptor.bytes,
                };
                if let Some(prior) = selected
                    .media
                    .iter()
                    .find(|r| r.sha256 == descriptor.sha256)
                {
                    ensure!(prior == &reference, "selected media reference collision");
                } else {
                    selected.media.push(reference);
                }
                selected
                    .media_coverage
                    .retain(|c| c.hash() != descriptor.sha256);
                selected.media_coverage.push(Coverage::Included {
                    sha256: descriptor.sha256,
                });
            }
        }
        selected.records.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        selected.media.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        selected
            .media_coverage
            .sort_by(|a, b| a.hash().cmp(b.hash()));
        let transaction = self.next_selection(
            inner.generation,
            &expected.scope,
            Some(&previous),
            SelectionChange {
                operation,
                accepted_base_sha256: previous.accepted_base_sha256.clone(),
                selected,
                retained: previous.retained.clone(),
            },
            fingerprint,
        )?;
        self.write_selection(
            &mut inner,
            &root,
            Some(&previous),
            transaction,
            objects,
            &chain,
        )
    }
}
