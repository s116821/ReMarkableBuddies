use super::*;
fn request(store: &Store) -> SelectedAttemptRequest {
    let bytes = b"opaque staged descriptor, not domain authority";
    let descriptor = ObjectRef {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
    };
    store.stage_blob(&descriptor, &bytes[..]).unwrap();
    SelectedAttemptRequest {
        operation: Uuid::new_v4(),
        slot_id: "fixed-slot".into(),
        descriptor_id: "fixed-descriptor".into(),
        descriptor,
    }
}
#[test]
fn selected_outbox_restart_replay_withdraw_and_unrelated_scope_are_preserved() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let original = initialize(&store, &s, &[record(&store)]);
    let other = scope();
    let unrelated = initialize(&store, &other, &[record(&store)]);
    let request = request(&store);
    let prepared = store
        .prepare_selected_attempt(&original.token, request.clone())
        .unwrap();
    assert_eq!(prepared.state, SelectedAttemptState::Pending);
    assert_eq!(
        store
            .current_selected_attempt(&original.token, request.operation)
            .unwrap()
            .request,
        request
    );
    let fingerprint = prepared.request_sha256;
    drop(store);
    let store = f.store();
    assert_eq!(
        store
            .inspect_selected_attempt(request.operation)
            .unwrap()
            .unwrap()
            .request_sha256,
        fingerprint
    );
    assert_eq!(
        store
            .prepare_selected_attempt(&original.token, request.clone())
            .unwrap()
            .request_sha256,
        fingerprint
    );
    let (change, objects) = change(&[record(&store)]);
    let replacement = store
        .activate_selected(&original.token, change, objects)
        .unwrap();
    assert!(store
        .current_selected_attempt(&original.token, request.operation)
        .is_err());
    assert!(store
        .current_selected_attempt(&replacement.token, request.operation)
        .is_err());
    assert!(store
        .current_selected_attempt(&unrelated.token, request.operation)
        .is_err());
    assert_eq!(
        store
            .prepare_selected_attempt(&original.token, request.clone())
            .unwrap()
            .request_sha256,
        fingerprint
    );
    let withdrawn = store.withdraw_selected_attempt(request.operation).unwrap();
    assert_eq!(withdrawn.state, SelectedAttemptState::Withdrawn);
    assert_eq!(withdrawn.request_sha256, fingerprint);
    assert_eq!(
        store
            .prepare_selected_attempt(&original.token, request.clone())
            .unwrap()
            .state,
        SelectedAttemptState::Withdrawn
    );
    assert!(store
        .current_selected_attempt(&original.token, request.operation)
        .is_err());
    let mut collision = request.clone();
    collision.slot_id = "another-slot".into();
    assert!(store
        .prepare_selected_attempt(&original.token, collision)
        .is_err());
    assert_eq!(selected(&store, &other).token, unrelated.token);
    assert_eq!(selected(&store, &s).token, replacement.token);
    let archive = f.0.join("archive");
    store.export_selected(&archive).unwrap();
    assert!(!archive.join("sync").exists());
    assert!(!archive
        .join("objects")
        .join(&request.descriptor.sha256)
        .exists());
}
#[test]
fn selected_outbox_faults_recover_absent_or_complete_original_intent() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let publication = initialize(&store, &s, &[record(&store)]);
        let request = request(&store);
        let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
        store.set_fault(fault).unwrap();
        assert!(store
            .prepare_selected_attempt(&publication.token, request.clone())
            .is_err());
        store.set_fault(Fault::None).unwrap();
        let observed = store.inspect_selected_attempt(request.operation).unwrap();
        assert_eq!(observed.is_some(), fault == Fault::AfterCommit);
        drop(store);
        let store = f.store();
        let replay = store
            .prepare_selected_attempt(&publication.token, request.clone())
            .unwrap();
        assert_eq!(replay.request, request);
        assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
        store.set_fault(fault).unwrap();
        assert!(store.withdraw_selected_attempt(request.operation).is_err());
        store.set_fault(Fault::None).unwrap();
        assert_eq!(
            store
                .inspect_selected_attempt(request.operation)
                .unwrap()
                .unwrap()
                .state,
            if fault == Fault::AfterCommit {
                SelectedAttemptState::Withdrawn
            } else {
                SelectedAttemptState::Pending
            }
        );
        store.withdraw_selected_attempt(request.operation).unwrap();
        assert_eq!(selected(&store, &s).token, publication.token);
    }
}
#[test]
fn selected_outbox_refuses_stale_missing_media_descriptor_and_foreign_journal() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let (_, publication, _, _) = retained_fixture(&store, &s);
    let request = request(&store);
    let root = store.generation(publication.token.store_generation());
    let descriptor_path = root.join("objects").join(&request.descriptor.sha256);
    let descriptor_bytes = fs::read(&descriptor_path).unwrap();
    fs::remove_file(&descriptor_path).unwrap();
    assert!(store
        .prepare_selected_attempt(&publication.token, request.clone())
        .is_err());
    assert!(store
        .inspect_selected_attempt(request.operation)
        .unwrap()
        .is_none());
    fs::write(&descriptor_path, &descriptor_bytes).unwrap();
    store
        .prepare_selected_attempt(&publication.token, request.clone())
        .unwrap();
    let media_ref = publication.transaction.retained[0].media[0].clone();
    let media_path = root.join("objects").join(&media_ref.sha256);
    let media_bytes = fs::read(&media_path).unwrap();
    fs::remove_file(&media_path).unwrap();
    assert!(store
        .current_selected_attempt(&publication.token, request.operation)
        .is_err());
    assert!(store
        .prepare_selected_attempt(&publication.token, request.clone())
        .is_err());
    fs::write(media_path, media_bytes).unwrap();
    assert!(store
        .current_selected_attempt(&publication.token, request.operation)
        .is_ok());
    let journal = store
        .paths
        .data
        .join("sync/selected-outbox")
        .join(format!("{}.json", request.operation));
    let original = fs::read(&journal).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["actor"] = json!(Uuid::new_v4());
    fs::write(&journal, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(store.inspect_selected_attempt(request.operation).is_err());
    fs::write(journal, original).unwrap();
    let (change, objects) = change(&[record(&store)]);
    store
        .activate_selected(&publication.token, change, objects)
        .unwrap();
    let fresh = SelectedAttemptRequest {
        operation: Uuid::new_v4(),
        ..request
    };
    assert!(store
        .prepare_selected_attempt(&publication.token, fresh)
        .is_err());
}

