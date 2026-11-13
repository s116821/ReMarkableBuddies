use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-compound-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn current(&self) -> Uuid {
        files::json(&self.paths().data.join("CURRENT"), 128).unwrap()
    }
    fn request(&self) -> Request {
        let store = Store::prepare_notebook(self.paths()).unwrap();
        let token = serde_json::to_value(&store.sync_mode_snapshot().unwrap().token).unwrap();
        Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: self.current(),
            group: Uuid::new_v4(),
            binding_sha256: digest(b"compound synthetic binding"),
            expected_mode: ModeEvidence {
                actor: token["actor"].as_str().unwrap().parse().unwrap(),
                sha256: token["sha256"].as_str().unwrap().into(),
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn produce_notebook(store: Arc<Store>) -> Uuid {
    use crate::{
        conversation::{notebook::*, *},
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
        writer::notebook::{
            runner::{self, Input, Status},
            *,
        },
    };
    struct Model(usize);
    impl crate::LLMEngine for Model {
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, _: &str) {
            panic!("archived notebook text is not recognition");
        }
        fn clear_content(&mut self) {}
        fn execute(&mut self) -> Result<String> {
            self.0 += 1;
            Ok(if self.0 == 1 {
                "Force = 10 N. (page 1)."
            } else {
                "# Force\n\nForce = 10 N. (page 1)."
            }
            .into())
        }
    }
    let ledger = Ledger::new(store);
    let conversation = Uuid::new_v4();
    let operation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 0).unwrap();
    let document_id = Uuid::new_v4();
    let document = NormalizedDocument {
        document: document_id,
        format: SourceFormat::Notebook,
        source_sha256: digest(b"original source"),
        extractor_revision: "compound-fixture-v1".into(),
        units: vec![SourceUnit {
            page: Some(1),
            section: None,
            text: "Force = 10 N.".into(),
        }],
    };
    let raw = serde_json::to_vec(&document).unwrap();
    let source = ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap();
    let user_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:writer:v1:{operation}:user").as_bytes(),
    );
    let archive = ArchivedNormalizedSource::from_notebook(
        conversation,
        user_id,
        &source,
        TextScope::SuppliedNotebook,
        None,
    )
    .unwrap();
    let preference = crate::config::Personalization::default()
        .snapshot()
        .unwrap();
    let payloads = FrozenNotebookPayloads::new(
        &source,
        &archive,
        PayloadContext {
            operation,
            request: "Generate study notes",
            personalization: &preference,
            history: &[],
            history_range: None,
            prior: None,
            max_model_calls: 2,
        },
    )
    .unwrap();
    let user = Turn {
        id: user_id,
        conversation,
        exchange: operation,
        sequence: 0,
        role: Role::User,
        mode: Mode::Writer,
        outcome: Outcome::Interpreted,
        text: Some(payloads.plan.request.clone()),
        sources: vec![archive.id],
        correction_of: None,
        created_ms: 0,
        updated_ms: 0,
        completion: None,
    };
    let input = Input {
        admission: NotebookAdmission {
            plan: payloads.plan.clone(),
            archive,
            canonical: source.canonical_bytes().to_vec(),
            user,
            expected: ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            guards: vec![],
        },
        payloads,
    };
    let mut model = Model(0);
    let result = runner::run(&ledger, &mut model, input, &mut || false);
    assert_eq!(result.report.status, Status::Generated);
    assert_eq!(model.0, 2);
    document_id
}

