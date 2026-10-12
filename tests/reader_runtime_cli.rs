//! Actual host executable paths; fixtures convey no native authority.
use remarkable_reader_buddy::{
    conversation::{Record, Root, SCHEMA},
    reader_runtime::{self, Operation},
    storage::{
        self,
        selection::{SelectionChange, SelectionScope},
        Envelope, Kind, Manifest, Namespace, ObjectRef, Scope, Store, StorePaths, Uuid, FORMAT,
    },
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    process::Command,
};

struct Fixture {
    root: PathBuf,
    conversation: Uuid,
    scope: SelectionScope,
}
impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: std::env::temp_dir().join(format!("buddy-reader-runtime-{}", Uuid::new_v4())),
            conversation: Uuid::new_v4(),
            scope: SelectionScope {
                group: Uuid::new_v4(),
                key_sha256: storage::digest(b"host conversation"),
                binding_sha256: storage::digest(b"host binding"),
            },
        };
        fs::create_dir_all(&fixture.root).unwrap();
        let store = Store::open(fixture.paths()).unwrap();
        let envelope = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: SCHEMA,
            record_id: fixture.conversation,
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::Root(Root {
                id: fixture.conversation,
                next_sequence: 0,
                binding: None,
                created_ms: 1,
                updated_ms: 1,
            }))
            .unwrap(),
            media_descriptors: vec![],
        };
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let sha256 = storage::digest(&bytes);
        store
            .initialize_selected(
                &fixture.scope,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: storage::digest(b"fixture initial base"),
                    retained: vec![],
                    selected: Manifest {
                        format: FORMAT,
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::SelectedRecords,
                        records: vec![ObjectRef {
                            sha256: sha256.clone(),
                            bytes: bytes.len() as u64,
                        }],
                        record_namespaces: BTreeMap::from([(
                            sha256.clone(),
                            Namespace::Conversation,
                        )]),
                        media: vec![],
                        media_coverage: vec![],
                    },
                },
                BTreeMap::from([(sha256, bytes)]),
            )
            .unwrap();
        drop(store);
        fs::write(
            fixture.root.join("config.json"),
            serde_json::to_vec(&json!({
                "paths": fixture.paths(), "reader": {"selection": fixture.scope}
            }))
            .unwrap(),
        )
        .unwrap();
        fixture
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.root.join("data"),
            cache: self.root.join("cache"),
            credentials: self.root.join("credentials"),
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .current_dir(&self.root)
            .env("REMARKABLE_BUDDIES_CONFIG", self.root.join("config.json"))
            .env("OPENAI_API_KEY", "private-provider-sentinel")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("READER_BUDDY_DEBUG_DUMP", "invalid-unused-setting");
        command
    }
    fn report(&self, operation: &str, input: Value) -> Value {
        fs::write(
            self.root.join("input.json"),
            serde_json::to_vec(&input).unwrap(),
        )
        .unwrap();
        let output = self
            .command()
            .args([
                "--conversation-operation",
                operation,
                "--conversation-input",
            ])
            .arg(self.root.join("input.json"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["native_authority"], false);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-provider-sentinel"));
        value
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn actual_selected_startup_refuses_native_before_resolving_provider() {
    let fixture = Fixture::new();
    let output = fixture.command().output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "unsupported");
    assert_eq!(report["data"]["reason"], "no-native-adapter");
    assert_eq!(report["native_authority"], false);
    // The selector introduced by dotenv must route before ordinary startup too.
    fs::write(
        fixture.root.join(".env"),
        format!(
            "REMARKABLE_BUDDIES_CONFIG='{}'\n",
            fixture
                .root
                .join("config.json")
                .to_string_lossy()
                .replace('\\', "/")
        ),
    )
    .unwrap();
    let output = fixture
        .command()
        .env_remove("REMARKABLE_BUDDIES_CONFIG")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "unsupported"
    );
}

#[test]
fn actual_inspection_and_context_do_not_invent_token_counts() {
    let fixture = Fixture::new();
    let inspect = fixture.report("inspect", json!({"conversation": fixture.conversation}));
    assert_eq!(inspect["status"], "complete");
    assert_eq!(inspect["data"]["records"].as_array().unwrap().len(), 1);
    let context = fixture.report(
        "context",
        json!({"conversation": fixture.conversation, "range": null,
        "budget": {"max_text_bytes": 4096, "max_turns": 8, "provider_token_limit": 1000}}),
    );
    assert_eq!(context["status"], "choice-needed");
    assert!(context["data"].is_null());
}

#[test]
fn invalid_input_is_private_and_store_busy_is_distinct() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.report(
            "inspect",
            json!({"conversation": fixture.conversation, "unknown": "private-provider-sentinel"})
        )["status"],
        "invalid-input"
    );
    let held = Store::open(fixture.paths()).unwrap();
    assert_eq!(
        fixture.report("inspect", json!({"conversation": fixture.conversation}))["status"],
        "busy"
    );
    drop(held);
}

#[test]
fn current_inspection_consumes_its_seal_and_rejects_modified_payload() {
    let fixture = Fixture::new();
    let input = fixture.root.join("input.json");
    fs::write(
        &input,
        serde_json::to_vec(&json!({"conversation": fixture.conversation})).unwrap(),
    )
    .unwrap();
    let mut report = reader_runtime::run(
        Operation::Inspect,
        &input,
        Some(fixture.root.join("config.json")),
    );
    let mut first = vec![];
    report.write_to(&mut first).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&first).unwrap()["status"],
        "complete"
    );
    let mut second = vec![];
    report.write_to(&mut second).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&second).unwrap()["status"],
        "unavailable"
    );
    drop(report);
    let mut report = reader_runtime::run(
        Operation::Inspect,
        &input,
        Some(fixture.root.join("config.json")),
    );
    report.data = Some(json!({"forged": true}));
    let mut output = vec![];
    report.write_to(&mut output).unwrap();
    let output: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(output["status"], "unavailable");
    assert!(output["data"].is_null());
}