#[test]
fn selected_outbox_bounds_use_actual_journal_bytes_and_wake_only_new_preparations() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let publication = initialize(&store, &s, &[record(&store)]);
    let request = request(&store);
    let (wake, receive) = std::sync::mpsc::sync_channel(4);
    store.set_wake(Some(wake));
    store
        .prepare_selected_attempt(&publication.token, request.clone())
        .unwrap();
    assert!(receive.try_recv().is_ok());
    store
        .prepare_selected_attempt(&publication.token, request.clone())
        .unwrap();
    assert!(receive.try_recv().is_err());
    for case in 0..3 {
        let mut invalid = request.clone();
        invalid.operation = Uuid::new_v4();
        match case {
            0 => invalid.descriptor.bytes = 64 * 1024 + 1,
            1 => invalid.descriptor_id = invalid.slot_id.clone(),
            _ => invalid.slot_id = "invalid/provider/path".into(),
        }
        assert!(store
            .prepare_selected_attempt(&publication.token, invalid)
            .is_err());
    }
    let journal = store
        .paths
        .data
        .join("sync/selected-outbox")
        .join(format!("{}.json", request.operation));
    let original = fs::read(&journal).unwrap();
    let mut padded = original.clone();
    padded.resize(MAX_METADATA, b' ');
    fs::write(&journal, padded).unwrap();
    let next = SelectedAttemptRequest {
        operation: Uuid::new_v4(),
        slot_id: "another-fixed-slot".into(),
        ..request.clone()
    };
    assert!(store
        .prepare_selected_attempt(&publication.token, next.clone())
        .is_err());
    assert!(store
        .inspect_selected_attempt(next.operation)
        .unwrap()
        .is_none());
    assert!(receive.try_recv().is_err());
    fs::write(journal, original).unwrap();
    store
        .prepare_selected_attempt(&publication.token, next)
        .unwrap();
    assert!(receive.try_recv().is_ok());
    assert_eq!(selected(&store, &s).token, publication.token);
}

