//! Conditional ordinary-record maintenance. Persisted evidence never mints a token.
use super::*;

#[derive(Debug)]
pub(crate) struct Refused;
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("conditional record changed")
    }
}
impl std::error::Error for Refused {}
fn check(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Refused.into())
    }
}

fn heads(inner: &Inner, namespace: Namespace, record: Uuid) -> Result<Vec<Envelope>> {
    ensure!(
        inner.index.unavailable == 0,
        "unavailable committed history"
    );
    let revisions = inner
        .index
        .heads
        .get(&(namespace, record))
        .cloned()
        .unwrap_or_default();
    ensure!(revisions.len() <= MAX_ITEMS, "conditional head bound");
    let mut bytes = 0usize;
    revisions
        .iter()
        .map(|revision| {
            let envelope = &inner
                .index
                .records
                .get(revision)
                .context("conditional head absent")?
                .0;
            bytes = bytes
                .checked_add(serde_json::to_vec(envelope)?.len())
                .context("conditional bytes overflow")?;
            ensure!(bytes <= MAX_METADATA, "conditional metadata bound");
            Ok(envelope.clone())
        })
        .collect()
}
fn content(
    inner: &Inner,
    namespace: Namespace,
    record: Uuid,
    expected: &BTreeMap<Uuid, String>,
) -> Result<()> {
    ensure!(
        !expected.is_empty() && expected.len() <= MAX_ITEMS,
        "conditional expected bound"
    );
    for (revision, sha) in expected {
        check(valid_digest(sha))?;
        let parent = &inner.index.records.get(revision).ok_or(Refused)?.0;
        check(
            parent.namespace == namespace
                && parent.record_id == record
                && digest(&serde_json::to_vec(parent)?) == *sha,
        )?;
    }
    Ok(())
}
fn current(
    inner: &Inner,
    generation: Uuid,
    namespace: Namespace,
    record: Uuid,
    expected: &BTreeMap<Uuid, String>,
) -> Result<()> {
    check(inner.generation == generation)?;
    let actual = heads(inner, namespace, record)?;
    check(
        actual
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>()
            == expected.keys().copied().collect(),
    )?;
    content(inner, namespace, record, expected)
}
fn receipt(
    inner: &Inner,
    envelope: &Envelope,
    expected: &BTreeMap<Uuid, String>,
) -> Result<Option<Uuid>> {
    ensure!(
        inner.index.unavailable == 0,
        "unavailable committed history"
    );
    content(inner, envelope.namespace, envelope.record_id, expected)?;
    let Some(hash) = inner.index.operations.get(&envelope.operation_id) else {
        return Ok(None);
    };
    let encoded = serde_json::to_vec(envelope)?;
    check(
        *hash == digest(&encoded)
            && inner
                .index
                .records
                .get(&envelope.revision_id)
                .is_some_and(|(e, _)| e == envelope),
    )?;
    ensure!(
        inner.index.manifests.len() <= MAX_ITEMS,
        "conditional receipt manifest bound"
    );
    let reference = ObjectRef {
        sha256: hash.clone(),
        bytes: encoded.len() as u64,
    };
    let original = inner
        .index
        .manifests
        .values()
        .find(|m| {
            m.scope == Scope::LocalTransaction
                && m.records == [reference.clone()]
                && m.media.is_empty()
                && m.media_coverage.is_empty()
                && m.record_namespaces == BTreeMap::from([(hash.clone(), envelope.namespace)])
        })
        .map(|m| m.transaction_id)
        .context("original receipt manifest unresolved")?;
    Ok(Some(original))
}
impl Store {
    pub(crate) fn conditional_snapshot(
        &self,
        namespace: Namespace,
        record: Uuid,
    ) -> Result<(Uuid, Vec<Envelope>)> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        Ok((inner.generation, heads(&inner, namespace, record)?))
    }
    pub(crate) fn with_conditional_heads<T>(
        &self,
        generation: Uuid,
        namespace: Namespace,
        record: Uuid,
        expected: &BTreeMap<Uuid, String>,
        consume: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        current(&inner, generation, namespace, record, expected)?;
        consume()
    }
    pub(crate) fn with_conditional_receipt<T>(
        &self,
        generation: Uuid,
        envelope: &Envelope,
        expected: &BTreeMap<Uuid, String>,
        transaction: Uuid,
        consume: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        check(
            inner.generation == generation
                && receipt(&inner, envelope, expected)? == Some(transaction),
        )?;
        consume()
    }
    pub(crate) fn conditional_receipt(
        &self,
        generation: Uuid,
        envelope: &Envelope,
        expected: &BTreeMap<Uuid, String>,
    ) -> Result<Option<Uuid>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        check(inner.generation == generation)?;
        receipt(&inner, envelope, expected)
    }
    pub(crate) fn conditional_commit(
        &self,
        generation: Uuid,
        envelope: Envelope,
        expected: &BTreeMap<Uuid, String>,
        attempted: &mut bool,
    ) -> Result<Uuid> {
        envelope.validate()?;
        check(
            envelope.actor_id == self.actor_id
                && envelope.kind == Kind::Tombstone
                && envelope.payload.is_null()
                && envelope.media_descriptors.is_empty()
                && envelope.parents == expected.keys().copied().collect(),
        )?;
        let encoded = serde_json::to_vec(&envelope)?;
        let sha = digest(&encoded);
        let manifest = Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::LocalTransaction,
            records: vec![ObjectRef {
                sha256: sha.clone(),
                bytes: encoded.len() as u64,
            }],
            record_namespaces: BTreeMap::from([(sha.clone(), envelope.namespace)]),
            media: vec![],
            media_coverage: vec![],
        };
        manifest.validate()?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        check(inner.generation == generation)?;
        if let Some(original) = receipt(&inner, &envelope, expected)? {
            return Ok(original);
        }
        // ONE guard spans original generation/content/head checks and publication.
        // Restore/migrate cannot replace this generation between check and commit.
        current(
            &inner,
            generation,
            envelope.namespace,
            envelope.record_id,
            expected,
        )?;
        *attempted = true;
        let (receipt, changed) =
            self.publish_locked(&mut inner, manifest, BTreeMap::from([(sha, encoded)]), true)?;
        drop(inner);
        if changed {
            self.notify_commit();
        }
        Ok(receipt)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{self, Provider};
    use std::sync::mpsc;
    struct Fixture {
        root: PathBuf,
        store: Arc<Store>,
        memory: memory::LocalMemory,
        key: memory::Key,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("conditional-record-{}", Uuid::new_v4()));
            let store = Arc::new(
                Store::open(StorePaths {
                    data: root.join("data"),
                    cache: root.join("cache"),
                    credentials: root.join("credentials"),
                })
                .unwrap(),
            );
            let profile = Uuid::new_v4();
            let key = memory::Key {
                profile,
                scope: memory::Scope::Global,
                fact: Uuid::new_v4(),
            };
            let memory = memory::LocalMemory::new_local(store.clone(), profile).unwrap();
            let fact = memory::Fact {
                key: key.clone(),
                claim: "units".into(),
                value: "SI".into(),
                subject: "physics".into(),
                tags: vec![],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "fixture".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 8000,
                observed_ms: 1,
                expires_ms: None,
            };
            memory
                .put(memory::Mutation {
                    operation: Uuid::new_v4(),
                    expected: BTreeSet::new(),
                    fact,
                })
                .unwrap();
            Self {
                root,
                store,
                memory,
                key,
            }
        }
        fn snapshot(&self) -> (Uuid, BTreeMap<Uuid, String>, Envelope) {
            let (generation, heads) = self
                .store
                .conditional_snapshot(Namespace::SubjectMemory, self.key.record_id().unwrap())
                .unwrap();
            let expected = heads
                .iter()
                .map(|e| (e.revision_id, digest(&serde_json::to_vec(e).unwrap())))
                .collect::<BTreeMap<_, _>>();
            let envelope = crate::memory::prepared::Intent {
                prepared_version: 1,
                operation: Uuid::new_v4(),
                key: self.key.clone(),
                expected: heads
                    .iter()
                    .map(|e| crate::memory::prepared::Expected {
                        revision: e.revision_id,
                        sha256: digest(&serde_json::to_vec(e).unwrap()),
                    })
                    .collect(),
                binding: crate::memory::prepared::Binding {
                    config_sha256: digest(b"unused domain helper test"),
                    mode: crate::memory::prepared::Mode {
                        actor: self.store.actor_id,
                        sha256: digest(b"mode"),
                    },
                    actor: self.store.actor_id,
                    store_generation: generation,
                    domain: crate::memory::prepared::Domain::Local {},
                },
            }
            .envelope(&self.store)
            .unwrap();
            (generation, expected, envelope)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn prepared_generation_refuses_restore_with_identical_heads_and_original_mode_token() {
        let f = Fixture::new();
        let (generation, expected, envelope) = f.snapshot();
        let original_mode = f.store.sync_mode_snapshot().unwrap().token;
        let _lease = f.store.ordinary_mode_lease(&original_mode).unwrap();
        let backup = f.root.join("backup");
        f.store.export(&backup).unwrap();
        f.store.restore(&backup).unwrap();
        let (next, current) = f
            .store
            .conditional_snapshot(Namespace::SubjectMemory, envelope.record_id)
            .unwrap();
        assert_ne!(next, generation);
        assert_eq!(
            current
                .iter()
                .map(|e| e.revision_id)
                .collect::<BTreeSet<_>>(),
            envelope.parents
        );
        assert_eq!(f.store.sync_mode_snapshot().unwrap().token, original_mode);
        let before = f.store.manifests().unwrap();
        let mut attempted = false;
        assert!(f
            .store
            .conditional_commit(generation, envelope, &expected, &mut attempted)
            .unwrap_err()
            .is::<Refused>());
        assert!(!attempted);
        assert_eq!(f.store.manifests().unwrap(), before);
        assert_eq!(f.memory.inspect(&f.key).unwrap()[0].kind, Kind::Value);
    }
    #[test]
    fn prepared_writer_holds_generation_mutex_until_consumed() {
        let f = Fixture::new();
        let (generation, expected, envelope) = f.snapshot();
        let backup = f.root.join("backup");
        f.store.export(&backup).unwrap();
        let (start_tx, start_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let store = f.store.clone();
        let worker = std::thread::spawn(move || {
            start_rx.recv().unwrap();
            let result = store.restore(&backup);
            done_tx.send(result).unwrap();
        });
        f.store
            .with_conditional_heads(
                generation,
                Namespace::SubjectMemory,
                envelope.record_id,
                &expected,
                || {
                    // This is the actual callback under the real Store guard, not elapsed-time proof.
                    assert!(f.store.inner.try_lock().is_err());
                    start_tx.send(()).unwrap();
                    assert!(done_rx.try_recv().is_err());
                    Ok(())
                },
            )
            .unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_ne!(
            f.store
                .conditional_snapshot(Namespace::SubjectMemory, envelope.record_id)
                .unwrap()
                .0,
            generation
        );
    }
    #[test]
    fn prepared_commit_faults_are_uncertain_and_receipt_is_pure_and_content_bound() {
        for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
            let f = Fixture::new();
            let (generation, expected, envelope) = f.snapshot();
            f.store.set_fault(fault).unwrap();
            let mut attempted = false;
            assert!(f
                .store
                .conditional_commit(generation, envelope.clone(), &expected, &mut attempted)
                .is_err());
            assert!(attempted);
            let before = f.store.manifests().unwrap();
            let receipt = f
                .store
                .conditional_receipt(generation, &envelope, &expected)
                .unwrap();
            assert_eq!(receipt.is_some(), fault == Fault::AfterCommit);
            assert_eq!(f.store.manifests().unwrap(), before);
            let mut forged = expected.clone();
            *forged.values_mut().next().unwrap() = digest(b"tampered");
            assert!(f
                .store
                .conditional_receipt(generation, &envelope, &forged)
                .is_err());
            assert_eq!(f.store.manifests().unwrap(), before);
        }
    }
}
