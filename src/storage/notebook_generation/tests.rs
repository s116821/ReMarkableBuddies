use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("notebook-adoption-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn root(&self) -> PathBuf {
        let generation: Uuid = files::json(&self.paths().data.join("CURRENT"), 128).unwrap();
        self.paths()
            .data
            .join("generations")
            .join(generation.to_string())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn retained(store: &Store, bytes: &[u8]) -> Envelope {
    let media = Media {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
        media_type: "text/plain".into(),
    };
    Envelope {
        envelope_version: 1,
        namespace: Namespace::SubjectMemory,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::json!({"fixture":"retained"}),
        media_descriptors: vec![media],
    }
}

#[test]
fn in_place_adoption_preserves_current_and_all_retained_bytes() {
    let f = Fixture::new();
    let store = Store::open(f.paths()).unwrap();
    let current = fs::read(f.paths().data.join("CURRENT")).unwrap();
    let legacy = fs::read(f.paths().data.join("contract-v1.json")).unwrap();
    let bytes = b"original retained media\n";
    let record = retained(&store, bytes);
    let receipt = store
        .commit(
            vec![record.clone()],
            BTreeMap::from([(digest(bytes), bytes.to_vec())]),
        )
        .unwrap();
    let raw_record = serde_json::to_vec(&record).unwrap();
    let root = f.root();
    let manifest = fs::read(root.join("commits").join(format!("{receipt}.json"))).unwrap();
    drop(store);
    let store = Store::prepare_notebook(f.paths()).unwrap();
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
    assert_eq!(f.root(), root);
    assert_eq!(
        fs::read(f.paths().data.join("contract-v1.json")).unwrap(),
        legacy
    );
    assert_eq!(
        fs::read(root.join("objects").join(digest(&raw_record))).unwrap(),
        raw_record
    );
    assert_eq!(
        fs::read(root.join("objects").join(digest(bytes))).unwrap(),
        bytes
    );
    assert_eq!(
        fs::read(root.join("commits").join(format!("{receipt}.json"))).unwrap(),
        manifest
    );
    assert_eq!(feedback_generation::version(&root).unwrap(), 4);
    assert_eq!(
        store
            .media_references(&record.media_descriptors[0])
            .unwrap(),
        1
    );
    drop(store);
    drop(Store::open(f.paths()).unwrap());
}

#[test]
fn interrupted_adoption_refuses_ordinary_open_and_explicitly_resumes() {
    for fault in [
        PreparationStage::BeforeMarker,
        PreparationStage::AfterMarker,
        PreparationStage::AfterContract,
        PreparationStage::AfterFormat,
        PreparationStage::AfterCapabilities,
    ] {
        let f = Fixture::new();
        let mut store = Store::open_policy(f.paths(), OpenPreparation::Notebook).unwrap();
        let current = fs::read(f.paths().data.join("CURRENT")).unwrap();
        assert!(prepare(&mut store, |stage| {
            if stage == fault {
                anyhow::bail!("controlled interruption");
            }
            Ok(())
        })
        .is_err());
        drop(store);
        if matches!(
            fault,
            PreparationStage::AfterMarker
                | PreparationStage::AfterContract
                | PreparationStage::AfterFormat
        ) {
            assert!(Store::open(f.paths()).is_err());
        }
        drop(Store::prepare_notebook(f.paths()).unwrap());
        assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
        drop(Store::open(f.paths()).unwrap());
    }
}

#[test]
#[cfg(unix)]
fn atomic_format_failure_uses_observed_format_and_never_returns_effect_custody() {
    for fault in [
        files::AtomicFailure::BeforeRename,
        files::AtomicFailure::RenameAcknowledgment,
        files::AtomicFailure::ParentSync,
    ] {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        let record = retained(&store, b"immutable history through lost acknowledgment");
        store
            .commit(
                vec![record],
                BTreeMap::from([(
                    digest(b"immutable history through lost acknowledgment"),
                    b"immutable history through lost acknowledgment".to_vec(),
                )]),
            )
            .unwrap();
        drop(store);
        let identity = fs::read(f.paths().data.join("identity.json")).unwrap();
        let current = fs::read(f.paths().data.join("CURRENT")).unwrap();
        let original = ["objects", "commits"]
            .into_iter()
            .flat_map(|directory| {
                fs::read_dir(f.root().join(directory))
                    .unwrap()
                    .map(|entry| {
                        let path = entry.unwrap().path();
                        let bytes = fs::read(&path).unwrap();
                        (path, bytes)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        files::fail_next_atomic(f.root().join("format.json"), fault);
        let admission = Store::prepare_notebook_validated(f.paths(), |_| Ok(()));
        assert!(
            admission.is_err(),
            "failed acknowledgment must not return Store/effect custody"
        );
        assert_eq!(
            files::json::<u32>(&f.root().join("format.json"), 64).unwrap(),
            if fault == files::AtomicFailure::BeforeRename {
                1
            } else {
                VERSION
            }
        );
        assert!(
            Store::open(f.paths()).is_err(),
            "ordinary reopen must not finish incomplete adoption"
        );
        assert_eq!(
            fs::read(f.paths().data.join("identity.json")).unwrap(),
            identity
        );
        assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
        for (path, bytes) in &original {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
        drop(Store::prepare_notebook(f.paths()).unwrap());
        drop(Store::open(f.paths()).unwrap());
        assert_eq!(
            fs::read(f.paths().data.join("identity.json")).unwrap(),
            identity
        );
        assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
        for (path, bytes) in original {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn complete_custody_is_required_and_corrupt_media_is_not_discarded() {
    let f = Fixture::new();
    let store = Store::open(f.paths()).unwrap();
    assert!(
        Store::prepare_notebook(f.paths()).is_err(),
        "live process lease must exclude adoption"
    );
    let record = retained(&store, b"valid");
    store
        .commit(
            vec![record.clone()],
            BTreeMap::from([(digest(b"valid"), b"valid".to_vec())]),
        )
        .unwrap();
    fs::write(f.root().join("objects").join(digest(b"valid")), b"corrupt").unwrap();
    drop(store);
    assert!(Store::prepare_notebook(f.paths()).is_err());
    assert_eq!(feedback_generation::version(&f.root()).unwrap(), 1);
    assert_eq!(
        fs::read(
            f.root()
                .join("objects")
                .join(record.media_descriptors[0].sha256.clone())
        )
        .unwrap(),
        b"corrupt"
    );
    assert!(!f.root().join(MARKER).exists());
}

#[test]
fn foreign_feature_and_wrong_capabilities_refuse_without_ordinary_repair() {
    let f = Fixture::new();
    let store = Store::prepare_notebook(f.paths()).unwrap();
    let actor = store.actor_id;
    let generation = store.inner.lock().unwrap().generation;
    drop(store);
    let path = f.paths().data.join("capabilities.json");
    fs::write(&path, b"{}").unwrap();
    assert!(Store::open(f.paths()).is_err());
    assert_eq!(
        fs::read(&path).unwrap(),
        b"{}",
        "ordinary open cannot repair incomplete adoption"
    );
    drop(Store::prepare_notebook(f.paths()).unwrap());
    files::atomic_json(
        &f.root().join(MARKER),
        &Feature {
            format: 1,
            actor: Uuid::new_v4(),
            generation,
        },
    )
    .unwrap();
    assert!(Store::open(f.paths()).is_err());
    assert!(Store::prepare_notebook(f.paths()).is_err());
    assert_ne!(
        files::json::<Feature>(&f.root().join(MARKER), 4096)
            .unwrap()
            .actor,
        actor
    );
}

#[test]
fn feedback_feature_bytes_and_contract2_survive_without_downgrade() {
    let f = Fixture::new();
    let feedback = crate::unassociated_feedback::Feedback::prepare(f.paths()).unwrap();
    drop(feedback);
    let marker = fs::read(f.root().join(feedback_generation::MARKER)).unwrap();
    let contract2 = fs::read(f.paths().data.join("contract-v2.json")).unwrap();
    drop(Store::prepare_notebook(f.paths()).unwrap());
    let feedback = crate::unassociated_feedback::Feedback::prepare(f.paths()).unwrap();
    assert!(feedback.snapshot().unwrap().is_empty());
    drop(feedback);
    assert_eq!(feedback_generation::version(&f.root()).unwrap(), 4);
    assert_eq!(
        fs::read(f.root().join(feedback_generation::MARKER)).unwrap(),
        marker
    );
    assert_eq!(
        fs::read(f.paths().data.join("contract-v2.json")).unwrap(),
        contract2
    );
}

#[test]
fn legacy_maintenance_and_generic_inspection_refuse_notebook_generation() {
    let f = Fixture::new();
    drop(Store::prepare_notebook(f.paths()).unwrap());
    assert!(Store::open_existing(f.paths()).is_err());
    let report = inspection::inspect_owned_data(&f.paths());
    assert_eq!(report.status, inspection::InspectionStatus::Unsupported);
    assert!(
        report.current_generation_format.is_none()
            || report.current_generation_format == Some(None)
    );
}

#[test]
fn explicit_local_only_adoption_and_real_ordinary_leases_allow_legacy_publication() {
    for explicit in [false, true] {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        if explicit {
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    sync::mode::SyncModeChoice::LocalOnly,
                )
                .unwrap();
        }
        drop(store);
        let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
        let ordinary = store
            .ordinary_mode_lease(&store.sync_mode_snapshot().unwrap().token)
            .unwrap();
        let local = store.local_domain_lease().unwrap();
        let record = retained(&store, b"ordinary retained bytes");
        store
            .commit(
                vec![record.clone()],
                BTreeMap::from([(
                    digest(b"ordinary retained bytes"),
                    b"ordinary retained bytes".to_vec(),
                )]),
            )
            .unwrap();
        let ledger = crate::conversation::Ledger::new(store.clone());
        ledger.create(Uuid::new_v4(), Uuid::new_v4(), 1).unwrap();
        assert!(store.legacy_publisher_lease().is_err());
        assert!(store.legacy_sync_lease().is_err());
        assert!(store
            .sync_mode_lease(&store.sync_mode_snapshot().unwrap().token)
            .is_err());
        drop(local);
        drop(ordinary);
        drop(ledger);
        let backup = f.0.join("archive");
        store.export_notebook_backup(&backup).unwrap();
        store.restore_notebook_backup(&backup).unwrap();
        assert_eq!(
            store
                .media_references(&record.media_descriptors[0])
                .unwrap(),
            1
        );
    }
}

#[test]
fn nonlocal_mode_refuses_before_notebook_marker_or_format_mutation() {
    let f = Fixture::new();
    let store = Store::open(f.paths()).unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            sync::mode::SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"nonlocal"),
            },
        )
        .unwrap();
    let format = fs::read(f.root().join("format.json")).unwrap();
    drop(store);
    assert!(Store::prepare_notebook(f.paths()).is_err());
    assert_eq!(fs::read(f.root().join("format.json")).unwrap(), format);
    assert!(!f.root().join(MARKER).exists());
}
#[test]
fn command_preflight_refusal_and_escaped_reader_cannot_publish_notebook_adoption() {
    let f = Fixture::new();
    let conversation = Uuid::new_v4();
    {
        let ledger = crate::conversation::Ledger::new(Arc::new(Store::open(f.paths()).unwrap()));
        ledger.create(conversation, Uuid::new_v4(), 0).unwrap();
    }
    let current = fs::read(f.paths().data.join("CURRENT")).unwrap();
    let failed = Store::prepare_notebook_validated(f.paths(), |ledger| -> Result<()> {
        ledger.expected(Namespace::Conversation, conversation)?;
        anyhow::bail!("bounded preflight refusal")
    });
    assert!(failed.is_err());
    assert!(!f.root().join(MARKER).exists());
    let mut escaped = None;
    assert!(Store::prepare_notebook_validated(f.paths(), |ledger| {
        escaped = Some(ledger.store().clone());
        Ok(())
    })
    .is_err());
    assert!(!f.root().join(MARKER).exists());
    drop(escaped);
    let (store, expected) = Store::prepare_notebook_validated(f.paths(), |ledger| {
        ledger.expected(Namespace::Conversation, conversation)
    })
    .unwrap();
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
    assert_eq!(
        store
            .heads(Namespace::Conversation, conversation)
            .unwrap()
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>(),
        expected.0
    );
}
#[test]
fn retained_revision_reference_preserves_noncanonical_record_bytes_and_checks_actual_object() {
    let f = Fixture::new();
    let store = Store::prepare_notebook(f.paths()).unwrap();
    let record = retained(&store, b"retained object");
    let transaction = store
        .commit(
            vec![record.clone()],
            BTreeMap::from([(digest(b"retained object"), b"retained object".to_vec())]),
        )
        .unwrap();
    let root = f.root();
    drop(store);
    let path = root.join("commits").join(format!("{transaction}.json"));
    let mut manifest: Manifest = files::json(&path, MAX_METADATA as u64).unwrap();
    let original = manifest.records[0].sha256.clone();
    let mut bytes = serde_json::to_vec_pretty(&record).unwrap();
    bytes.push(b'\n');
    let actual = digest(&bytes);
    fs::write(root.join("objects").join(&actual), &bytes).unwrap();
    manifest.records[0] = ObjectRef {
        sha256: actual.clone(),
        bytes: bytes.len() as u64,
    };
    let namespace = manifest.record_namespaces.remove(&original).unwrap();
    manifest.record_namespaces.insert(actual.clone(), namespace);
    files::atomic_json(&path, &manifest).unwrap();
    let store = Store::open(f.paths()).unwrap();
    let reference = store.notebook_revision_reference(&record).unwrap();
    assert_eq!(reference.sha256, actual);
    assert_ne!(
        reference.sha256,
        digest(&serde_json::to_vec(&record).unwrap())
    );
    assert_eq!(reference.bytes, bytes.len() as u64);
    fs::write(root.join("objects").join(&actual), b"corrupt").unwrap();
    assert!(store.notebook_revision_reference(&record).is_err());
}

#[test]
fn actual_selected_history_refuses_but_empty_bound_feature_marker_is_preserved() {
    for nonempty in [false, true] {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        let generation = store.inner.lock().unwrap().generation;
        selection::prepare_generation(&f.root(), store.actor_id, generation).unwrap();
        if nonempty {
            let (mut selected, objects) = Store::ordinary_transaction(
                vec![retained(&store, b"selected")],
                BTreeMap::from([(digest(b"selected"), b"selected".to_vec())]),
            )
            .unwrap();
            selected.scope = Scope::SelectedRecords;
            store
                .initialize_selected(
                    &selection::SelectionScope {
                        group: Uuid::new_v4(),
                        key_sha256: digest(b"scope"),
                        binding_sha256: digest(b"binding"),
                    },
                    selection::SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: digest(b"base"),
                        selected,
                        retained: vec![],
                    },
                    objects,
                )
                .unwrap();
        }
        let marker = fs::read(f.root().join("selection-feature.json")).unwrap();
        let format = fs::read(f.root().join("format.json")).unwrap();
        drop(store);
        if nonempty {
            assert!(Store::prepare_notebook(f.paths()).is_err());
            assert!(!f.root().join(MARKER).exists());
            assert_eq!(fs::read(f.root().join("format.json")).unwrap(), format);
        } else {
            let store = Store::prepare_notebook(f.paths()).unwrap();
            assert_eq!(
                fs::read(f.root().join("selection-feature.json")).unwrap(),
                marker
            );
            let archive = f.0.join("empty-feature-backup");
            store.export_notebook_backup(&archive).unwrap();
            store.restore_notebook_backup(&archive).unwrap();
            let feature: serde_json::Value =
                files::json(&f.root().join("selection-feature.json"), 4096).unwrap();
            let generation = store.inner.lock().unwrap().generation;
            assert_eq!(
                feature,
                serde_json::json!({"format":1,"actor":store.actor_id,"generation":generation})
            );
            drop(store);
            drop(Store::open(f.paths()).unwrap());
        }
        if nonempty {
            assert_eq!(
                fs::read(f.root().join("selection-feature.json")).unwrap(),
                marker
            );
        }
    }
}
