//! Actual Store commit contention while its existing snapshot predicate runs.
use super::*;
use std::sync::{mpsc, Arc};

#[test]
fn status_snapshot_keeps_coupled_heads_while_a_writer_waits() {
    let root = std::env::temp_dir().join(format!("buddy-snapshot-status-{}", Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let store = Arc::new(
        Store::open(StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("secrets"),
        })
        .unwrap(),
    );
    let original: Vec<_> = (0..2)
        .map(|_| Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::json!({"epoch":0}),
            media_descriptors: vec![],
        })
        .collect();
    store.commit(original.clone(), BTreeMap::new()).unwrap();
    let replacements: Vec<_> = original
        .iter()
        .map(|old| {
            let mut next = old.clone();
            next.parents = BTreeSet::from([old.revision_id]);
            next.revision_id = Uuid::new_v4();
            next.operation_id = Uuid::new_v4();
            next.payload = serde_json::json!({"epoch":1});
            next
        })
        .collect();
    let (start, started) = mpsc::channel();
    let (observed, blocked) = mpsc::channel();
    let writer_store = store.clone();
    let writer = std::thread::spawn(move || {
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        // Establish actual contention at the snapshot's paused predicate.
        assert!(matches!(
            writer_store.inner.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        observed.send(()).unwrap();
        writer_store.commit(replacements, BTreeMap::new()).unwrap();
    });
    let first = std::cell::Cell::new(true);
    let (status, snapshot) = store
        .snapshot_heads_matching_with_status(&[Namespace::Conversation], 2, |_| {
            if first.replace(false) {
                start.send(()).unwrap();
                blocked
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
            true
        })
        .unwrap();
    assert_eq!(status, 0);
    assert_eq!(
        snapshot
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>(),
        original.iter().map(|e| e.revision_id).collect()
    );
    assert!(snapshot.iter().all(|e| e.payload["epoch"] == 0));
    writer.join().unwrap();
    let (status, after) = store
        .snapshot_heads_matching_with_status(&[Namespace::Conversation], 2, |_| true)
        .unwrap();
    assert_eq!(status, 0);
    assert!(after.iter().all(|e| e.payload["epoch"] == 1));
    drop(store);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    assert!(root
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("buddy-snapshot-status-"));
    fs::remove_dir_all(root).unwrap();
}