#[test]
fn foreign_notebook_closure_keeps_raw_actor_bytes_and_receives_fresh_local_metadata() {
    let source_fixture = Fixture::new();
    source_fixture.request();
    let source = Arc::new(Store::open(source_fixture.paths()).unwrap());
    let document = produce_notebook(source.clone());
    let target_fixture = Fixture::new();
    let request = target_fixture.request();
    let target = Store::prepare_shared_notebook(target_fixture.paths(), request.clone()).unwrap();
    assert_ne!(source.actor_id, target.actor_id);
    let mut records = BTreeMap::new();
    let mut media = BTreeMap::new();
    let mut namespaces = BTreeMap::new();
    let mut objects = BTreeMap::new();
    for manifest in source.manifests().unwrap() {
        for reference in manifest.records {
            let bytes = source.read_object(&reference).unwrap();
            let envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(envelope.actor_id, source.actor_id);
            namespaces.insert(reference.sha256.clone(), envelope.namespace);
            objects.insert(reference.sha256.clone(), bytes);
            records.insert(reference.sha256.clone(), reference);
        }
        for reference in manifest.media {
            objects.insert(
                reference.sha256.clone(),
                source.read_object(&reference).unwrap(),
            );
            media.insert(reference.sha256.clone(), reference);
        }
    }
    let operation = Uuid::new_v4();
    let scope = selection::SelectionScope {
        group: request.group,
        binding_sha256: request.binding_sha256,
        key_sha256: crate::memory::catalog::document_key(document).unwrap(),
    };
    let manifest = Manifest {
        format: FORMAT,
        transaction_id: operation,
        scope: Scope::SelectedRecords,
        records: records.into_values().collect(),
        record_namespaces: namespaces,
        media: media.values().cloned().collect(),
        media_coverage: media
            .keys()
            .map(|sha256| Coverage::Included {
                sha256: sha256.clone(),
            })
            .collect(),
    };
    // This is the real storage portable-byte boundary. It grants no document
    // registration, model continuation or native authority to the imported data.
    target
        .stage_selected_candidate(&manifest, &objects)
        .unwrap();
    let received = target
        .initialize_selected(
            &scope,
            selection::SelectionChange {
                operation,
                accepted_base_sha256: digest(b"synthetic winning commit"),
                selected: manifest,
                retained: vec![],
            },
            objects.clone(),
        )
        .unwrap();
    let generation = received.token.store_generation();
    assert_eq!(received.transaction.operation, operation);
    drop(target);
    let reopened = Store::open(target_fixture.paths()).unwrap();
    let snapshot = reopened
        .selected_snapshot(&scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.token.store_generation(), generation);
    assert_eq!(snapshot.transaction.operation, operation);
    assert!(snapshot
        .selected_records
        .iter()
        .all(|e| e.actor_id == source.actor_id));
    for reference in snapshot
        .transaction
        .selected
        .records
        .iter()
        .chain(&snapshot.transaction.selected.media)
    {
        assert_eq!(
            reopened.read_object(reference).unwrap(),
            objects[&reference.sha256]
        );
    }
}

#[test]
fn actual_notebook_source_plan_and_stage_bytes_survive_fresh_compound_generation() {
    let f = Fixture::new();
    let request = f.request();
    let store = Arc::new(Store::open(f.paths()).unwrap());
    produce_notebook(store.clone());
    let mut originals = BTreeMap::new();
    let mut source3 = 0;
    for manifest in store.manifests().unwrap() {
        for reference in manifest.records.iter().chain(&manifest.media) {
            let bytes = store.read_object(reference).unwrap();
            if let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) {
                if envelope.domain_schema_version == 3 {
                    source3 += 1;
                }
            }
            originals.insert(reference.sha256.clone(), (reference.clone(), bytes));
        }
    }
    assert!(
        source3 >= 5,
        "actual archive/plan and both prepared/generated stages are required"
    );
    drop(store);
    let prepared = Store::prepare_shared_notebook(f.paths(), request).unwrap();
    assert_eq!(prepared.unavailable_commits().unwrap(), 0);
    for (reference, bytes) in originals.values() {
        assert_eq!(prepared.read_object(reference).unwrap(), *bytes);
    }
    drop(prepared);
    let reopened = Store::open(f.paths()).unwrap();
    assert_eq!(reopened.unavailable_commits().unwrap(), 0);
    for (reference, bytes) in originals.values() {
        assert_eq!(reopened.read_object(reference).unwrap(), *bytes);
    }
}

