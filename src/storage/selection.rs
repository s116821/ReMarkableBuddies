//! Domain-opaque selected references. No domain/native admission or publisher.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionScope {
    pub group: Uuid,
    pub key_sha256: String,
    /// Digest of the selected provider/account/application binding, not credentials.
    pub binding_sha256: String,
}
impl SelectionScope {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.group.is_nil(), "nil selected group");
        ensure!(
            valid_digest(&self.key_sha256) && valid_digest(&self.binding_sha256),
            "invalid selected scope"
        );
        Ok(())
    }
    fn filename(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{}.json", digest(&serde_json::to_vec(self)?)))
    }
}

/// One durable transaction must reference winner AND unresolved evidence together.
/// Domain owners validate ownership/intent state; storage keeps payloads opaque.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionTransaction {
    pub format: u32,
    pub operation: Uuid,
    pub store_generation: Uuid,
    pub aggregate_generation: Uuid,
    pub scope: SelectionScope,
    pub accepted_base_sha256: String,
    pub selected: Manifest,
    pub retained: Vec<Manifest>,
}
impl SelectionTransaction {
    pub fn validate(&self, limit: usize) -> Result<()> {
        self.scope.validate()?;
        ensure!(
            (1..=MAX_ITEMS).contains(&limit),
            "invalid selected snapshot bound"
        );
        ensure!(
            self.format == FORMAT
                && !self.operation.is_nil()
                && !self.store_generation.is_nil()
                && !self.aggregate_generation.is_nil(),
            "invalid selection transaction identity"
        );
        ensure!(
            valid_digest(&self.accepted_base_sha256),
            "invalid accepted base digest"
        );
        ensure!(
            self.retained.len() <= limit,
            "retained closure exceeds bound"
        );
        self.selected.validate()?;
        ensure!(
            self.selected.scope == Scope::SelectedRecords
                && self
                    .retained
                    .iter()
                    .all(|m| m.scope == Scope::SelectedRecords),
            "selection requires explicit reference closures"
        );
        let selected: BTreeSet<_> = self.selected.records.iter().map(|r| &r.sha256).collect();
        let mut count = 0usize;
        let mut bytes = 0u64;
        for manifest in std::iter::once(&self.selected).chain(&self.retained) {
            manifest.validate()?;
            count = count
                .checked_add(manifest.records.len() + manifest.media.len())
                .context("selected item overflow")?;
            bytes = bytes
                .checked_add(manifest.records.iter().map(|r| r.bytes).sum::<u64>())
                .context("selected byte overflow")?;
            ensure!(
                count <= limit && bytes <= MAX_METADATA as u64,
                "selected closure exceeds bound"
            );
        }
        for manifest in &self.retained {
            ensure!(
                manifest
                    .records
                    .iter()
                    .all(|r| !selected.contains(&r.sha256)),
                "retained evidence is also active membership"
            );
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_METADATA,
            "selection metadata exceeds bound"
        );
        Ok(())
    }
}

/// Only the store can mint this live token. Persisted pins are evidence, not tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionToken {
    scope: SelectionScope,
    store_generation: Uuid,
    aggregate_generation: Uuid,
    accepted_base_sha256: String,
    selection_sha256: String,
}
impl SelectionToken {
    pub fn scope(&self) -> &SelectionScope {
        &self.scope
    }
    pub fn store_generation(&self) -> Uuid {
        self.store_generation
    }
    pub fn aggregate_generation(&self) -> Uuid {
        self.aggregate_generation
    }
    pub fn accepted_base_sha256(&self) -> &str {
        &self.accepted_base_sha256
    }
    pub fn selection_sha256(&self) -> &str {
        &self.selection_sha256
    }
}

pub struct SelectedSnapshot {
    pub token: SelectionToken,
    pub transaction: SelectionTransaction,
    /// Exact immutable envelopes, never a later lookup of all-history heads.
    pub selected_records: Vec<Envelope>,
    pub retained_records: Vec<Envelope>,
    pub media: BTreeMap<String, files::Source>,
}

