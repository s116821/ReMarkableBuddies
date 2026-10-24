//! Private ordinary read-set admission. No selected or maintenance authority.
use super::*;
use std::{io::Read, sync::Weak};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRef {
    pub namespace: Namespace,
    pub record: Uuid,
    pub revision: Uuid,
    pub object: ObjectRef,
}
pub(crate) struct Captured {
    store: Weak<Store>,
    pub generation: Uuid,
    pub actor: Uuid,
    conversation: Uuid,
    matches: fn(&Envelope, Uuid) -> bool,
    pub records: Vec<(Envelope, RecordRef)>,
}
impl Captured {
    pub fn digest(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(
            &self.records.iter().map(|(_, r)| r).collect::<Vec<_>>(),
        )?))
    }
}
#[derive(Default)]
pub(crate) struct ReadBudget {
    bytes: u64,
    visits: usize,
    cache: BTreeMap<Uuid, (RecordRef, Envelope)>,
}
impl ReadBudget {
    pub fn charge(&mut self, bytes: u64) -> Result<()> {
        self.visits = self
            .visits
            .checked_add(1)
            .context("reference work overflow")?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .context("reference byte overflow")?;
        ensure!(
            self.visits <= 8192 && self.bytes <= MAX_METADATA as u64,
            "historical reference work bound"
        );
        Ok(())
    }
}
fn capture_records(
    inner: &Inner,
    conversation: Uuid,
    matches: fn(&Envelope, Uuid) -> bool,
) -> Result<Vec<(Envelope, RecordRef)>> {
    let mut records = Vec::new();
    let mut bytes = 0u64;
    for heads in inner.index.heads.values() {
        if !heads.iter().any(|id| {
            inner
                .index
                .records
                .get(id)
                .is_some_and(|(e, _)| matches(e, conversation))
        }) {
            continue;
        }
        for id in heads {
            ensure!(records.len() < MAX_ITEMS, "captured head bound");
            let (e, _) = inner.index.records.get(id).context("head absent")?;
            let hash = &inner.index.records.get(id).context("head absent")?.1;
            let object = inner
                .index
                .record_refs
                .get(hash)
                .context("original committed reference absent")?
                .clone();
            ensure!(object.bytes <= MAX_RECORD as u64, "original record bound");
            let reference = RecordRef {
                namespace: e.namespace,
                record: e.record_id,
                revision: *id,
                object,
            };
            bytes = bytes
                .checked_add(reference.object.bytes)
                .context("capture byte overflow")?;
            ensure!(bytes <= MAX_METADATA as u64, "captured metadata bound");
            records.push((e.clone(), reference));
        }
    }
    Ok(records)
}
impl Store {
    pub(crate) fn bounded_heads(
        &self,
        namespace: Namespace,
        record: Uuid,
        limit: usize,
    ) -> Result<Vec<Envelope>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let Some(heads) = inner.index.heads.get(&(namespace, record)) else {
            return Ok(vec![]);
        };
        ensure!(
            heads.len() <= limit.min(MAX_ITEMS),
            "historical head count bound"
        );
        let mut bytes = 0u64;
        for revision in heads {
            let (_, hash) = inner
                .index
                .records
                .get(revision)
                .context("historical head absent")?;
            bytes = bytes
                .checked_add(
                    inner
                        .index
                        .record_refs
                        .get(hash)
                        .context("historical object absent")?
                        .bytes,
                )
                .context("head byte overflow")?;
            ensure!(bytes <= MAX_METADATA as u64, "historical head byte bound");
        }
        Ok(heads
            .iter()
            .map(|id| inner.index.records[id].0.clone())
            .collect())
    }
    pub(crate) fn capture(
        self: &Arc<Self>,
        conversation: Uuid,
        matches: fn(&Envelope, Uuid) -> bool,
    ) -> Result<Captured> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        Ok(Captured {
            store: Arc::downgrade(self),
            generation: inner.generation,
            actor: self.actor_id,
            conversation,
            matches,
            records: capture_records(&inner, conversation, matches)?,
        })
    }
    pub(crate) fn commit_captured(
        self: &Arc<Self>,
        captured: &Captured,
        records: Vec<Envelope>,
    ) -> Result<Uuid> {
        ensure!(
            Weak::ptr_eq(&captured.store, &Arc::downgrade(self)),
            "preparation belongs to another Store"
        );
        let (manifest, objects) = Self::ordinary_transaction(records, BTreeMap::new())?;
        manifest.validate()?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        ensure!(
            inner.generation == captured.generation,
            "stale captured generation"
        );
        let current = capture_records(&inner, captured.conversation, captured.matches)?;
        ensure!(
            current
                .iter()
                .map(|(_, r)| r)
                .eq(captured.records.iter().map(|(_, r)| r)),
            "stale captured heads"
        );
        let mut budget = ReadBudget::default();
        for (_, reference) in &captured.records {
            self.resolve_locked(&inner, reference, &mut budget)?;
        }
        let (receipt, changed) = self.publish_locked(&mut inner, manifest, objects, true)?;
        drop(inner);
        if changed {
            self.notify_commit();
        }
        Ok(receipt)
    }
    fn resolve_locked(
        &self,
        inner: &Inner,
        reference: &RecordRef,
        budget: &mut ReadBudget,
    ) -> Result<Envelope> {
        budget.charge(0)?;
        if let Some((old, envelope)) = budget.cache.get(&reference.revision) {
            ensure!(old == reference, "contradictory pinned ref");
            return Ok(envelope.clone());
        }
        let (e, hash) = inner
            .index
            .records
            .get(&reference.revision)
            .context("pinned revision absent")?;
        ensure!(
            e.namespace == reference.namespace
                && e.record_id == reference.record
                && hash == &reference.object.sha256,
            "pinned identity mismatch"
        );
        ensure!(
            reference.object.bytes <= MAX_RECORD as u64,
            "pinned record bound"
        );
        ensure!(
            inner.index.record_refs.get(hash) == Some(&reference.object),
            "pinned original ref mismatch"
        );
        // Charge the bounded file read and content-hash pass before either.
        budget.charge(
            reference
                .object
                .bytes
                .checked_mul(2)
                .context("reference work overflow")?,
        )?;
        let path = self.generation(inner.generation).join("objects").join(hash);
        files::safe_path(&path)?;
        ensure!(
            fs::symlink_metadata(&path)?.is_file(),
            "pinned regular file required"
        );
        let mut file = File::open(&path)?;
        ensure!(
            file.metadata()?.len() == reference.object.bytes,
            "pinned size mismatch"
        );
        let mut bytes = Vec::with_capacity(reference.object.bytes as usize);
        file.by_ref()
            .take(reference.object.bytes)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == reference.object.bytes
                && file.metadata()?.len() == reference.object.bytes,
            "pinned size changed"
        );
        ensure!(digest(&bytes) == *hash, "pinned bytes changed");
        let decoded: Envelope = serde_json::from_slice(&bytes)?;
        ensure!(&decoded == e, "pinned envelope mismatch");
        budget
            .cache
            .insert(reference.revision, (reference.clone(), decoded.clone()));
        Ok(decoded)
    }
    pub(crate) fn resolve_record(
        &self,
        reference: &RecordRef,
        budget: &mut ReadBudget,
    ) -> Result<Envelope> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        self.resolve_locked(&inner, reference, budget)
    }
    /// Exact original transaction, bounded and unambiguous; never selects a winner.
    pub(crate) fn original_transaction(
        &self,
        revision: Uuid,
        budget: &mut ReadBudget,
    ) -> Result<Vec<Envelope>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let (e, hash) = inner
            .index
            .records
            .get(&revision)
            .context("transaction revision absent")?;
        let transactions = inner
            .index
            .record_transactions
            .get(hash)
            .context("original transaction absent")?;
        ensure!(transactions.len() == 1, "ambiguous evidence transaction");
        let manifest = inner
            .index
            .manifests
            .get(transactions.first().unwrap())
            .context("transaction absent")?;
        ensure!(manifest.records.len() == 3, "evidence transaction size");
        let members = manifest.records.clone();
        let mut result = Vec::new();
        for r in members {
            let envelope = inner
                .index
                .records
                .values()
                .find(|(_, h)| h == &r.sha256)
                .map(|(e, _)| e)
                .context("transaction member absent")?;
            let reference = RecordRef {
                namespace: envelope.namespace,
                record: envelope.record_id,
                revision: envelope.revision_id,
                object: r,
            };
            result.push(self.resolve_locked(&inner, &reference, budget)?);
        }
        ensure!(
            result.iter().any(|r| r.revision_id == e.revision_id),
            "transaction identity mismatch"
        );
        Ok(result)
    }
}