#[test]
fn selected_outbox_interrupted_staging_reopen_preserves_pending_and_withdrawn() {
    for state in [
        None,
        Some(SelectedAttemptState::Pending),
        Some(SelectedAttemptState::Withdrawn),
    ] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let publication = initialize(&store, &s, &[record(&store)]);
        let request = request(&store);
        if state.is_some() {
            store
                .prepare_selected_attempt(&publication.token, request.clone())
                .unwrap();
        }
        if state == Some(SelectedAttemptState::Withdrawn) {
            store.withdraw_selected_attempt(request.operation).unwrap();
        }
        let directory = store.paths.data.join("sync/selected-outbox");
        files::directory(&directory).unwrap();
        let journal = directory.join(format!("{}.json", request.operation));
        let original = journal.exists().then(|| fs::read(&journal).unwrap());
        let staging = directory.join(format!(".stage-{}", Uuid::new_v4()));
        fs::write(&staging, b"partial preparation or withdrawal JSON").unwrap();
        let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
        drop(store);
        let store = f.store();
        assert!(
            !staging.exists(),
            "uncommitted outbox staging survives reopen"
        );
        let inspected = store.inspect_selected_attempt(request.operation).unwrap();
        assert_eq!(inspected.as_ref().map(|a| a.state), state);
        if let Some(original) = original {
            assert_eq!(fs::read(&journal).unwrap(), original);
        }
        let replay = store
            .prepare_selected_attempt(&publication.token, request.clone())
            .unwrap();
        assert_eq!(replay.state, state.unwrap_or(SelectedAttemptState::Pending));
        assert_eq!(replay.request, request);
        assert_eq!(selected(&store, &s).token, publication.token);
        assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
        // Unsupported foreign files survive cleanup and still refuse inventory.
        let foreign = directory.join(".stage-not-a-uuid");
        fs::write(&foreign, b"foreign unsupported metadata").unwrap();
        drop(store);
        let store = f.store();
        assert!(foreign.exists());
        assert!(store.inspect_selected_attempt(request.operation).is_err());
        fs::remove_file(foreign).unwrap();
        assert_eq!(
            store
                .inspect_selected_attempt(request.operation)
                .unwrap()
                .unwrap()
                .state,
            replay.state
        );
    }
}
#[test]
fn selected_outbox_staging_cleanup_refuses_nonregular_entries_without_deletion() {
    let f = Fixture::new();
    let store = f.store();
    let directory = store.paths.data.join("sync/selected-outbox");
    files::directory(&directory).unwrap();
    let unsupported = directory.join(format!(".stage-{}", Uuid::new_v4()));
    fs::create_dir(&unsupported).unwrap();
    let sentinel = unsupported.join("preserved");
    fs::write(&sentinel, b"keep").unwrap();
    drop(store);
    assert!(Store::open(f.paths()).is_err());
    assert!(sentinel.exists());
    fs::remove_dir_all(&unsupported).unwrap();
    #[cfg(unix)]
    {
        let target = f.0.join("preserved-target");
        fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, &unsupported).unwrap();
        assert!(Store::open(f.paths()).is_err());
        assert!(fs::symlink_metadata(&unsupported)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }
}

