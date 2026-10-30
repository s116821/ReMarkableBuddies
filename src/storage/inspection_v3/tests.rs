use super::*;
use serde_json::Value;

#[test]
fn decimal_byte_totals_preserve_large_values_and_null_only_the_overflow() {
    use super::report::{Bytes, Count};
    assert_eq!(
        serde_json::to_string(&Count(u64::MAX)).unwrap(),
        "\"18446744073709551615\""
    );
    let mut bytes = Bytes::default();
    bytes.add(9_007_199_254_740_993);
    assert_eq!(
        serde_json::to_string(&bytes).unwrap(),
        "\"9007199254740993\""
    );
    bytes.add(u64::MAX);
    assert_eq!(serde_json::to_string(&bytes).unwrap(), "null");
    assert_eq!(serde_json::to_string(&Count(2)).unwrap(), "\"2\"");
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_does_not_inspect_or_initialize_roots() {
    let root = std::env::temp_dir().join(format!("report3-absent-{}", uuid::Uuid::new_v4()));
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    let mut bytes = vec![];
    inspect_owned_data(&paths).write_to(&mut bytes).unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["status"], "unsupported");
    assert_eq!(v["lease_identity_check"], "unavailable");
    assert!(!root.exists());
}

#[cfg(target_os = "linux")]
mod linux_controls {
    use super::*;
    use crate::storage::{self, Store, Uuid};
    use std::{
        collections::BTreeMap,
        fs,
        io::{self, Write},
        path::{Path, PathBuf},
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("report3-{}", Uuid::new_v4())))
        }
        fn paths(&self) -> StorePaths {
            StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            }
        }
        fn generation(&self) -> PathBuf {
            let id: Uuid = storage::files::json(&self.paths().data.join("CURRENT"), 128).unwrap();
            self.paths().data.join("generations").join(id.to_string())
        }
        fn report(&self) -> Value {
            let mut bytes = vec![];
            inspect_owned_data(&self.paths())
                .write_to(&mut bytes)
                .unwrap();
            assert!(bytes.len() <= 32768 && bytes.ends_with(b"\n"));
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v.as_object().unwrap().len(), 26);
            v
        }
        fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
            fn visit(path: &Path, all: &mut BTreeMap<PathBuf, Vec<u8>>) {
                if path.exists() {
                    for entry in fs::read_dir(path).unwrap() {
                        let path = entry.unwrap().path();
                        if path.is_dir() {
                            visit(&path, all);
                        } else {
                            all.insert(path.clone(), fs::read(&path).unwrap());
                        }
                    }
                }
            }
            let mut all = BTreeMap::new();
            visit(&self.0, &mut all);
            all
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn complete_empty_legacy_and_notebook_are_distinct_and_read_only() {
        let f = Fixture::new();
        drop(Store::open(f.paths()).unwrap());
        let before = f.snapshot();
        let legacy = f.report();
        assert_eq!(f.snapshot(), before);
        assert_eq!(legacy["status"], "complete");
        assert_eq!(legacy["current_generation_format"], 1);
        assert_eq!(legacy["observed_domains"].as_object().unwrap().len(), 6);
        assert_eq!(legacy["storage"].as_object().unwrap().len(), 8);
        assert_eq!(
            legacy["observed_notebook_records"]
                .as_object()
                .unwrap()
                .len(),
            3
        );
        drop(Store::prepare_notebook(f.paths()).unwrap());
        let before = f.snapshot();
        let notebook = f.report();
        assert_eq!(f.snapshot(), before);
        assert_eq!(notebook["status"], "complete");
        assert_eq!(notebook["current_generation_format"], 4);
        assert_eq!(
            notebook["complete_apparent_bytes"],
            notebook["observed_apparent_bytes"]
        );
        let old = storage::inspection::inspect_owned_data(&f.paths());
        assert_eq!(serde_json::to_value(old).unwrap()["status"], "unsupported");
    }
    #[test]
    fn retained_notebook_and_feedback_inside_current_four_have_independent_bindings() {
        use crate::{conversation::AttemptReason, unassociated_feedback::Feedback};
        let f = Fixture::new();
        let feedback = Feedback::prepare(f.paths()).unwrap();
        feedback
            .retain(Uuid::new_v4(), 1, AttemptReason::Canceled, false)
            .unwrap();
        drop(feedback);
        drop(Store::prepare_notebook(f.paths()).unwrap());
        let v = f.report();
        assert_eq!(v["status"], "complete");
        assert_eq!(
            v["observed_domains"]["unassociated-feedback"]["observed_record_files"],
            "1"
        );
        let legacy = Uuid::new_v4();
        Store::create_generation(&f.paths().data, legacy).unwrap();
        storage::files::atomic_json(&f.paths().data.join("CURRENT"), &legacy).unwrap();
        let before = f.snapshot();
        let v = f.report();
        assert_eq!(v["status"], "complete");
        assert_eq!(v["current_generation_format"], 1);
        assert_eq!(v["retained_generation_formats"], serde_json::json!([4]));
        assert_eq!(f.snapshot(), before);
    }
    fn inject_tombstone(f: &Fixture, namespace: storage::Namespace) {
        use storage::{digest, Envelope, Kind, Manifest, ObjectRef, Scope};
        let identity: Value =
            storage::files::json(&f.paths().data.join("identity.json"), 4096).unwrap();
        let envelope = Envelope {
            envelope_version: 1,
            namespace,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: serde_json::from_value(identity["actor"].clone()).unwrap(),
            kind: Kind::Tombstone,
            payload: Value::Null,
            media_descriptors: vec![],
        };
        envelope.validate().unwrap();
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let hash = digest(&bytes);
        let transaction_id = Uuid::new_v4();
        let manifest = Manifest {
            format: 1,
            transaction_id,
            scope: Scope::LocalTransaction,
            records: vec![ObjectRef {
                sha256: hash.clone(),
                bytes: bytes.len() as u64,
            }],
            record_namespaces: BTreeMap::from([(hash.clone(), namespace)]),
            media: vec![],
            media_coverage: vec![],
        };
        manifest.validate().unwrap();
        fs::write(f.generation().join("objects").join(hash), bytes).unwrap();
        fs::write(
            f.generation()
                .join("commits")
                .join(format!("{transaction_id}.json")),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn feedback_tombstones_refuse_with_or_without_feature_but_generic_tombstones_remain_valid() {
        for feature in [false, true] {
            let f = Fixture::new();
            if feature {
                drop(crate::unassociated_feedback::Feedback::prepare(f.paths()).unwrap());
            }
            drop(Store::prepare_notebook(f.paths()).unwrap());
            inject_tombstone(&f, storage::Namespace::UnassociatedFeedback);
            let before = f.snapshot();
            let v = f.report();
            assert_eq!(v["status"], "partial");
            assert!(v["issues"]["invalid-reference"].is_string());
            assert_eq!(f.snapshot(), before);
        }
        for namespace in [storage::Namespace::Source, storage::Namespace::Conversation] {
            let f = Fixture::new();
            drop(Store::open(f.paths()).unwrap());
            inject_tombstone(&f, namespace);
            let v = f.report();
            assert_eq!(v["status"], "complete");
        }
    }
    #[test]
    fn retained_unknown_object_matches_the_explicit_unclassified_physical_bucket() {
        let f = Fixture::new();
        drop(Store::prepare_notebook(f.paths()).unwrap());
        let retained = f.generation();
        let legacy = Uuid::new_v4();
        Store::create_generation(&f.paths().data, legacy).unwrap();
        storage::files::atomic_json(&f.paths().data.join("CURRENT"), &legacy).unwrap();
        let unknown = b"PRIVATE_UNCLASSIFIED_OBJECT";
        fs::write(retained.join("objects").join("a".repeat(64)), unknown).unwrap();
        let before = f.snapshot();
        let v = f.report();
        assert_eq!(v["status"], "partial");
        assert_eq!(v["storage"]["unclassified-object"]["files"], "1");
        assert_eq!(
            v["storage"]["unclassified-object"]["apparent_bytes"],
            unknown.len().to_string()
        );
        assert!(!v.to_string().contains("PRIVATE_UNCLASSIFIED_OBJECT"));
        assert_eq!(f.snapshot(), before);
    }
    #[test]
    fn unknown_stage_prefix_never_becomes_recognized_staging() {
        let f = Fixture::new();
        drop(Store::open(f.paths()).unwrap());
        for parent in [
            f.paths().data,
            f.generation(),
            f.generation().join("objects"),
            f.generation().join("commits"),
        ] {
            let path = parent.join(".stage-not-an-id");
            fs::write(&path, b"PRIVATE_UNKNOWN").unwrap();
            let before = f.snapshot();
            let v = f.report();
            assert_eq!(v["status"], "partial");
            assert!(v["issues"]["unexpected-entry"].is_string());
            assert_eq!(f.snapshot(), before);
            fs::remove_file(path).unwrap();
        }
        let path = f.paths().data.join(format!(".stage-{}", Uuid::new_v4()));
        fs::write(path, b"owned interrupted stage").unwrap();
        let v = f.report();
        assert_eq!(v["status"], "complete");
        assert_eq!(v["storage"]["staging"]["files"], "1");
    }
    #[test]
    fn custody_is_held_through_output_and_released_on_prefix_failure() {
        let f = Fixture::new();
        drop(Store::open(f.paths()).unwrap());
        let before = f.snapshot();
        struct Prefix<'a> {
            f: &'a Fixture,
            writes: usize,
        }
        impl Write for Prefix<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                assert!(Store::open_existing(self.f.paths()).is_err());
                self.writes += 1;
                if self.writes == 1 {
                    Ok(bytes.len().min(7))
                } else {
                    Err(io::Error::other("synthetic prefix failure"))
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Prefix { f: &f, writes: 0 };
        assert!(inspect_owned_data(&f.paths())
            .write_to(&mut writer)
            .is_err());
        assert_eq!(writer.writes, 2);
        assert_eq!(f.snapshot(), before);
        drop(Store::open_existing(f.paths()).unwrap());
    }
    #[test]
    fn busy_and_changed_witness_never_become_complete() {
        let f = Fixture::new();
        let store = Store::open(f.paths()).unwrap();
        assert_eq!(f.report()["status"], "busy");
        drop(store);
        let inspection = inspect_owned_data(&f.paths());
        fs::write(f.paths().data.join("CURRENT"), b"null").unwrap();
        let mut bytes = vec![];
        assert!(inspection.write_to(&mut bytes).is_err());
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], "partial");
        assert!(v["complete_apparent_bytes"].is_null());
    }
    #[test]
    fn renamed_root_ancestor_with_retained_handles_refuses_output_completion() {
        let f = Fixture::new();
        drop(Store::open(f.paths()).unwrap());
        let inspection = inspect_owned_data(&f.paths());
        let moved = f.0.with_extension("moved");
        fs::rename(&f.0, &moved).unwrap();
        let mut output = vec![];
        let result = inspection.write_to(&mut output);
        fs::rename(&moved, &f.0).unwrap();
        assert!(result.is_err());
        let v: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(v["status"], "partial");
        assert!(v["complete_apparent_bytes"].is_null());
    }
    #[test]
    fn nofollow_fifo_unknown_objects_and_foreign_feature_refuse_without_reads_or_repair() {
        let f = Fixture::new();
        drop(Store::prepare_notebook(f.paths()).unwrap());
        let unknown = f.generation().join("objects").join("a".repeat(64));
        fs::write(&unknown, b"PRIVATE_OBJECT_PAYLOAD").unwrap();
        let before = f.snapshot();
        let v = f.report();
        assert_eq!(v["status"], "partial");
        assert!(v["issues"]["unclassified-object"].is_string());
        assert!(!v.to_string().contains("PRIVATE_OBJECT_PAYLOAD"));
        assert_eq!(f.snapshot(), before);
        fs::remove_file(unknown).unwrap();
        let marker = f.generation().join("notebook-feature.json");
        let mut feature: Value = storage::files::json(&marker, 4096).unwrap();
        feature["actor"] = serde_json::json!(Uuid::new_v4());
        fs::write(&marker, serde_json::to_vec(&feature).unwrap()).unwrap();
        let before = f.snapshot();
        assert_eq!(f.report()["status"], "unsupported");
        assert_eq!(f.snapshot(), before);
        let fresh = Fixture::new();
        drop(Store::open(fresh.paths()).unwrap());
        let lock = fresh.paths().data.join("store.lock");
        fs::remove_file(&lock).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&lock)
            .status()
            .unwrap()
            .success());
        assert_ne!(fresh.report()["status"], "complete");
        fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink("/dev/zero", &lock).unwrap();
        assert_ne!(fresh.report()["status"], "complete");
    }
    #[test]
    fn notebook_archive_descriptor_counts_metadata_without_reading_media_payload() {
        use crate::{
            conversation::{
                notebook::{
                    ArchivedNormalizedSource, NotebookPlan, NotebookStage, NotebookStageKind,
                },
                Record, TextScope,
            },
            retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
            writer::notebook::ValidatedNotebook,
        };
        use storage::{digest, Envelope, Kind, Namespace};
        let f = Fixture::new();
        let store = Store::prepare_notebook(f.paths()).unwrap();
        let document = NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "fixture-v1".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "PRIVATE_ARCHIVED_TEXT".into(),
            }],
        };
        let raw = serde_json::to_vec(&document).unwrap();
        let source = ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap();
        let archive = ArchivedNormalizedSource::from_notebook(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &source,
            TextScope::SuppliedNotebook,
            None,
        )
        .unwrap();
        let media = archive.media.clone();
        let envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::Source,
            domain_schema_version: 3,
            record_id: archive.id,
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::ArchivedNormalizedSource(Box::new(
                archive.clone(),
            )))
            .unwrap(),
            media_descriptors: vec![media.clone()],
        };
        let source_revision = digest(&serde_json::to_vec(&envelope).unwrap());
        store
            .commit(
                vec![envelope],
                BTreeMap::from([(media.sha256.clone(), source.canonical_bytes().to_vec())]),
            )
            .unwrap();
        let operation = Uuid::new_v4();
        let mut plan = NotebookPlan {
            id: NotebookPlan::identity(operation),
            conversation: archive.conversation,
            operation,
            archive_id: archive.id,
            source_revision: source_revision.clone(),
            source_identity_digest: archive.identity_digest().unwrap(),
            request: "PRIVATE_NOTE_REQUEST".into(),
            guidance: None,
            personalization_digest: digest(b"preference"),
            history_digest: digest(b"history"),
            prior: None,
            caller_max_model_calls: 2,
            chunks: source
                .coverage(&TextScope::SuppliedNotebook, 2)
                .unwrap()
                .chunks()
                .to_vec(),
            core_digest: String::new(),
            map_input_digests: vec![digest(b"map input")],
            reducer_prefix_digest: digest(b"reducer"),
            reducer_reserved_bytes: 8192,
        };
        plan.core_digest = plan.core_identity().unwrap();
        let plan_envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::Conversation,
            domain_schema_version: 3,
            record_id: plan.id,
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::NotebookPlan(Box::new(plan.clone()))).unwrap(),
            media_descriptors: vec![],
        };
        let stage = NotebookStage {
            id: NotebookStage::identity(operation, NotebookStageKind::Map, 0),
            conversation: archive.conversation,
            operation,
            plan_id: plan.id,
            core_digest: plan.core_digest.clone(),
            plan_revision: digest(&serde_json::to_vec(&plan_envelope).unwrap()),
            kind: NotebookStageKind::Map,
            ordinal: 0,
            archive_id: archive.id,
            source_revision,
            input_digest: plan.map_input_digests[0].clone(),
            predecessors: vec![],
            prepared_revision: None,
            outcome: None,
            invoked: None,
            output: None,
            observed_output_digest: None,
            observed_output_bytes: None,
            failure: None,
        };
        let stage_envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::Conversation,
            domain_schema_version: 3,
            record_id: stage.id,
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::NotebookStage(Box::new(stage))).unwrap(),
            media_descriptors: vec![],
        };
        store
            .commit(vec![plan_envelope, stage_envelope], BTreeMap::new())
            .unwrap();
        let shared = Envelope {
            envelope_version: 1,
            namespace: Namespace::SubjectMemory,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::json!({"fixture":"shared-owned-reference"}),
            media_descriptors: vec![media.clone()],
        };
        store.commit(vec![shared], BTreeMap::new()).unwrap();
        drop(store);
        // Same-length corruption is deliberately invisible: this observer sizes
        // the owned media file and never claims to hash/decode its payload.
        fs::write(
            f.generation().join("objects").join(&media.sha256),
            vec![b'x'; media.bytes as usize],
        )
        .unwrap();
        let before = f.snapshot();
        let media_path = f.generation().join("objects").join(&media.sha256);
        use std::os::unix::fs::PermissionsExt;
        let original_permissions = fs::metadata(&media_path).unwrap().permissions();
        fs::set_permissions(&media_path, fs::Permissions::from_mode(0)).unwrap();
        let denied_media_read = fs::File::open(&media_path).is_err();
        let v = f.report();
        fs::set_permissions(&media_path, original_permissions).unwrap();
        // Privileged manual runners cannot establish permission-denied reads.
        if unsafe { nix::libc::geteuid() } != 0 {
            assert!(denied_media_read);
        }
        assert_eq!(v["status"], "complete");
        assert_eq!(
            v["observed_notebook_records"]["archived-normalized-source"]["files"],
            "1"
        );
        assert_eq!(v["media_content_verified"], false);
        assert_eq!(v["shared_media_files"], "1");
        assert_eq!(
            v["observed_notebook_records"]["notebook-plan"]["files"],
            "1"
        );
        assert_eq!(
            v["observed_notebook_records"]["notebook-stage"]["files"],
            "1"
        );
        assert!(!v.to_string().contains("PRIVATE_NOTE_REQUEST"));
        assert!(!v.to_string().contains("PRIVATE_ARCHIVED_TEXT"));
        assert_eq!(f.snapshot(), before);
        fs::remove_file(f.generation().join("objects").join(media.sha256)).unwrap();
        let v = f.report();
        assert_eq!(v["status"], "partial");
        assert_eq!(v["observed_domains"]["source"]["missing_references"], "1");
    }
}