#[test]
fn partial_output_never_attempts_a_second_report_and_retains_store_lease() {
    use std::io::{self, Write};
    struct FailingWriter {
        bytes: Vec<u8>,
        calls: usize,
    }
    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                self.bytes.extend_from_slice(&bytes[..7]);
                Ok(7)
            } else {
                Err(io::Error::other("synthetic stdout failure"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let fixture = Fixture::new();
    let input = fixture.root.join("input.json");
    fs::write(
        &input,
        serde_json::to_vec(&json!({"conversation": fixture.conversation})).unwrap(),
    )
    .unwrap();
    let mut report = reader_runtime::run(
        Operation::Inspect,
        &input,
        Some(fixture.root.join("config.json")),
    );
    assert!(
        Store::open(fixture.paths()).is_err(),
        "returned current report must retain lease"
    );
    let mut writer = FailingWriter {
        bytes: vec![],
        calls: 0,
    };
    assert!(report.write_to(&mut writer).is_err());
    assert_eq!(
        writer.calls, 2,
        "write_all prefix and error only; no fallback report"
    );
    assert_eq!(writer.bytes, b"{\"repor");
    drop(report);
    assert!(
        Store::open(fixture.paths()).is_ok(),
        "dropping report releases lease"
    );
}

#[test]
fn oversized_input_and_invalid_range_refuse_without_private_output() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.report(
            "context",
            json!({"conversation": fixture.conversation, "range": {"start": 4, "end_exclusive": 4},
        "budget": {"max_text_bytes": 4096, "max_turns": 8, "provider_token_limit": 1000}})
        )["status"],
        "invalid-input"
    );
    fs::write(
        fixture.root.join("input.json"),
        vec![b'x'; storage::MAX_RECORD + 1],
    )
    .unwrap();
    let output = fixture
        .command()
        .args([
            "--conversation-operation",
            "inspect",
            "--conversation-input",
        ])
        .arg(fixture.root.join("input.json"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.len() < 512 && output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "invalid-input"
    );
}

#[test]
fn selected_inspection_never_fills_missing_conversation_from_ordinary_or_retained_heads() {
    use remarkable_reader_buddy::conversation::Ledger;
    use std::sync::Arc;
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let ledger = Ledger::new(store.clone());
    let outside = Uuid::new_v4();
    ledger.create(outside, Uuid::new_v4(), 1).unwrap();
    let mut retained = store
        .manifests()
        .unwrap()
        .into_iter()
        .find(|manifest| manifest.scope == Scope::LocalTransaction)
        .unwrap();
    retained.scope = Scope::SelectedRecords;
    let snapshot = store
        .selected_snapshot(&fixture.scope, storage::MAX_ITEMS)
        .unwrap()
        .unwrap();
    let mut selected = snapshot.transaction.selected.clone();
    selected.transaction_id = Uuid::new_v4();
    store
        .activate_selected(
            &snapshot.token,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"retained history fixture"),
                selected,
                retained: vec![retained],
            },
            BTreeMap::new(),
        )
        .unwrap();
    drop(snapshot);
    drop(ledger);
    drop(store);
    let absent = fixture.report("inspect", json!({"conversation": outside}));
    assert_eq!(absent["status"], "unavailable");
    assert!(absent["data"].is_null());
    assert_eq!(
        fixture.report("inspect", json!({"conversation": fixture.conversation}))["status"],
        "complete"
    );
}

#[test]
fn actual_recovery_command_distinguishes_absent_operation_and_foreign_input_before_store_open() {
    use remarkable_reader_buddy::conversation::{
        IntentSelectionEvidence, PendingIntentRequest, SourceObservation,
    };
    let fixture = Fixture::new();
    let request = PendingIntentRequest {
        operation: Uuid::new_v4(),
        conversation: fixture.conversation,
        turn: Uuid::new_v4(),
        updated_ms: 1,
        selection: IntentSelectionEvidence {
            group: fixture.scope.group,
            key_sha256: fixture.scope.key_sha256.clone(),
            binding_sha256: fixture.scope.binding_sha256.clone(),
            store_generation: Uuid::new_v4(),
            aggregate_generation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"historical fixture base"),
            selection_sha256: storage::digest(b"historical fixture selection"),
        },
        source: SourceObservation {
            document: Uuid::new_v4(),
            page: Uuid::new_v4(),
            session: "synthetic fixture".into(),
            visit: "synthetic fixture".into(),
            content_revision: None,
            order_revision: None,
            capability_revision: "synthetic-only".into(),
            evidence_procedure: "no native qualification".into(),
        },
        evidence: vec![],
        media: vec![],
    };
    let absent = fixture.report("recover-pending", json!({"request": request}));
    assert_eq!(absent["status"], "not-found");
    assert!(absent["data"].is_null());
    let held = Store::open(fixture.paths()).unwrap();
    let mut foreign = request.clone();
    foreign.selection.group = Uuid::new_v4();
    assert_eq!(
        fixture.report("recover-pending", json!({"request": foreign}))["status"],
        "invalid-input",
        "pure foreign-scope validation must run before a busy Store can be opened"
    );
    assert_eq!(
        fixture.report("recover-pending", json!({"request": request}))["status"],
        "busy"
    );
    drop(held);
}