#[test]
fn selected_outbox_mode_epochs_refuse_unbound_and_old_shared_intents() {
    use crate::storage::sync::mode::{SyncMode, SyncModeChoice};
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let publication = initialize(&store, &s, &[record(&store)]);
    let request = request(&store);
    let unbound = store
        .prepare_selected_attempt(&publication.token, request.clone())
        .unwrap();
    assert!(unbound.mode_epoch.is_none());
    let shared = SyncModeChoice::SharedGroup {
        group: s.group,
        binding_sha256: s.binding_sha256.clone(),
        root_id: "chosen-root".into(),
    };
    let mode = store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            shared.clone(),
        )
        .unwrap();
    assert!(matches!(mode.mode, SyncMode::SharedGroup { .. }));
    assert!(store
        .current_selected_attempt(&publication.token, request.operation)
        .is_err());
    assert!(store
        .prepare_selected_attempt(&publication.token, request.clone())
        .unwrap()
        .mode_epoch
        .is_none());
    let bound_request = SelectedAttemptRequest {
        operation: Uuid::new_v4(),
        ..request.clone()
    };
    let bound = store
        .prepare_selected_attempt(&publication.token, bound_request.clone())
        .unwrap();
    assert_eq!(bound.mode_epoch, Some(mode.epoch));
    assert!(store
        .current_selected_attempt(&publication.token, bound_request.operation)
        .is_ok());
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::LocalOnly,
        )
        .unwrap();
    assert!(store
        .current_selected_attempt(&publication.token, bound_request.operation)
        .is_err());
    let fresh = SelectedAttemptRequest {
        operation: Uuid::new_v4(),
        ..request.clone()
    };
    assert!(store
        .prepare_selected_attempt(&publication.token, fresh)
        .is_err());
    let returned = store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            shared,
        )
        .unwrap();
    assert_ne!(returned.epoch, mode.epoch);
    assert!(store
        .current_selected_attempt(&publication.token, bound_request.operation)
        .is_err());
    assert_eq!(
        store
            .inspect_selected_attempt(bound_request.operation)
            .unwrap()
            .unwrap()
            .mode_epoch,
        bound.mode_epoch
    );
    assert_eq!(selected(&store, &s).token, publication.token);
    let archive = f.0.join("archive");
    store.export_selected(&archive).unwrap();
    assert!(!archive.join("sync/mode.json").exists());
}

#[test]
fn selected_outbox_root_only_mode_replacement_keeps_original_custody() {
    use crate::storage::sync::mode::SyncModeChoice;
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let selected = initialize(&store, &s, &[record(&store)]);
    let choose = |root: &str| SyncModeChoice::SharedGroup {
        group: s.group,
        binding_sha256: s.binding_sha256.clone(),
        root_id: root.into(),
    };
    let original = store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            choose("first-root"),
        )
        .unwrap();
    let request = request(&store);
    let attempt = store
        .prepare_selected_attempt(&selected.token, request.clone())
        .unwrap();
    assert_eq!(attempt.mode_epoch, Some(original.epoch));
    let journal = serde_json::to_vec(&attempt).unwrap();
    assert!(store
        .current_selected_attempt(&selected.token, request.operation)
        .is_ok());
    let replaced = store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            choose("replacement-root"),
        )
        .unwrap();
    assert_ne!(original.epoch, replaced.epoch);
    drop(store);
    let store = f.store();
    assert!(store
        .current_selected_attempt(&selected.token, request.operation)
        .is_err());
    assert_eq!(
        serde_json::to_vec(
            &store
                .inspect_selected_attempt(request.operation)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        journal
    );
    assert_eq!(
        serde_json::to_vec(
            &store
                .prepare_selected_attempt(&selected.token, request.clone())
                .unwrap()
        )
        .unwrap(),
        journal
    );
    let next = SelectedAttemptRequest {
        operation: Uuid::new_v4(),
        ..request
    };
    assert_eq!(
        store
            .prepare_selected_attempt(&selected.token, next.clone())
            .unwrap()
            .mode_epoch,
        Some(replaced.epoch)
    );
    assert!(store
        .current_selected_attempt(&selected.token, next.operation)
        .is_ok());
    assert_eq!(
        store.selected_snapshot(&s, 4096).unwrap().unwrap().token,
        selected.token
    );
}

