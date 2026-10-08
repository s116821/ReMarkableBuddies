use super::*;

fn archive_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path.join("backup.json")).unwrap()).unwrap()
}
fn write_archive(path: &Path, value: &serde_json::Value) {
    files::atomic_json(&path.join("backup.json"), value).unwrap();
}
fn export_fixture() -> (
    Fixture,
    Store,
    SelectionScope,
    SelectionPublication,
    PathBuf,
) {
    let fixture = Fixture::new();
    let store = fixture.store();
    let scope = scope();
    let (_, publication, _, _) = retained_fixture(&store, &scope);
    let path = fixture.0.join("archive");
    store.export_selected(&path).unwrap();
    (fixture, store, scope, publication, path)
}

#[test]
fn portable_archive_preserves_history_retained_media_raw_references_and_other_scopes() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let (original, replacement, intent, intent_reference) = retained_fixture(&store, &s);
    let mut settlement = edit(&intent);
    settlement.payload = json!({"historical_settlement":true});
    let (settled, objects) = closure(&[intent.clone(), settlement.clone()], BTreeMap::new());
    let publication = store
        .commit_retained(
            &replacement.token,
            RetainedCommit {
                operation: Uuid::new_v4(),
                original_operation: original.transaction.operation,
                intent: intent_reference.clone(),
                retained: vec![settled],
            },
            objects,
        )
        .unwrap();
    let second = scope();
    let first = record(&store);
    let last = edit(&first);
    let (mut change, _) = change(&[last.clone(), first.clone()]);
    let mut raw = BTreeMap::new();
    let mut refs = Vec::new();
    change.selected.record_namespaces.clear();
    for envelope in [last, first] {
        let bytes = serde_json::to_vec_pretty(&envelope).unwrap();
        let hash = digest(&bytes);
        refs.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        change
            .selected
            .record_namespaces
            .insert(hash.clone(), envelope.namespace);
        raw.insert(hash, bytes);
    }
    change.selected.records = refs.clone();
    let second_publication = store
        .initialize_selected(&second, change, raw.clone())
        .unwrap();
    let unrelated = record(&store);
    store
        .commit(vec![unrelated.clone()], BTreeMap::new())
        .unwrap();
    let mut tombstone = edit(&unrelated);
    tombstone.kind = Kind::Tombstone;
    tombstone.payload = serde_json::Value::Null;
    store.commit(vec![tombstone], BTreeMap::new()).unwrap();
    let root = store.generation(publication.token.store_generation());
    // Noncanonical selection metadata must also retain its ORIGINAL hash.
    let head_path = root.join("selections").join(s.filename().unwrap());
    let head_bytes = serde_json::to_vec_pretty(&publication.transaction).unwrap();
    files::atomic(&head_path, &head_bytes).unwrap();
    fs::write(
        root.join("selection-history").join(digest(b"orphan")),
        b"orphan",
    )
    .unwrap();
    fs::write(root.join("native-document.fixture"), b"not Buddy-owned").unwrap();
    fs::write(
        store.paths.data.join("config.snapshot.json"),
        b"not an export allowlist",
    )
    .unwrap();
    files::directory(&store.paths.credentials).unwrap();
    fs::write(
        store.paths.credentials.join("fixture-key"),
        b"excluded fixture",
    )
    .unwrap();
    let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
    let before = selected(&store, &s).token;
    let archive = f.0.join("archive");
    let report = store.export_selected(&archive).unwrap();
    assert_eq!(report, Store::inspect_backup(&archive).unwrap());
    assert_eq!(report.format, 2);
    assert_eq!(report.selected_scopes, 2);
    assert_eq!(report.selection_receipts, 5);
    assert!(report.complete_media);
    assert!(!report.configuration_included);
    assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
    assert_eq!(selected(&store, &s).token, before);
    for (hash, bytes) in raw {
        assert_eq!(fs::read(archive.join("objects").join(hash)).unwrap(), bytes);
    }
    assert_eq!(
        fs::read(archive.join("objects").join(digest(&head_bytes))).unwrap(),
        head_bytes
    );
    let mut transaction = publication.transaction.clone();
    while let Some(hash) = transaction.previous_sha256 {
        let original_bytes = fs::read(root.join("selection-history").join(&hash)).unwrap();
        assert_eq!(
            fs::read(archive.join("objects").join(hash)).unwrap(),
            original_bytes
        );
        transaction = serde_json::from_slice(&original_bytes).unwrap();
    }
    let metadata = archive_json(&archive);
    assert_eq!(
        metadata["selected_history"]["origin_actor"],
        store.actor_id.to_string()
    );
    assert_eq!(
        metadata["selected_history"]["origin_generation"],
        publication.token.store_generation().to_string()
    );
    let media = &intent.media_descriptors[0];
    assert_eq!(
        fs::read(archive.join("objects").join(&media.sha256)).unwrap(),
        b"owned admitted evidence"
    );
    assert!(!archive.join("objects").join(digest(b"orphan")).exists());
    assert_eq!(fs::read_dir(&archive).unwrap().count(), 2);
    let target = Fixture::new();
    let target_store = target.store();
    let target_current = fs::read(target_store.paths.data.join("CURRENT")).unwrap();
    assert!(target_store.restore(&archive).is_err());
    assert_eq!(
        fs::read(target_store.paths.data.join("CURRENT")).unwrap(),
        target_current
    );
    drop(store);
    let reopened = f.store();
    assert_eq!(selected(&reopened, &s).token, before);
    assert_eq!(
        selected(&reopened, &second).transaction.selected.records,
        refs
    );
    assert_eq!(selected(&reopened, &second).token, second_publication.token);
    assert!(reopened
        .value(unrelated.namespace, unrelated.record_id)
        .unwrap()
        .is_none());
}