impl Store {
    /// Read an existing complete selection. Absence is not an empty winner or
    /// permission to bootstrap/publish. No activation writer is enabled here.
    pub fn selected_snapshot(
        &self,
        scope: &SelectionScope,
        limit: usize,
    ) -> Result<Option<SelectedSnapshot>> {
        scope.validate()?;
        ensure!(
            (1..=MAX_ITEMS).contains(&limit),
            "invalid selected snapshot bound"
        );
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let root = self.generation(inner.generation);
        let path = root.join("selections").join(scope.filename()?);
        files::safe_path(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let transaction: SelectionTransaction = files::json(&path, MAX_METADATA as u64)?;
        transaction.validate(limit)?;
        ensure!(
            &transaction.scope == scope && transaction.store_generation == inner.generation,
            "selection scope/store generation mismatch"
        );
        let token = SelectionToken {
            scope: transaction.scope.clone(),
            store_generation: inner.generation,
            aggregate_generation: transaction.aggregate_generation,
            accepted_base_sha256: transaction.accepted_base_sha256.clone(),
            selection_sha256: digest(&serde_json::to_vec(&transaction)?),
        };
        // Pin one immutable generation path before disk/media validation. Do not
        // follow a new CURRENT or new selection while opening the closure.
        drop(inner);
        let selected_records = Self::validate_objects(&root, &transaction.selected)?;
        let mut selected_keys = BTreeSet::new();
        for (record, _) in &selected_records {
            ensure!(
                selected_keys.insert((record.namespace, record.record_id)),
                "conflicted selected record membership"
            );
        }
        let mut retained_records = Vec::new();
        for manifest in &transaction.retained {
            retained_records.extend(
                Self::validate_objects(&root, manifest)?
                    .into_iter()
                    .map(|(record, _)| record),
            );
        }
        let mut media = BTreeMap::new();
        for manifest in std::iter::once(&transaction.selected).chain(&transaction.retained) {
            for reference in &manifest.media {
                if let Some(existing) = media.get(&reference.sha256) {
                    let existing: &files::Source = existing;
                    ensure!(
                        existing.reference() == *reference,
                        "conflicting media reference"
                    );
                } else {
                    media.insert(
                        reference.sha256.clone(),
                        files::Source::open(
                            &root.join("objects").join(&reference.sha256),
                            reference,
                        )?,
                    );
                }
            }
        }
        Ok(Some(SelectedSnapshot {
            token,
            transaction,
            selected_records: selected_records
                .into_iter()
                .map(|(record, _)| record)
                .collect(),
            retained_records,
            media,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("buddy-selected-{}", Uuid::new_v4())))
        }
        fn store(&self) -> Store {
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn scope() -> SelectionScope {
        SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"scope"),
            binding_sha256: digest(b"binding"),
        }
    }
    fn commit(store: &Store) -> Manifest {
        let record = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: json!({"opaque":"fixture"}),
            media_descriptors: vec![],
        };
        let id = store.commit(vec![record], BTreeMap::new()).unwrap();
        store
            .manifests()
            .unwrap()
            .into_iter()
            .find(|m| m.transaction_id == id)
            .unwrap()
    }
    fn fixture_transaction(store: &Store, scope: SelectionScope) -> SelectionTransaction {
        let store_generation = store.inner.lock().unwrap().generation;
        let mut selected = commit(store);
        selected.scope = Scope::SelectedRecords;
        let mut retained = commit(store);
        retained.scope = Scope::SelectedRecords;
        SelectionTransaction {
            format: FORMAT,
            operation: Uuid::new_v4(),
            store_generation,
            aggregate_generation: Uuid::new_v4(),
            scope,
            accepted_base_sha256: digest(b"accepted base"),
            selected,
            retained: vec![retained],
        }
    }
    fn persist_fixture(store: &Store, transaction: &SelectionTransaction) {
        let path = store
            .generation(transaction.store_generation)
            .join("selections")
            .join(transaction.scope.filename().unwrap());
        files::directory(path.parent().unwrap()).unwrap();
        files::atomic_json(&path, transaction).unwrap();
    }
    #[test]
    fn absent_selection_is_not_an_empty_winner() {
        let f = Fixture::new();
        let store = f.store();
        assert!(store
            .selected_snapshot(&scope(), MAX_ITEMS)
            .unwrap()
            .is_none());
    }
    #[test]
    fn one_transaction_reopens_winner_and_retained_refs_together() {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let t = fixture_transaction(&store, s.clone());
        persist_fixture(&store, &t);
        drop(store);
        let store = f.store();
        let snapshot = store.selected_snapshot(&s, MAX_ITEMS).unwrap().unwrap();
        assert_eq!(snapshot.selected_records.len(), 1);
        assert_eq!(snapshot.retained_records.len(), 1);
        assert_eq!(
            snapshot.token.aggregate_generation(),
            t.aggregate_generation
        );
        assert_eq!(
            snapshot.token.accepted_base_sha256(),
            t.accepted_base_sha256
        );
    }
    #[test]
    fn corrupted_or_wrong_generation_transaction_refuses() {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let mut t = fixture_transaction(&store, s.clone());
        let generation = t.store_generation;
        t.store_generation = Uuid::new_v4();
        let path = store
            .generation(generation)
            .join("selections")
            .join(s.filename().unwrap());
        files::directory(path.parent().unwrap()).unwrap();
        files::atomic_json(&path, &t).unwrap();
        assert!(store.selected_snapshot(&s, MAX_ITEMS).is_err());
        fs::write(&path, b"broken").unwrap();
        assert!(store.selected_snapshot(&s, MAX_ITEMS).is_err());
    }
    #[test]
    fn retained_active_overlap_and_combined_bounds_refuse() {
        let f = Fixture::new();
        let store = f.store();
        let mut t = fixture_transaction(&store, scope());
        assert!(t.validate(1).is_err());
        t.retained = vec![t.selected.clone()];
        assert!(t.validate(MAX_ITEMS).is_err());
    }
}