#[test]
fn original_preparation_stages_fresh_generation_and_preserves_ordinary_source() {
    let f = Fixture::new();
    let request = f.request();
    let store = Arc::new(Store::open(f.paths()).unwrap());
    let conversation = Uuid::new_v4();
    crate::conversation::Ledger::new(store.clone())
        .create(conversation, Uuid::new_v4(), 1)
        .unwrap();
    let envelope = store
        .heads(Namespace::Conversation, conversation)
        .unwrap()
        .pop()
        .unwrap();
    let raw = serde_json::to_vec(&envelope).unwrap();
    let original = store.generation(request.source_generation);
    let identity = files::read(&f.paths().data.join("identity.json"), MAX_RECORD as u64).unwrap();
    drop(store);
    let prepared = Arc::new(Store::prepare_shared_notebook(f.paths(), request.clone()).unwrap());
    assert_ne!(f.current(), request.source_generation);
    assert_eq!(prepared.actor_id, request.actor);
    assert_eq!(feedback_generation::version(&original).unwrap(), 4);
    assert_eq!(
        files::read(
            &original.join("objects").join(digest(&raw)),
            MAX_RECORD as u64
        )
        .unwrap(),
        raw
    );
    assert_eq!(
        files::read(
            &prepared
                .generation(f.current())
                .join("objects")
                .join(digest(&raw)),
            MAX_RECORD as u64
        )
        .unwrap(),
        raw
    );
    assert_eq!(
        files::read(&f.paths().data.join("identity.json"), MAX_RECORD as u64).unwrap(),
        identity
    );
    assert_eq!(
        prepared
            .value(Namespace::Conversation, conversation)
            .unwrap()
            .unwrap()
            .revision_id,
        envelope.revision_id
    );
    assert!(prepared.legacy_publisher_lease().is_err());
    drop(prepared);
    drop(Store::open(f.paths()).unwrap());
    let current = f.current();
    drop(Store::prepare_shared_notebook(f.paths(), request).unwrap());
    assert_eq!(f.current(), current);
}

#[test]
fn each_interruption_keeps_old_or_complete_new_current_and_only_original_resumes() {
    for fault in [
        Stage::BeforeTarget,
        Stage::AfterMarkers,
        Stage::AfterObjects,
        Stage::AfterContract,
        Stage::BeforeCurrent,
        Stage::AfterCurrent,
        Stage::AfterCapabilities,
    ] {
        let f = Fixture::new();
        let request = f.request();
        let original_caps =
            files::read(&f.paths().data.join("capabilities.json"), MAX_RECORD as u64).unwrap();
        assert!(
            Store::prepare_shared_notebook_with(f.paths(), request.clone(), |stage| {
                ensure!(stage != fault, "controlled compound interruption");
                Ok(())
            })
            .is_err()
        );
        let after_current = matches!(fault, Stage::AfterCurrent | Stage::AfterCapabilities);
        assert_eq!(f.current() != request.source_generation, after_current);
        if fault == Stage::AfterCurrent {
            assert!(Store::open(f.paths()).is_err());
            assert_eq!(
                files::read(&f.paths().data.join("capabilities.json"), MAX_RECORD as u64).unwrap(),
                original_caps,
                "normal open must not repair capabilities"
            );
        } else {
            drop(Store::open(f.paths()).unwrap());
        }
        let mut other = request.clone();
        other.operation = Uuid::new_v4();
        assert!(Store::prepare_shared_notebook(f.paths(), other).is_err());
        let mut other = request.clone();
        other.group = Uuid::new_v4();
        assert!(Store::prepare_shared_notebook(f.paths(), other).is_err());
        let original: Preparation =
            files::json(&f.paths().data.join(PREPARATION), MAX_RECORD as u64).unwrap();
        drop(Store::prepare_shared_notebook(f.paths(), request).unwrap());
        assert_eq!(f.current(), original.target_generation);
        drop(Store::open(f.paths()).unwrap());
    }
}

#[test]
fn existing_only_and_busy_custody_cannot_create_or_modify_a_store() {
    let f = Fixture::new();
    let fake = Request {
        operation: Uuid::new_v4(),
        actor: Uuid::new_v4(),
        source_generation: Uuid::new_v4(),
        group: Uuid::new_v4(),
        binding_sha256: digest(b"binding"),
        expected_mode: ModeEvidence {
            actor: Uuid::nil(),
            sha256: digest(b"mode"),
        },
    };
    assert!(Store::prepare_shared_notebook(f.paths(), fake).is_err());
    assert!(!f.0.exists());
    let request = f.request();
    assert!(
        matches!(
            Store::open_existing_for_mode(f.paths()),
            Err(maintenance::Refusal::Unsupported)
        ),
        "format4 mode-maintenance refusal must remain unchanged"
    );
    let held = Store::open(f.paths()).unwrap();
    let current = fs::read(f.paths().data.join("CURRENT")).unwrap();
    let error = Store::prepare_shared_notebook(f.paths(), request)
        .err()
        .unwrap();
    assert!(matches!(
        error.downcast_ref::<maintenance::Refusal>(),
        Some(maintenance::Refusal::Busy)
    ));
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
    drop(held);
}