#[test]
fn portable_archive_faults_publish_marker_last_and_never_mutate_source() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::AfterCommit,
        Fault::None,
    ] {
        let f = Fixture::new();
        let store = f.store();
        let s = scope();
        let (_, publication, _, _) = retained_fixture(&store, &s);
        let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
        store.set_fault(fault).unwrap();
        let archive = f.0.join("archive");
        assert_eq!(
            store.export_selected(&archive).is_ok(),
            fault == Fault::None
        );
        let committed = matches!(fault, Fault::AfterCommit | Fault::None);
        assert_eq!(archive.join("backup.json").exists(), committed);
        assert_eq!(Store::inspect_backup(&archive).is_ok(), committed);
        assert_eq!(selected(&store, &s).token, publication.token);
        assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
    }
}

#[test]
fn portable_archive_refuses_inventory_chain_origin_and_format_tampering() {
    for case in 0..10 {
        let (_f, store, s, publication, archive) = export_fixture();
        let mut metadata = archive_json(&archive);
        match case {
            0 => metadata["format"] = json!(99),
            1 => metadata["surprise"] = json!(true),
            2 => metadata["selected_history"]["origin_generation"] = json!(Uuid::new_v4()),
            3 => {
                let duplicate = metadata["selected_history"]["heads"][0].clone();
                metadata["selected_history"]["heads"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            4 => {
                let hash = publication.transaction.previous_sha256.as_ref().unwrap();
                fs::remove_file(archive.join("objects").join(hash)).unwrap();
            }
            5 => {
                metadata["objects"].as_array_mut().unwrap().pop();
            }
            6 => {
                metadata["manifests"].as_array_mut().unwrap().clear();
            }
            7 => metadata["complete_media"] = json!(false),
            8 => metadata["config_snapshot"] = json!({}),
            9 => {
                let object = metadata["objects"][0].clone();
                metadata["objects"].as_array_mut().unwrap().push(object);
            }
            _ => unreachable!(),
        }
        write_archive(&archive, &metadata);
        assert!(Store::inspect_backup(&archive).is_err(), "case {case}");
        assert_eq!(selected(&store, &s).token, publication.token);
    }
}

#[test]
fn portable_archive_refuses_supported_hashes_with_unknown_feature_or_invalid_head() {
    for feature in [true, false] {
        let (_f, _, _, _, archive) = export_fixture();
        let mut metadata = archive_json(&archive);
        let reference = if feature {
            metadata["selected_history"]["feature"].clone()
        } else {
            metadata["selected_history"]["heads"][0].clone()
        };
        let old_hash = reference["sha256"].as_str().unwrap();
        let old_path = archive.join("objects").join(old_hash);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&old_path).unwrap()).unwrap();
        if feature {
            value["format"] = json!(99);
        } else {
            value["history_depth"] = json!(1);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let new_hash = digest(&bytes);
        let new_ref = json!({"sha256":new_hash,"bytes":bytes.len()});
        for object in metadata["objects"].as_array_mut().unwrap() {
            if object["sha256"] == old_hash {
                *object = new_ref.clone();
            }
        }
        if feature {
            metadata["selected_history"]["feature"] = new_ref;
        } else {
            metadata["selected_history"]["heads"][0] = new_ref;
        }
        fs::remove_file(old_path).unwrap();
        fs::write(archive.join("objects").join(new_hash), bytes).unwrap();
        write_archive(&archive, &metadata);
        assert!(Store::inspect_backup(&archive).is_err());
    }
}

#[test]
fn portable_archive_partial_media_is_explicit_and_promised_missing_media_refuses() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let mut omitted = record(&store);
    let hash = digest(b"deliberately omitted media");
    omitted.media_descriptors.push(Media {
        sha256: hash.clone(),
        bytes: 26,
        media_type: "application/octet-stream".into(),
    });
    let (mut change, objects) = change(&[omitted]);
    change.selected.media.clear();
    change.selected.media_coverage = vec![Coverage::Omitted {
        sha256: hash,
        reason: Omission::PolicyDisabled,
    }];
    store.initialize_selected(&s, change, objects).unwrap();
    let archive = f.0.join("partial");
    let report = store.export_selected(&archive).unwrap();
    assert!(!report.complete_media);
    assert_eq!(Store::inspect_backup(&archive).unwrap(), report);
    let (complete_f, complete_store, s, _, archive) = export_fixture();
    let media = selected(&complete_store, &s).retained_records[0].media_descriptors[0].clone();
    fs::remove_file(archive.join("objects").join(&media.sha256)).unwrap();
    assert!(Store::inspect_backup(&archive).is_err());
    let generation = complete_store.inner.lock().unwrap().generation;
    fs::remove_file(
        complete_store
            .generation(generation)
            .join("objects")
            .join(media.sha256),
    )
    .unwrap();
    let refused = complete_f.0.join("missing-source-media");
    assert!(complete_store.export_selected(&refused).is_err());
    assert!(!refused.join("backup.json").exists());
}

