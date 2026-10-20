use remarkable_reader_buddy::storage::{inspection::*, selection::*, *};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-inspection-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn open(&self) -> Store {
        Store::open(self.paths()).unwrap()
    }
    fn generation(&self) -> PathBuf {
        let current: Uuid = files::json(&self.paths().data.join("CURRENT"), 128).unwrap();
        self.paths()
            .data
            .join("generations")
            .join(current.to_string())
    }
    fn inspect(&self) -> OwnedDataReport {
        let before = inventory(&self.0);
        let result = inspect_owned_data(&self.paths());
        assert_eq!(
            inventory(&self.0),
            before,
            "inspection changed contents, paths or modification times"
        );
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, (u64, String, Option<std::time::SystemTime>)> {
    fn visit(
        root: &Path,
        path: &Path,
        items: &mut BTreeMap<PathBuf, (u64, String, Option<std::time::SystemTime>)>,
    ) {
        if !path.exists() {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, items);
            } else if metadata.is_file() {
                // No writer lease is held in content-preservation checks.
                items.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    (
                        metadata.len(),
                        digest(&fs::read(&path).unwrap()),
                        metadata.modified().ok(),
                    ),
                );
            }
        }
    }
    let mut items = BTreeMap::new();
    visit(root, root, &mut items);
    items
}
fn record(store: &Store, namespace: Namespace, media: Vec<Media>) -> Envelope {
    Envelope {
        envelope_version: FORMAT,
        namespace,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"private-text":"QUESTION_SENTINEL_NOT_FOR_REPORT"}),
        media_descriptors: media,
    }
}
fn media(bytes: &[u8]) -> Media {
    Media {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    }
}
fn manifest(
    records: &[Envelope],
    included: &[(&Media, Vec<u8>)],
    omitted: &[&Media],
) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut objects = BTreeMap::new();
    let mut references = vec![];
    let mut namespaces = BTreeMap::new();
    for record in records {
        let bytes = serde_json::to_vec(record).unwrap();
        let hash = digest(&bytes);
        references.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(hash.clone(), record.namespace);
        objects.insert(hash, bytes);
    }
    let mut refs = vec![];
    let mut coverage = vec![];
    for (descriptor, bytes) in included {
        refs.push(ObjectRef {
            sha256: descriptor.sha256.clone(),
            bytes: descriptor.bytes,
        });
        coverage.push(Coverage::Included {
            sha256: descriptor.sha256.clone(),
        });
        objects.insert(descriptor.sha256.clone(), bytes.clone());
    }
    coverage.extend(omitted.iter().map(|descriptor| Coverage::Omitted {
        sha256: descriptor.sha256.clone(),
        reason: Omission::PolicyDisabled,
    }));
    (
        Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: references,
            record_namespaces: namespaces,
            media: refs,
            media_coverage: coverage,
        },
        objects,
    )
}
#[test]
fn nonexistent_roots_and_missing_lock_remain_unavailable_without_creation() {
    let fixture = Fixture::new();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Unavailable);
    assert!(report.complete_apparent_bytes.is_none());
    assert!(!fixture.0.exists());
    assert!(report.observed_domains.is_empty());
    let store = fixture.open();
    drop(store);
    fs::remove_file(fixture.paths().data.join("store.lock")).unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Unavailable);
    assert!(report.issues.contains_key(&InspectionIssue::MissingLock));
    assert!(!fixture.paths().data.join("store.lock").exists());
}
#[test]
fn active_real_store_writer_is_busy_and_never_stopped_or_recovered() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    let report = inspect_owned_data(&fixture.paths());
    assert_eq!(report.status, InspectionStatus::Busy);
    assert!(report.complete_apparent_bytes.is_none());
    assert_eq!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        current
    );
    assert_eq!(store.unavailable_commits().unwrap(), 0);
    drop(store);
    assert_eq!(fixture.inspect().status, InspectionStatus::Complete);
}
#[test]
fn real_shared_media_namespace_references_are_nonadditive_and_file_charge_is_once() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"fixture-image-metadata-only".to_vec();
    let image = media(&bytes);
    let source = record(&store, Namespace::Source, vec![image.clone()]);
    let memory = record(&store, Namespace::SubjectMemory, vec![image.clone()]);
    store
        .commit(
            vec![source, memory],
            BTreeMap::from([(image.sha256.clone(), bytes)]),
        )
        .unwrap();
    drop(store);
    fs::write(
        fixture.paths().credentials.join("model.key"),
        "CREDENTIAL_SENTINEL_NOT_FOR_REPORT",
    )
    .unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Complete);
    assert_eq!(report.shared_media_files, 1);
    assert_eq!(report.shared_media_apparent_bytes, image.bytes);
    assert_eq!(
        report.storage[&StorageKind::MediaObject].apparent_bytes,
        image.bytes
    );
    assert_eq!(
        report.observed_domains[&Namespace::Source].declared_media_bytes,
        image.bytes
    );
    assert_eq!(
        report.observed_domains[&Namespace::SubjectMemory].declared_media_bytes,
        image.bytes
    );
    assert!(!report.reference_totals_additive);
    assert!(!report.credentials_inspected);
    assert!(!report.native_documents_inspected);
    assert_eq!(
        report
            .storage
            .values()
            .map(|item| item.apparent_bytes)
            .sum::<u64>(),
        report.observed_apparent_bytes
    );
    let encoded = serde_json::to_string(&report).unwrap();
    assert!(!encoded.contains("SENTINEL"));
    assert!(!encoded.contains("private-text"));
    assert!(!encoded.contains("model.key"));
}
#[test]
fn media_content_is_not_read_or_claimed_verified_and_size_mismatch_is_explicit() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"original-media-not-a-decoded-image".to_vec();
    let image = media(&bytes);
    store
        .commit(
            vec![record(&store, Namespace::Source, vec![image.clone()])],
            BTreeMap::from([(image.sha256.clone(), bytes)]),
        )
        .unwrap();
    drop(store);
    let object = fixture.generation().join("objects").join(&image.sha256);
    fs::write(&object, vec![b'x'; image.bytes as usize]).unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Complete);
    assert!(!report.media_content_verified);
    fs::write(&object, b"short").unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.complete_apparent_bytes.is_none());
    assert_eq!(
        report.observed_domains[&Namespace::Source].size_mismatched_references,
        1
    );
    assert_eq!(
        report.observed_domains[&Namespace::Source].missing_references,
        0
    );
    assert_eq!(
        report.observed_domains[&Namespace::Source].present_referenced_media_bytes,
        5
    );
}
#[test]
fn missing_required_media_and_omitted_selected_media_never_become_zero_or_ready() {
    for omitted in [false, true] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let bytes = b"included-or-omitted-media".to_vec();
        let image = media(&bytes);
        let source = record(&store, Namespace::Source, vec![image.clone()]);
        if omitted {
            let (selected, objects) = manifest(&[source], &[], &[&image]);
            let scope = SelectionScope {
                group: Uuid::new_v4(),
                key_sha256: digest(b"key"),
                binding_sha256: digest(b"binding"),
            };
            store
                .initialize_selected(
                    &scope,
                    SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: digest(b"base"),
                        selected,
                        retained: vec![],
                    },
                    objects,
                )
                .unwrap();
        } else {
            store
                .commit(
                    vec![source],
                    BTreeMap::from([(image.sha256.clone(), bytes)]),
                )
                .unwrap();
        }
        drop(store);
        if !omitted {
            fs::remove_file(fixture.generation().join("objects").join(&image.sha256)).unwrap();
        }
        let report = fixture.inspect();
        assert_eq!(report.status, InspectionStatus::Partial);
        assert!(report.complete_apparent_bytes.is_none());
        let source = &report.observed_domains[&Namespace::Source];
        assert_eq!(source.declared_media_bytes, image.bytes);
        assert_eq!(source.missing_references, 1);
        assert_eq!(source.omitted_references, u64::from(omitted));
    }
}
#[test]
fn selected_pointer_and_history_deduplicate_refs_schema_two_is_supported_and_tombstones_retained() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"selected-media".to_vec();
    let image = media(&bytes);
    let mut original = record(&store, Namespace::Conversation, vec![image.clone()]);
    original.domain_schema_version = 2;
    let mut tombstone = original.clone();
    tombstone.revision_id = Uuid::new_v4();
    tombstone.operation_id = Uuid::new_v4();
    tombstone.parents.insert(original.revision_id);
    tombstone.kind = Kind::Tombstone;
    tombstone.payload = serde_json::Value::Null;
    tombstone.media_descriptors.clear();
    let (selected, objects) = manifest(&[original, tombstone], &[(&image, bytes)], &[]);
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"key"),
        binding_sha256: digest(b"binding"),
    };
    let initial = store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    store
        .activate_selected(
            &initial.token,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected: initial.transaction.selected.clone(),
                retained: vec![],
            },
            BTreeMap::new(),
        )
        .unwrap();
    drop(store);
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Complete);
    assert_eq!(
        report.observed_domains[&Namespace::Conversation].observed_record_files,
        2
    );
    assert_eq!(
        report.observed_domains[&Namespace::Conversation].declared_media_bytes,
        image.bytes
    );
    assert_eq!(report.storage[&StorageKind::MediaObject].files, 1);
    assert!(!report.logical_liveness_evaluated);
}
#[test]
fn staged_files_unknown_metadata_and_unsupported_generation_are_preserved_and_sanitized() {
    let fixture = Fixture::new();
    let store = fixture.open();
    drop(store);
    let staging = fixture
        .paths()
        .data
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&staging, b"STAGING_SENTINEL").unwrap();
    assert_eq!(
        fixture.inspect().storage[&StorageKind::Staging].apparent_bytes,
        16
    );
    fs::write(
        fixture.paths().data.join("UNRECOGNIZED_SECRET_FILENAME"),
        b"not-owned",
    )
    .unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.complete_apparent_bytes.is_none());
    assert!(!serde_json::to_string(&report).unwrap().contains("SENTINEL"));
    assert!(!serde_json::to_string(&report).unwrap().contains("SECRET"));
    fs::write(fixture.generation().join("format.json"), "99").unwrap();
    let report = fixture.inspect();
    assert_ne!(report.status, InspectionStatus::Complete);
    assert!(report
        .issues
        .contains_key(&InspectionIssue::UnsupportedGeneration));
}
#[test]
fn aggregate_entry_bound_reports_partial_without_cleaning_any_staging() {
    let fixture = Fixture::new();
    let store = fixture.open();
    drop(store);
    for _ in 0..MAX_ITEMS {
        fs::write(
            fixture
                .paths()
                .cache
                .join(format!(".stage-{}", Uuid::new_v4())),
            b"x",
        )
        .unwrap();
    }
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.issues.contains_key(&InspectionIssue::EntryLimit));
    assert!(report.complete_apparent_bytes.is_none());
    assert!(report.observed_files <= MAX_ITEMS as u64);
}
#[test]
fn foreign_cache_owner_overlap_and_missing_current_are_not_adopted_or_repaired() {
    let fixture = Fixture::new();
    let store = fixture.open();
    drop(store);
    fs::write(
        fixture.paths().cache.join("owner.json"),
        serde_json::to_vec(&json!({"format":1,"actor":Uuid::new_v4()})).unwrap(),
    )
    .unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(!report.cache_inspected);
    let mut paths = fixture.paths();
    paths.cache = paths.data.join("cache");
    assert_eq!(
        inspect_owned_data(&paths).status,
        InspectionStatus::Unsupported
    );
    assert!(!paths.cache.exists());
    fs::remove_file(fixture.paths().data.join("CURRENT")).unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Unavailable);
    assert!(!fixture.paths().data.join("CURRENT").exists());
}

