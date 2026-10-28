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
