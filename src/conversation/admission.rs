//! Canonical in-process selected admission. This is not native qualification.
use crate::storage::selection::{
    SelectedSnapshot, SelectionChange, SelectionPublication, SelectionScope, SelectionToken,
};
use crate::storage::{Store, Uuid, MAX_ITEMS};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

type AdmissionKey = (usize, Uuid, String);
type Registry = BTreeMap<AdmissionKey, Weak<Mutex<()>>>;
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

/// All handles for the same Store and aggregate share admission, including a
/// replacement binding. Callers still own qualified source/backend validation.
pub struct SelectedAdmission {
    store: Arc<Store>,
    scope: SelectionScope,
    gate: Arc<Mutex<()>>,
}
impl SelectedAdmission {
    pub fn new(store: Arc<Store>, scope: SelectionScope) -> Result<Self> {
        scope.validate()?;
        let key = (
            Arc::as_ptr(&store) as usize,
            scope.group,
            scope.key_sha256.clone(),
        );
        let mut registry = REGISTRY
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| anyhow::anyhow!("domain admission registry unavailable"))?;
        registry.retain(|_, weak| weak.strong_count() > 0);
        let gate = registry
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let gate = Arc::new(Mutex::new(()));
                registry.insert(key, Arc::downgrade(&gate));
                gate
            });
        drop(registry);
        Ok(Self { store, scope, gate })
    }
    /// Domain lock precedes every Store read/mutation. No Store mutex is held
    /// during the supplied synchronous operation. Keep this closure through the
    /// actual backend submission; a returned historical value is not permission.
    pub fn with_current<T>(
        &self,
        accepted: &SelectionToken,
        operation: impl FnOnce(&SelectedSnapshot) -> Result<T>,
    ) -> Result<T> {
        let _admission = self
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("domain admission unavailable"))?;
        ensure!(
            accepted.scope() == &self.scope,
            "foreign selected admission scope"
        );
        let snapshot = self
            .store
            .selected_snapshot(&self.scope, MAX_ITEMS)?
            .context("selected admission absent")?;
        ensure!(
            &snapshot.token == accepted,
            "selected admission replaced or stale"
        );
        operation(&snapshot)
    }
    /// Publication/activation must use the same gate as synchronous handoff.
    fn mutate<T>(&self, operation: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let _admission = self
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("domain admission unavailable"))?;
        operation(&self.store)
    }
    pub fn initialize(
        &self,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        self.mutate(|store| store.initialize_selected(&self.scope, change, objects))
    }
    pub fn activate(
        &self,
        accepted: &SelectionToken,
        change: SelectionChange,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        ensure!(
            accepted.scope() == &self.scope,
            "foreign selected activation scope"
        );
        self.mutate(|store| store.activate_selected(accepted, change, objects))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::digest;
    #[test]
    fn multiple_handles_and_replacement_bindings_share_only_their_aggregate_gate() {
        // Identity registry behavior does not require native operations.
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture =
            Fixture(std::env::temp_dir().join(format!("buddy-admission-{}", Uuid::new_v4())));
        let store = Arc::new(
            Store::open(crate::storage::StorePaths {
                data: fixture.0.join("data"),
                cache: fixture.0.join("cache"),
                credentials: fixture.0.join("secrets"),
            })
            .unwrap(),
        );
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"document"),
            binding_sha256: digest(b"first binding"),
        };
        let first = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let mut replaced = scope.clone();
        replaced.binding_sha256 = digest(b"replacement");
        let second = SelectedAdmission::new(store.clone(), replaced).unwrap();
        assert!(Arc::ptr_eq(&first.gate, &second.gate));
        let held = first.gate.lock().unwrap();
        assert!(second.gate.try_lock().is_err());
        let mut unrelated = scope;
        unrelated.key_sha256 = digest(b"other document");
        let other = SelectedAdmission::new(store, unrelated).unwrap();
        assert!(other.gate.try_lock().is_ok());
        drop(held);
        assert!(second.gate.try_lock().is_ok());
    }
    #[test]
    fn synchronous_handoff_allows_store_reads_but_serializes_activation_and_refuses_stale_token() {
        use crate::storage::{
            Envelope, Kind, Manifest, Namespace, ObjectRef, Scope, StorePaths, FORMAT,
        };
        use std::sync::Barrier;
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(
            std::env::temp_dir().join(format!("buddy-admission-current-{}", Uuid::new_v4())),
        );
        let store = Arc::new(
            Store::open(StorePaths {
                data: fixture.0.join("data"),
                cache: fixture.0.join("cache"),
                credentials: fixture.0.join("secrets"),
            })
            .unwrap(),
        );
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"document"),
            binding_sha256: digest(b"binding"),
        };
        let first = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let second = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
        let envelope = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::json!({"domain-opaque":"admission mechanism fixture"}),
            media_descriptors: vec![],
        };
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let hash = digest(&bytes);
        let mut change = SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"base"),
            retained: vec![],
            selected: Manifest {
                format: FORMAT,
                transaction_id: Uuid::new_v4(),
                scope: Scope::SelectedRecords,
                records: vec![ObjectRef {
                    sha256: hash.clone(),
                    bytes: bytes.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
                media: vec![],
                media_coverage: vec![],
            },
        };
        let accepted = first
            .initialize(change.clone(), BTreeMap::from([(hash, bytes)]))
            .unwrap();
        change.operation = Uuid::new_v4();
        change.selected.transaction_id = Uuid::new_v4();
        let barrier = Arc::new(Barrier::new(2));
        let mut worker = None;
        first
            .with_current(&accepted.token, |snapshot| {
                assert_eq!(
                    store.selected_snapshot(&scope, MAX_ITEMS)?.unwrap().token,
                    snapshot.token
                );
                assert!(second.gate.try_lock().is_err());
                let started = barrier.clone();
                let token = accepted.token.clone();
                worker = Some(std::thread::spawn(move || {
                    started.wait();
                    second.activate(&token, change, BTreeMap::new())
                }));
                barrier.wait();
                // This represents the entire synchronous backend interval. Domain
                // admission is still held; the Store mutex is independently usable.
                assert_eq!(
                    store.selected_snapshot(&scope, MAX_ITEMS)?.unwrap().token,
                    snapshot.token
                );
                Ok(())
            })
            .unwrap();
        let replaced = worker.unwrap().join().unwrap().unwrap();
        assert_ne!(replaced.token, accepted.token);
        assert_eq!(
            replaced.transaction.selected.records,
            accepted.transaction.selected.records
        );
        let mut submitted = false;
        assert!(first
            .with_current(&accepted.token, |_| {
                submitted = true;
                Ok(())
            })
            .is_err());
        assert!(!submitted);
    }
}
