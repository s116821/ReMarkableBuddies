use remarkable_reader_buddy::{
    conversation::AttemptReason,
    conversation_feedback::FeedbackKind,
    storage::*,
    unassociated_feedback::{Feedback, Observation},
};
use std::{collections::BTreeMap, fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("feedback-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn root(&self) -> PathBuf {
        let id: Uuid =
            serde_json::from_slice(&fs::read(self.paths().data.join("CURRENT")).unwrap()).unwrap();
        self.paths().data.join("generations").join(id.to_string())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn explicit_preparation_reopen_and_exact_literal_identity() {
    let f = Fixture::new();
    let store = Store::open(f.paths()).unwrap();
    assert_eq!(fs::read(f.root().join("format.json")).unwrap(), b"1");
    let before = fs::read(f.paths().data.join("contract-v1.json")).unwrap();
    drop(store);
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let (wake, events) = std::sync::mpsc::sync_channel(2);
    feedback.store().set_wake(Some(wake));
    let op = Uuid::new_v4();
    let first = feedback
        .retain(op, 123, AttemptReason::ProviderFailure, false)
        .unwrap();
    events.try_recv().unwrap();
    assert_eq!(
        first,
        feedback
            .retain(op, 123, AttemptReason::ProviderFailure, false)
            .unwrap()
    );
    assert!(events.try_recv().is_err());
    assert!(feedback
        .retain(op, 124, AttemptReason::ProviderFailure, false)
        .is_err());
    assert_eq!(feedback.snapshot().unwrap(), vec![first.clone()]);
    drop(feedback);
    let reopened = Feedback::prepare(f.paths()).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), vec![first]);
    assert_eq!(
        fs::read(f.paths().data.join("contract-v1.json")).unwrap(),
        before
    );
    let caps: serde_json::Value =
        serde_json::from_slice(&fs::read(f.paths().data.join("capabilities.json")).unwrap())
            .unwrap();
    assert_eq!(caps["contract_version"], 2);
    assert_eq!(caps["namespaces"].as_array().unwrap().len(), 6);
}
#[test]
fn object_failure_and_committed_ambiguous_return_are_distinct() {
    let f = Fixture::new();
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let op = Uuid::new_v4();
    feedback.store().set_fault(Fault::AfterObjects).unwrap();
    assert!(feedback
        .retain(op, 1, AttemptReason::RequestFailed, false)
        .is_err());
    assert!(feedback.snapshot().unwrap().is_empty());
    feedback.store().set_fault(Fault::AfterCommit).unwrap();
    let result = feedback
        .retain(op, 1, AttemptReason::RequestFailed, false)
        .unwrap();
    assert_eq!(feedback.snapshot().unwrap(), vec![result.clone()]);
    drop(feedback);
    assert_eq!(
        Feedback::prepare(f.paths()).unwrap().snapshot().unwrap(),
        vec![result]
    );
}
#[test]
fn direct_import_cannot_change_identity_or_private_payload() {
    let f = Fixture::new();
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let op = Uuid::new_v4();
    let first = feedback
        .retain(op, 7, AttemptReason::DeviceUncertain, false)
        .unwrap();
    assert_eq!(first.kind, FeedbackKind::Reconciliation);
    let mut envelope = Observation {
        schema: 1,
        actor: feedback.actor(),
        operation: op,
        observed_at: 7,
        reason: AttemptReason::DeviceUncertain,
        reconciliation: false,
    }
    .envelope()
    .unwrap();
    let count = feedback.store().manifests().unwrap().len();
    envelope.payload["raw_message"] =
        serde_json::json!("Bearer SECRET request answer auth@example.invalid");
    assert!(feedback
        .store()
        .commit(vec![envelope.clone()], BTreeMap::new())
        .is_err());
    envelope
        .payload
        .as_object_mut()
        .unwrap()
        .remove("raw_message");
    envelope.kind = Kind::Tombstone;
    envelope.payload = serde_json::Value::Null;
    assert!(feedback
        .store()
        .commit(vec![envelope], BTreeMap::new())
        .is_err());
    assert_eq!(feedback.store().manifests().unwrap().len(), count);
    assert!(feedback
        .retain(Uuid::nil(), 1, AttemptReason::RequestFailed, false)
        .is_err());
    assert!(!serde_json::to_string(&feedback.snapshot().unwrap())
        .unwrap()
        .contains("SECRET"));
}
#[test]
fn capacity_is_atomic_at_real_store_boundary() {
    let f = Fixture::new();
    let feedback = Feedback::prepare(f.paths()).unwrap();
    let records = (0..MAX_ITEMS - 1)
        .map(|n| {
            Observation {
                schema: 1,
                actor: feedback.actor(),
                operation: Uuid::new_v5(&feedback.actor(), format!("capacity-{n}").as_bytes()),
                observed_at: n as u64,
                reason: AttemptReason::NoSuccessor,
                reconciliation: false,
            }
            .envelope()
            .unwrap()
        })
        .collect();
    feedback.store().commit(records, BTreeMap::new()).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            feedback.retain(Uuid::new_v4(), 1, AttemptReason::RequestFailed, false)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            feedback.retain(Uuid::new_v4(), 2, AttemptReason::RequestFailed, false)
        });
        assert_ne!(a.join().unwrap().is_ok(), b.join().unwrap().is_ok());
    });
    assert_eq!(feedback.snapshot().unwrap().len(), MAX_ITEMS);
}