#[test]
fn ready_compound_mode_maintenance_uses_existing_only_custody() {
    let f = Fixture::new();
    let request = f.request();
    drop(Store::prepare_shared_notebook(f.paths(), request).unwrap());
    let store = Store::open_existing_for_mode(f.paths()).unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            sync::mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    drop(store);
    drop(Store::open(f.paths()).unwrap());
}

#[test]
fn ready_compound_never_enters_ordinary_reader_startup_or_fallback() {
    use crate::reader_runtime::{self, StartupDecision};
    let fixture = Fixture::new();
    let request = fixture.request();
    let config = crate::config::Config {
        paths: fixture.paths(),
        ..Default::default()
    };
    let StartupDecision::Ordinary(Some(ordinary)) = reader_runtime::startup(&config) else {
        panic!("format4 ordinary control must remain supported");
    };
    drop(ordinary);
    drop(Store::prepare_shared_notebook(fixture.paths(), request).unwrap());
    let StartupDecision::Report(report) = reader_runtime::startup(&config) else {
        panic!("format5 requires an explicit selected route before credentials");
    };
    assert_eq!(
        serde_json::to_value(report).unwrap()["status"],
        "unsupported"
    );
    let existing = Arc::new(Store::open(fixture.paths()).unwrap());
    assert!(reader_runtime::ordinary_store(&config, Some(existing)).is_err());
}

#[test]
fn legacy_generation_entrypoints_refuse_compound_without_export_or_downgrade() {
    let source_fixture = Fixture::new();
    let source = Store::open(source_fixture.paths()).unwrap();
    let archive = source_fixture.0.join("ordinary-archive");
    source.export(&archive).unwrap();
    drop(source);
    for operation in [
        "ordinary-export",
        "selected-export",
        "ordinary-restore",
        "feedback-prepare",
        "notebook-prepare",
    ] {
        let fixture = Fixture::new();
        let request = fixture.request();
        let mut store = Some(Store::prepare_shared_notebook(fixture.paths(), request).unwrap());
        let root = store.as_ref().unwrap().generation(fixture.current());
        let staged = root.join("objects/.stage-preserved-owner-artifact");
        fs::write(
            &staged,
            b"unsupported maintenance must not clean original staging",
        )
        .unwrap();
        let files = [
            fixture.paths().data.join("CURRENT"),
            fixture.paths().data.join("capabilities.json"),
            root.join("format.json"),
            root.join(MARKER),
            root.join(notebook_generation::MARKER),
            root.join("selection-feature.json"),
            staged,
        ];
        let before: Vec<_> = files.iter().map(|p| fs::read(p).unwrap()).collect();
        let generations = fs::read_dir(fixture.paths().data.join("generations"))
            .unwrap()
            .count();
        let destination = fixture.0.join("unsupported-export");
        let result = match operation {
            "ordinary-export" => store.as_ref().unwrap().export(&destination).map(|_| ()),
            "selected-export" => store
                .as_ref()
                .unwrap()
                .export_selected(&destination)
                .map(|_| ()),
            "ordinary-restore" => store.as_ref().unwrap().restore(&archive),
            "feedback-prepare" => {
                drop(store.take());
                crate::unassociated_feedback::Feedback::prepare(fixture.paths()).map(|_| ())
            }
            "notebook-prepare" => {
                drop(store.take());
                Store::prepare_notebook(fixture.paths()).map(|_| ())
            }
            _ => unreachable!(),
        };
        drop(store);
        assert!(
            result.is_err(),
            "legacy {operation} must not admit compound storage"
        );
        assert!(!destination.exists());
        assert_eq!(
            fs::read_dir(fixture.paths().data.join("generations"))
                .unwrap()
                .count(),
            generations
        );
        for (path, raw) in files.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), raw, "{operation} changed metadata");
        }
        drop(Store::open(fixture.paths()).unwrap());
    }
}