#[test]
fn selected_outbox_inventory_recovers_original_stale_and_withdrawn_custody() {
    let f = Fixture::new();
    let store = f.store();
    assert!(store.selected_attempt_inventory().unwrap().is_empty());
    let first = initialize(&store, &scope(), &[record(&store)]);
    let other = initialize(&store, &scope(), &[record(&store)]);
    let a = request(&store);
    let b = request(&store);
    store
        .prepare_selected_attempt(&other.token, b.clone())
        .unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            crate::storage::sync::mode::SyncModeChoice::SharedGroup {
                group: first.token.scope().group,
                binding_sha256: first.token.scope().binding_sha256.clone(),
                root_id: "chosen-root".into(),
            },
        )
        .unwrap();
    let prepared = store
        .prepare_selected_attempt(&first.token, a.clone())
        .unwrap();
    assert!(prepared.mode_epoch.is_some());
    let withdrawn = store.withdraw_selected_attempt(b.operation).unwrap();
    let (change, objects) = change(&[record(&store)]);
    let replacement = store
        .activate_selected(&first.token, change, objects)
        .unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            crate::storage::sync::mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    let directory = store.paths.data.join("sync/selected-outbox");
    let before_a = fs::read(directory.join(format!("{}.json", a.operation))).unwrap();
    let before_b = fs::read(directory.join(format!("{}.json", b.operation))).unwrap();
    drop(store);
    let store = f.store();
    let entries = store.selected_attempt_inventory().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries[0].request.operation < entries[1].request.operation);
    let recovered = entries
        .iter()
        .find(|e| e.request.operation == a.operation)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(recovered).unwrap(),
        serde_json::to_vec(&prepared).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(
            entries
                .iter()
                .find(|e| e.request.operation == b.operation)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_vec(&withdrawn).unwrap()
    );
    assert!(store
        .current_selected_attempt(&first.token, a.operation)
        .is_err());
    assert!(store
        .current_selected_attempt(&other.token, b.operation)
        .is_err());
    assert_eq!(
        selected(&store, first.token.scope()).token,
        replacement.token
    );
    assert_eq!(
        fs::read(directory.join(format!("{}.json", a.operation))).unwrap(),
        before_a
    );
    assert_eq!(
        fs::read(directory.join(format!("{}.json", b.operation))).unwrap(),
        before_b
    );
}

#[test]
fn selected_outbox_inventory_refuses_whole_corrupt_journal_without_mutation() {
    let f = Fixture::new();
    let store = f.store();
    let current = initialize(&store, &scope(), &[record(&store)]);
    let request = request(&store);
    store
        .prepare_selected_attempt(&current.token, request.clone())
        .unwrap();
    let directory = store.paths.data.join("sync/selected-outbox");
    let path = directory.join(format!("{}.json", request.operation));
    let original = fs::read(&path).unwrap();
    for corrupt in [b"{truncated".to_vec(), {
        let mut entry: serde_json::Value = serde_json::from_slice(&original).unwrap();
        entry["actor"] = serde_json::json!(Uuid::new_v4());
        serde_json::to_vec(&entry).unwrap()
    }] {
        fs::write(&path, &corrupt).unwrap();
        assert!(store.selected_attempt_inventory().is_err());
        assert_eq!(fs::read(&path).unwrap(), corrupt);
        fs::write(&path, &original).unwrap();
        assert_eq!(store.selected_attempt_inventory().unwrap().len(), 1);
    }
    let foreign = directory.join("foreign.json");
    fs::write(&foreign, &original).unwrap();
    assert!(store.selected_attempt_inventory().is_err());
    assert_eq!(fs::read(&foreign).unwrap(), original);
    fs::remove_file(&foreign).unwrap();
    assert_eq!(store.selected_attempt_inventory().unwrap().len(), 1);
    assert_eq!(selected(&store, current.token.scope()).token, current.token);
    assert_eq!(fs::read(&path).unwrap(), original);
}
