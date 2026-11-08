use super::*;
use crate::conversation::{target_source::tests::fixture, Ledger, Record};

#[test]
fn target_archive_exports_exact_closure_and_detached_origin_without_owner_files() {
    let (fixture, ledger, _, _) = fixture();
    let store = ledger.store().clone();
    let source_root = store.generation(store.inner.lock().unwrap().generation);
    drop(ledger);
    let destination = fixture.0.join("target-archive");
    let report = store.export_notebook_backup(&destination).unwrap();
    assert_eq!(report.format, 6);
    assert!(report.complete_media && report.objects > 0);
    assert_eq!(
        Store::inspect_notebook_backup(&destination).unwrap(),
        report
    );
    assert_eq!(command::Detached::inspect(&destination).unwrap().0, report);
    let archive = NotebookBackup::read(&destination).unwrap();
    assert!(archive.target_feature.is_some());
    for item in &archive.objects {
        assert_eq!(
            files::read(&source_root.join("objects").join(&item.sha256), MAX_MEDIA).unwrap(),
            files::read(&destination.join("objects").join(&item.sha256), MAX_MEDIA).unwrap()
        );
    }
    let names = fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from(["backup.json".into(), "objects".into()])
    );
    assert!(Store::inspect_backup(&destination).is_err());
}

#[test]
fn target_archive_requires_closed_format_and_exact_ordinary_origin() {
    let (fixture, ledger, _, _) = fixture();
    let store = ledger.store().clone();
    drop(ledger);
    let destination = fixture.0.join("target-archive");
    store.export_notebook_backup(&destination).unwrap();
    let marker = destination.join("backup.json");
    let original = files::read(&marker, MAX_METADATA as u64).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    for change in [
        "format4",
        "missing",
        "null",
        "actor",
        "generation",
        "selected",
        "contract",
    ] {
        let mut changed = value.clone();
        match change {
            "format4" => changed["format"] = serde_json::json!(4),
            "missing" => {
                changed.as_object_mut().unwrap().remove("target_feature");
            }
            "null" => changed["target_feature"] = serde_json::Value::Null,
            "actor" => {
                changed["target_feature"]["feature"]["actor"] = serde_json::json!(Uuid::new_v4())
            }
            "generation" => {
                changed["target_feature"]["feature"]["generation"] =
                    serde_json::json!(Uuid::new_v4())
            }
            "selected" => {
                changed["target_feature"]["feature"]["base_kind"] =
                    serde_json::json!("compound-selected");
                changed["target_feature"]["feature"]["source_family"] = serde_json::json!(5);
            }
            "contract" => {
                changed["target_feature"]["feature"]["contract_sha256"] =
                    serde_json::json!(digest(b"foreign contract"))
            }
            _ => unreachable!(),
        }
        files::atomic_json(&marker, &changed).unwrap();
        assert!(
            Store::inspect_notebook_backup(&destination).is_err(),
            "{change}"
        );
    }
    files::atomic(&marker, &original).unwrap();
    assert!(Store::inspect_notebook_backup(&destination).is_ok());
}

#[test]
fn legacy_archive_cannot_borrow_even_real_ready_target_owner_files() {
    let (_fixture, ledger, _, _) = fixture();
    let store = ledger.store().clone();
    drop(ledger);
    let generation = store.inner.lock().unwrap().generation;
    let root = store.generation(generation);
    let mut archive = NotebookBackup::build(
        &store.paths,
        &root,
        store.actor_id,
        generation,
        store
            .inner
            .lock()
            .unwrap()
            .index
            .manifests
            .values()
            .cloned()
            .collect(),
        None,
    )
    .unwrap();
    assert!(archive.inspect(&root).is_ok());
    archive.format = 4;
    archive.target_feature = None;
    let error = archive.inspect(&root).unwrap_err();
    assert!(error.to_string().contains("explicit archive format6"));
}

#[test]
fn target_archive_cannot_drop_referenced_target_from_otherwise_valid_inventory() {
    let (fixture, ledger, _, _) = fixture();
    let store = ledger.store().clone();
    drop(ledger);
    let destination = fixture.0.join("target-archive");
    store.export_notebook_backup(&destination).unwrap();
    let mut archive = NotebookBackup::read(&destination).unwrap();
    let mut removed = 0;
    for manifest in &mut archive.manifests {
        let mut descriptors = BTreeSet::new();
        let mut kept = Vec::new();
        for reference in &manifest.records {
            let envelope: Envelope = files::json(
                &destination.join("objects").join(&reference.sha256),
                MAX_RECORD as u64,
            )
            .unwrap();
            if matches!(Ledger::decode(&envelope), Ok(Record::SourceTarget(_))) {
                manifest.record_namespaces.remove(&reference.sha256);
                removed += 1;
            } else {
                descriptors.extend(
                    envelope
                        .media_descriptors
                        .into_iter()
                        .map(|media| media.sha256),
                );
                kept.push(reference.clone());
            }
        }
        manifest.records = kept;
        manifest
            .media
            .retain(|item| descriptors.contains(&item.sha256));
        manifest
            .media_coverage
            .retain(|item| descriptors.contains(item.hash()));
        manifest.validate().unwrap();
    }
    assert_eq!(removed, 1);
    let required = archive
        .manifests
        .iter()
        .flat_map(|manifest| manifest.records.iter().chain(&manifest.media))
        .map(|item| item.sha256.clone())
        .collect::<BTreeSet<_>>();
    archive
        .objects
        .retain(|item| required.contains(&item.sha256));
    let error = archive.inspect(&destination).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("target committed revision absent"),
        "{error:#}"
    );
}