#[test]
fn retained_generation_same_hash_is_another_apparent_path_charge() {
    fn copy_tree(source: &Path, target: &Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let path = entry.unwrap().path();
            let destination = target.join(path.file_name().unwrap());
            if path.is_dir() {
                copy_tree(&path, &destination);
            } else {
                fs::copy(path, destination).unwrap();
            }
        }
    }
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"shared-across-retained-paths".to_vec();
    let image = media(&bytes);
    store
        .commit(
            vec![record(&store, Namespace::Source, vec![image.clone()])],
            BTreeMap::from([(image.sha256.clone(), bytes)]),
        )
        .unwrap();
    drop(store);
    let original = fixture.generation();
    let retained = fixture
        .paths()
        .data
        .join("generations")
        .join(Uuid::new_v4().to_string());
    copy_tree(&original, &retained);
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Complete);
    assert_eq!(
        report.storage[&StorageKind::MediaObject].apparent_bytes,
        image.bytes
    );
    assert!(report.storage[&StorageKind::RetainedGeneration].apparent_bytes > image.bytes);
    assert_eq!(
        report.observed_domains[&Namespace::Source].declared_media_bytes,
        image.bytes * 2
    );
    assert_eq!(
        report.observed_domains[&Namespace::Source].observed_record_files,
        2
    );
}