#[test]
fn portable_inspection_accepts_legacy_shape_and_selected_export_requires_selected_source() {
    let f = Fixture::new();
    let store = f.store();
    store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
    let archive = f.0.join("legacy");
    store.export(&archive).unwrap();
    assert!(archive_json(&archive).get("selected_history").is_none());
    let report = Store::inspect_backup(&archive).unwrap();
    assert_eq!(report.format, 1);
    assert_eq!(report.selected_scopes, 0);
    assert!(store.export_selected(&f.0.join("refused")).is_err());
    assert!(!f.0.join("refused").exists());
    let target = Fixture::new();
    target.store().restore(&archive).unwrap();
    let mut metadata = archive_json(&archive);
    metadata["selected_history"] = serde_json::Value::Null;
    write_archive(&archive, &metadata);
    assert!(Store::inspect_backup(&archive).is_err());
    assert!(target.store().restore(&archive).is_err());
}

#[test]
fn portable_export_refuses_owned_paths_existing_destinations_and_symlink_destinations() {
    let (f, store, s, publication, _) = export_fixture();
    let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
    for path in [
        &store.paths.data,
        &store.paths.cache,
        &store.paths.credentials,
    ] {
        assert!(store.export_selected(&path.join("nested")).is_err());
    }
    assert!(store.export_selected(&f.0).is_err());
    let existing = f.0.join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("keep"), b"keep").unwrap();
    assert!(store.export_selected(&existing).is_err());
    assert_eq!(fs::read(existing.join("keep")).unwrap(), b"keep");
    #[cfg(unix)]
    {
        let link = f.0.join("linked");
        std::os::unix::fs::symlink(&existing, &link).unwrap();
        assert!(store.export_selected(&link.join("archive")).is_err());
        assert!(!existing.join("archive").exists());
    }
    assert_eq!(selected(&store, &s).token, publication.token);
    assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
}

#[test]
fn portable_export_capacity_refuses_without_claiming_complete_or_dropping_records() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let publication = initialize(&store, &s, &[record(&store)]);
    // Each committed record fits its per-object bound; their union exceeds the
    // inspector's explicit 8 MiB metadata capacity.
    for _ in 0..9 {
        let mut large = record(&store);
        large.payload = json!({"large_fixture":"x".repeat(950_000)});
        store.commit(vec![large], BTreeMap::new()).unwrap();
    }
    let current = fs::read(store.paths.data.join("CURRENT")).unwrap();
    let archive = f.0.join("over-capacity");
    assert!(store.export_selected(&archive).is_err());
    assert!(!archive.join("backup.json").exists());
    assert_eq!(selected(&store, &s).token, publication.token);
    assert_eq!(fs::read(store.paths.data.join("CURRENT")).unwrap(), current);
    assert_eq!(store.manifests().unwrap().len(), 10);
}