#[test]
fn original_reconciliation_never_repairs_missing_published_feature_markers() {
    for marker in [
        MARKER,
        notebook_generation::MARKER,
        "selection-feature.json",
    ] {
        let f = Fixture::new();
        let request = f.request();
        let store = Store::prepare_shared_notebook(f.paths(), request.clone()).unwrap();
        let root = store.generation(f.current());
        drop(store);
        let capabilities = fs::read(f.paths().data.join("capabilities.json")).unwrap();
        fs::remove_file(root.join(marker)).unwrap();
        assert!(Store::open(f.paths()).is_err());
        assert!(Store::prepare_shared_notebook(f.paths(), request).is_err());
        assert_eq!(fs::read(f.paths().data.join("capabilities.json")).unwrap(),capabilities,
            "a published incomplete feature is corruption, not an invitation to mint new capabilities");
        assert!(!root.join(marker).exists());
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an explicitly built pre-format5 host reader executable"]
fn pre_format5_executable_refuses_compound_generation_without_repair() {
    use std::process::Command;
    let executable = PathBuf::from(
        std::env::var_os("BUDDY_TEST_PRE_FORMAT5_READER")
            .expect("build a pre-format5 host reader and supply its path"),
    );
    assert!(executable.is_file());
    let fixture = Fixture::new();
    let request = fixture.request();
    let config = fixture.0.join("config.json");
    files::atomic_json(&config, &serde_json::json!({"paths":fixture.paths()})).unwrap();
    let inspect = || {
        // This is the early read-only report3 selector, before runtime/model or
        // device construction. The supplied old executable must be trusted.
        let output = Command::new(&executable)
            .env_clear()
            .env("REMARKABLE_BUDDIES_CONFIG", &config)
            .arg("--inspect-owned-data-v3")
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let ordinary = inspect();
    assert_eq!(
        ordinary["status"], "complete",
        "the old reader must first accept its ordinary format4 control"
    );
    assert_eq!(ordinary["current_generation_format"], 4);
    let prepared = Store::prepare_shared_notebook(fixture.paths(), request).unwrap();
    let root = prepared.generation(fixture.current());
    drop(prepared);
    let critical = [
        fixture.paths().data.join("CURRENT"),
        fixture.paths().data.join("identity.json"),
        fixture.paths().data.join("capabilities.json"),
        fixture.paths().data.join(PREPARATION),
        root.join("format.json"),
        root.join(MARKER),
        root.join(notebook_generation::MARKER),
        root.join("selection-feature.json"),
    ];
    let before: Vec<_> = critical.iter().map(|p| fs::read(p).unwrap()).collect();
    let refused = inspect();
    assert_eq!(refused["status"], "unsupported");
    assert_eq!(refused["domain_accounting_complete"], false);
    assert!(refused["issues"]["unsupported-generation"]
        .as_str()
        .and_then(|count| count.parse::<u64>().ok())
        .is_some_and(|n| n > 0));
    for (path, bytes) in critical.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[cfg(unix)]
#[test]
fn abrupt_preparation_death_releases_custody_and_original_operation_recovers() {
    use std::process::Command;
    for boundary in [
        "after-markers",
        "before-current",
        "after-current",
        "after-capabilities",
    ] {
        let f = Fixture::new();
        let request = f.request();
        files::atomic_json(&f.0.join("request.json"), &request).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::compound_generation::tests::compound_preparation_process_death_child",
                "--ignored",
            ])
            .env("BUDDY_COMPOUND_CRASH_FIXTURE", &f.0)
            .env("BUDDY_COMPOUND_CRASH_BOUNDARY", boundary)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(37),
            "child must reach the actual requested publication boundary"
        );
        if boundary == "after-current" {
            assert!(Store::open(f.paths()).is_err());
        } else {
            drop(Store::open(f.paths()).unwrap());
        }
        let original: Preparation =
            files::json(&f.paths().data.join(PREPARATION), MAX_RECORD as u64).unwrap();
        drop(Store::prepare_shared_notebook(f.paths(), request).unwrap());
        assert_eq!(f.current(), original.target_generation);
        drop(Store::open(f.paths()).unwrap());
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an explicitly built pre-portable format5 host reader"]
fn pre_portable_format5_reader_accepts_its_control_and_refuses_new_contract_without_mutation() {
    use std::process::Command;
    let executable = PathBuf::from(
        std::env::var_os("BUDDY_TEST_PRE_PORTABLE_FORMAT5_READER")
            .expect("supply a trusted pre-portable format5 host reader"),
    );
    assert!(executable.is_file());
    let old_contract = include_bytes!("fixtures/pre-portable-contract-v4.json");
    assert_eq!(
        digest(old_contract),
        "14417dc53bf35ead00701737256f4f11dc28fc49645f2c7e4f3db3c0dfef24aa"
    );
    assert_ne!(digest(old_contract), digest(CONTRACT.as_bytes()));
    for old_control in [true, false] {
        let fixture = Fixture::new();
        let request = fixture.request();
        drop(Store::prepare_shared_notebook(fixture.paths(), request).unwrap());
        let root = fixture
            .paths()
            .data
            .join("generations")
            .join(fixture.current().to_string());
        if old_control {
            // Build the exact prior draft contract/feature combination in a
            // disposable store. It contains no new namespace or user data.
            // The unmodified older executable supplies the positive control.
            let mut feature: Preparation =
                files::json(&root.join(MARKER), MAX_RECORD as u64).unwrap();
            feature.contract_sha256 = digest(old_contract);
            files::atomic_json(&root.join(MARKER), &feature).unwrap();
            files::atomic_json(&fixture.paths().data.join(PREPARATION), &feature).unwrap();
            files::atomic(&fixture.paths().data.join("contract-v4.json"), old_contract).unwrap();
        }
        let config = fixture.0.join("config.json");
        files::atomic_json(&config, &serde_json::json!({"paths":fixture.paths()})).unwrap();
        let critical = [
            fixture.paths().data.join("CURRENT"),
            fixture.paths().data.join("identity.json"),
            fixture.paths().data.join("capabilities.json"),
            fixture.paths().data.join("contract-v4.json"),
            fixture.paths().data.join(PREPARATION),
            root.join("format.json"),
            root.join(MARKER),
            root.join(notebook_generation::MARKER),
            root.join("selection-feature.json"),
        ];
        let before: Vec<_> = critical
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect();
        let output = Command::new(&executable)
            .env_clear()
            .env("REMARKABLE_BUDDIES_CONFIG", &config)
            .args(["--sync-mode-operation", "inspect"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["status"],
            if old_control { "ok" } else { "unsupported" }
        );
        assert_eq!(report["activation_performed"], false);
        assert!(!fixture.paths().data.join("sync/mode.json").exists());
        assert!(!fixture.paths().data.join("sync/state.json").exists());
        for (path, bytes) in critical.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        if !old_control {
            drop(Store::open_existing_for_mode(fixture.paths()).unwrap());
        }
    }
}

#[cfg(unix)]
#[test]
#[ignore = "owned subprocess crash witness"]
fn compound_preparation_process_death_child() {
    let root = PathBuf::from(std::env::var_os("BUDDY_COMPOUND_CRASH_FIXTURE").unwrap());
    assert!(root.starts_with(std::env::temp_dir()));
    let request: Request = files::json(&root.join("request.json"), MAX_RECORD as u64).unwrap();
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    let boundary = match std::env::var("BUDDY_COMPOUND_CRASH_BOUNDARY")
        .unwrap()
        .as_str()
    {
        "after-markers" => Stage::AfterMarkers,
        "before-current" => Stage::BeforeCurrent,
        "after-current" => Stage::AfterCurrent,
        "after-capabilities" => Stage::AfterCapabilities,
        _ => panic!("unsupported crash boundary"),
    };
    let _ = Store::prepare_shared_notebook_with(paths, request, |stage| {
        if stage == boundary {
            std::process::exit(37);
        }
        Ok(())
    });
    panic!("crash boundary not reached");
}

#[test]
fn stale_source_and_wrong_original_mode_refuse_preparation_without_retargeting() {
    for changed_mode in [false, true] {
        let f = Fixture::new();
        let request = f.request();
        assert!(
            Store::prepare_shared_notebook_with(f.paths(), request.clone(), |stage| {
                ensure!(stage != Stage::BeforeCurrent, "controlled interruption");
                Ok(())
            })
            .is_err()
        );
        let store = Arc::new(Store::open(f.paths()).unwrap());
        if changed_mode {
            assert!(
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        sync::mode::SyncModeChoice::LocalOnly,
                    )
                    .is_err(),
                "ordinary format4 mode refusal remains unchanged"
            );
            // A changed original conditional request is independently refused.
        } else {
            crate::conversation::Ledger::new(store.clone())
                .create(Uuid::new_v4(), Uuid::new_v4(), 2)
                .unwrap();
        }
        drop(store);
        let mut resumed = request.clone();
        if changed_mode {
            resumed.expected_mode.sha256 = digest(b"different original mode");
        }
        assert!(Store::prepare_shared_notebook(f.paths(), resumed).is_err());
        assert_eq!(f.current(), request.source_generation);
    }
}

#[test]
fn compound_store_serializes_local_operation_reuse_across_real_memory_producers() {
    check_local_operation_reuse(false);
}

#[test]
fn promoted_target_store_serializes_local_operation_reuse_across_real_memory_producers() {
    check_local_operation_reuse(true);
}

fn check_local_operation_reuse(promote_targets: bool) {
    use crate::memory::{
        self, catalog,
        selected::{InitializeAggregate, SelectedMemory},
    };
    use std::{sync::Barrier, thread};
    let fixture = Fixture::new();
    let request = fixture.request();
    let mut store = Store::prepare_shared_notebook(fixture.paths(), request.clone()).unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            sync::mode::SyncModeChoice::SharedGroup {
                group: request.group,
                binding_sha256: request.binding_sha256.clone(),
                root_id: "synthetic-root".into(),
            },
        )
        .unwrap();
    let generation = fixture.current();
    if promote_targets {
        target_generation::prepare(
            &mut store,
            &digest(b"synthetic target configuration"),
            target_generation::BaseKind::CompoundSelected,
            |_| Ok(()),
        )
        .unwrap();
        target_generation::validate_ready(&store.paths, store.actor_id, generation).unwrap();
    }
    assert_eq!(
        feedback_generation::version(&store.generation(generation)).unwrap(),
        if promote_targets { 6 } else { 5 }
    );
    assert_eq!(
        target_generation::base_version(&store.generation(generation)).unwrap(),
        5
    );
    let store = Arc::new(store);
    let operation = Uuid::new_v4();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            let request = request.clone();
            thread::spawn(move || {
                let profile = Uuid::new_v4();
                let producer = SelectedMemory::new(
                    store,
                    catalog::Identity {
                        group: request.group,
                        profile,
                        binding_sha256: request.binding_sha256,
                    },
                )
                .unwrap();
                let fact = memory::Fact {
                    key: memory::Key {
                        profile,
                        scope: memory::Scope::Global,
                        fact: Uuid::new_v4(),
                    },
                    claim: "atomic operation custody".into(),
                    value: "explicit confirmed value".into(),
                    subject: "units".into(),
                    tags: vec![],
                    provenance: memory::Provenance {
                        basis: memory::Basis::UserConfirmed,
                        description: "synthetic compound producer".into(),
                        conversation: None,
                        turn: None,
                        source: None,
                    },
                    confidence_basis_points: 9000,
                    observed_ms: 1,
                    expires_ms: None,
                };
                barrier.wait();
                let result = producer.initialize_aggregate(InitializeAggregate {
                    operation,
                    accepted_base_sha256: digest(b"synthetic agreed genesis"),
                    fact: fact.clone(),
                });
                if result.is_ok() {
                    assert_eq!(
                        producer
                            .initialize_aggregate(InitializeAggregate {
                                operation,
                                accepted_base_sha256: digest(b"synthetic agreed genesis"),
                                fact
                            })
                            .unwrap()
                            .acknowledgment,
                        "replayed"
                    );
                }
                result
            })
        })
        .collect();
    barrier.wait();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|o| o.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| o.as_ref().is_err_and(|e| e
                .downcast_ref::<selection::LocalOperationConflict>()
                .is_some()))
            .count(),
        1,
        "the actual Store publication mutex must reject reuse across independent scope gates"
    );
    // Discovery must preserve inherited selected history after target promotion,
    // with the same bounded reads and scope filters as the unpromoted store.
    let mut reads = selection::SelectedReadBudget::new(
        MAX_ITEMS as u64,
        MAX_ITEMS as u64,
        MAX_METADATA as u64,
        0,
    );
    let scopes = store
        .selected_scope_candidates(
            request.group,
            &request.binding_sha256,
            Namespace::Memory,
            &mut reads,
        )
        .unwrap();
    assert!(!scopes.is_empty(), "accepted memory scope disappeared");
    assert!(scopes.iter().all(
        |scope| scope.group == request.group && scope.binding_sha256 == request.binding_sha256
    ));
    assert!(store
        .selected_scope_candidates(
            Uuid::new_v4(),
            &request.binding_sha256,
            Namespace::Memory,
            &mut reads,
        )
        .unwrap()
        .is_empty());
    let mut exhausted = selection::SelectedReadBudget::new(0, 0, 0, 0);
    assert!(store
        .selected_scope_candidates(
            request.group,
            &request.binding_sha256,
            Namespace::Memory,
            &mut exhausted,
        )
        .is_err());
    assert!(exhausted.exhausted());
}