#[test]
fn omitted_selected_reference_stays_omitted_when_retained_media_is_physically_present() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let bytes = b"retained-not-selected-media".to_vec();
    let image = media(&bytes);
    let winner = record(&store, Namespace::Source, vec![image.clone()]);
    let loser = record(&store, Namespace::SubjectMemory, vec![image.clone()]);
    let (selected, mut objects) = manifest(&[winner], &[], &[&image]);
    let (retained, more) = manifest(&[loser], &[(&image, bytes)], &[]);
    objects.extend(more);
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"key"),
        binding_sha256: digest(b"binding"),
    };
    store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected,
                retained: vec![retained],
            },
            objects,
        )
        .unwrap();
    drop(store);
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.complete_apparent_bytes.is_none());
    assert_eq!(
        report.observed_domains[&Namespace::Source].omitted_references,
        1
    );
    assert_eq!(
        report.observed_domains[&Namespace::Source].present_referenced_media_bytes,
        image.bytes
    );
    assert_eq!(
        report.observed_domains[&Namespace::Source].missing_references,
        0
    );
    assert_eq!(report.shared_media_files, 1);
    assert_eq!(
        report.storage[&StorageKind::MediaObject].apparent_bytes,
        image.bytes
    );
}

#[test]
fn global_json_budget_and_corrupt_private_metadata_are_explicit_without_echo() {
    let fixture = Fixture::new();
    let store = fixture.open();
    for _ in 0..10 {
        let mut item = record(&store, Namespace::Conversation, vec![]);
        item.payload = json!({"text": "large-saved-text-".repeat(57_000)});
        store.commit(vec![item], BTreeMap::new()).unwrap();
    }
    drop(store);
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.issues.contains_key(&InspectionIssue::MetadataLimit));
    assert!(report.complete_apparent_bytes.is_none());
    let corrupt = fixture
        .generation()
        .join("commits")
        .join(format!("{}.json", Uuid::new_v4()));
    fs::write(
        corrupt,
        b"{\"namespace\":\"PRIVATE_PARSER_VALUE_SENTINEL\"}",
    )
    .unwrap();
    let report = fixture.inspect();
    let encoded = serde_json::to_string(&report).unwrap();
    assert!(!encoded.contains("SENTINEL"));
    assert!(!encoded.contains("large-saved-text"));
    assert!(report.complete_apparent_bytes.is_none());
}

