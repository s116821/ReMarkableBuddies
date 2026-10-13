use super::*;

/// Main supplies domain-validated settlement facts/latch evidence. Storage only
/// verifies original accepted publication and retained reference membership.
#[derive(Clone, Serialize)]
pub struct RetainedCommit {
    pub operation: Uuid,
    pub original_operation: Uuid,
    pub intent: ObjectRef,
    pub retained: Vec<Manifest>,
}

impl Store {
    pub fn commit_retained(
        &self,
        expected: &SelectionToken,
        change: RetainedCommit,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        ensure!(
            !change.operation.is_nil()
                && !change.original_operation.is_nil()
                && change.operation != change.original_operation,
            "invalid retained operation"
        );
        ensure!(
            valid_digest(&change.intent.sha256) && change.intent.bytes <= MAX_RECORD as u64,
            "invalid retained intent reference"
        );
        let fingerprint =
            publication::request("retained", &expected.scope, Some(expected), &change)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let root = self.generation(inner.generation);
        let previous = publication::current(&root, inner.generation, &expected.scope)?
            .context("selected scope is absent")?;
        let chain =
            publication::history(&root, inner.generation, &expected.scope, previous.clone())?;
        if let Some(receipt) = publication::replay(&chain, change.operation, &fingerprint)? {
            return Ok(receipt);
        }
        ensure!(
            publication::token(&previous)? == *expected,
            "stale selection token"
        );
        let original = chain
            .iter()
            .find(|t| t.operation == change.original_operation)
            .context("original accepted publication is absent")?;
        ensure!(
            original.mutation == SelectionMutation::Commit
                && original.selected.records.contains(&change.intent),
            "intent is outside original accepted publication"
        );
        ensure!(
            !previous.selected.records.contains(&change.intent)
                && previous
                    .retained
                    .iter()
                    .any(|m| m.records.contains(&change.intent)),
            "intent is not exclusively retained"
        );
        ensure!(
            change.retained.len() <= MAX_ITEMS,
            "retained closure exceeds bound"
        );
        // Settlement is additive. Pruning resolved evidence needs its own policy;
        // missing prior intent/media cannot silently clear a reconstructed latch.
        for manifest in &previous.retained {
            for reference in &manifest.records {
                ensure!(
                    change
                        .retained
                        .iter()
                        .any(|m| m.records.contains(reference)),
                    "retained record loss refused"
                );
            }
            for reference in &manifest.media {
                ensure!(
                    change.retained.iter().any(|m| m.media.contains(reference)),
                    "retained required media loss refused"
                );
            }
        }
        let active = Self::validate_objects(&root, &previous.selected)?;
        let active_index = publication::closure_index(&active, true)?;
        let mut old = BTreeMap::new();
        for manifest in &previous.retained {
            for item in Self::validate_objects(&root, manifest)? {
                old.insert(item.0.revision_id, item);
            }
        }
        let old_index = publication::closure_index(&old.into_values().collect::<Vec<_>>(), false)?;
        let transaction = self.next_selection(
            inner.generation,
            &expected.scope,
            Some(&previous),
            SelectionChange {
                operation: change.operation,
                accepted_base_sha256: previous.accepted_base_sha256.clone(),
                selected: previous.selected.clone(),
                retained: change.retained,
            },
            (fingerprint, SelectionMutation::Retained),
        )?;
        // Inspect exact immutable envelope bytes before publication. Existing
        // references are checked again by write_selection after durable staging.
        let mut new_keys = BTreeSet::new();
        for manifest in &transaction.retained {
            for reference in &manifest.records {
                let envelope: Envelope = if let Some(bytes) = objects.get(&reference.sha256) {
                    ensure!(
                        bytes.len() as u64 == reference.bytes && digest(bytes) == reference.sha256,
                        "retained object integrity failure"
                    );
                    serde_json::from_slice(bytes).context("unsupported retained envelope")?
                } else {
                    files::json(
                        &root.join("objects").join(&reference.sha256),
                        MAX_RECORD as u64,
                    )?
                };
                envelope.validate()?;
                if old_index.records.contains_key(&envelope.revision_id) {
                    continue;
                }
                let key = (envelope.namespace, envelope.record_id);
                ensure!(
                    !active_index.heads.contains_key(&key),
                    "retained settlement cannot revise active record keys"
                );
                ensure!(
                    new_keys.insert(key),
                    "multiple retained edits of one record"
                );
                let heads = old_index.heads.get(&key).cloned().unwrap_or_default();
                if heads != envelope.parents {
                    return Err(Conflict { heads }.into());
                }
            }
        }
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