#[test]
fn missing_current_generation_or_selected_history_is_not_an_empty_complete_store() {
    let fixture = Fixture::new();
    let store = fixture.open();
    drop(store);
    let missing = Uuid::new_v4();
    fs::write(
        fixture.paths().data.join("CURRENT"),
        serde_json::to_vec(&missing).unwrap(),
    )
    .unwrap();
    let report = fixture.inspect();
    assert_ne!(report.status, InspectionStatus::Complete);
    assert!(report.complete_apparent_bytes.is_none());
    assert!(report.issues.contains_key(&InspectionIssue::MissingEntry));
}

#[test]
fn required_record_deletion_reports_missing_input_not_parser_corruption() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let item = record(&store, Namespace::Conversation, vec![]);
    let hash = digest(&serde_json::to_vec(&item).unwrap());
    store.commit(vec![item], BTreeMap::new()).unwrap();
    drop(store);
    fs::remove_file(fixture.generation().join("objects").join(hash)).unwrap();
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.issues.contains_key(&InspectionIssue::MissingEntry));
    assert!(!report
        .issues
        .contains_key(&InspectionIssue::InvalidMetadata));
    assert!(report.observed_domains.is_empty());
    assert!(!report.domain_accounting_complete);
}

#[test]
fn truncated_selected_history_is_uninspected_instead_of_inferred_missing() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let item = record(&store, Namespace::Conversation, vec![]);
    let (selected, objects) = manifest(&[item], &[], &[]);
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"key"),
        binding_sha256: digest(b"binding"),
    };
    let initial = store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    store
        .activate_selected(
            &initial.token,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected: initial.transaction.selected.clone(),
                retained: vec![],
            },
            BTreeMap::new(),
        )
        .unwrap();
    drop(store);
    let history = fixture.generation().join("selection-history");
    fs::remove_file(
        fs::read_dir(&history)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    // Pointer parsing precedes a separately incomplete cache scan. Do not turn
    // that incomplete whole-scope inspection into a definitive absence claim.
    for _ in 0..MAX_ITEMS {
        fs::write(
            fixture
                .paths()
                .cache
                .join(format!(".stage-{}", Uuid::new_v4())),
            b"x",
        )
        .unwrap();
    }
    let report = fixture.inspect();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(report.issues.contains_key(&InspectionIssue::EntryLimit));
    assert!(report
        .issues
        .contains_key(&InspectionIssue::UninspectedReference));
    assert!(!report.issues.contains_key(&InspectionIssue::MissingEntry));
    assert!(!report.issues.contains_key(&InspectionIssue::MissingObject));
    assert!(report.complete_apparent_bytes.is_none());
}

#[cfg(unix)]
#[test]
fn fifo_at_existing_lock_is_refused_before_blocking_open() {
    use std::process::Command;
    let fixture = Fixture::new();
    let store = fixture.open();
    drop(store);
    let lock = fixture.paths().data.join("store.lock");
    fs::remove_file(&lock).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&lock)
        .status()
        .unwrap()
        .success());
    let report = inspect_owned_data(&fixture.paths());
    assert_eq!(report.status, InspectionStatus::Unsupported);
    assert!(report.issues.contains_key(&InspectionIssue::UnsafeEntry));
    assert!(!fs::symlink_metadata(&lock).unwrap().is_file());
}

#[cfg(windows)]
#[test]
fn windows_case_alias_roots_are_rejected_before_double_accounting() {
    let fixture = Fixture::new();
    let store = fixture.open();
    drop(store);
    let mut paths = fixture.paths();
    paths.cache = PathBuf::from(paths.data.to_string_lossy().to_uppercase());
    let before = inventory(&fixture.0);
    let report = inspect_owned_data(&paths);
    assert_eq!(report.status, InspectionStatus::Unsupported);
    assert_eq!(inventory(&fixture.0), before);
}
