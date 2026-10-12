//! Actual one-shot binary operations on isolated host Stores. No tablet/provider/service access.
use remarkable_reader_buddy::{
    memory::*,
    storage::{self, Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::PathBuf, process::Command, sync::Arc};

struct Fixture {
    root: PathBuf,
    profile: Uuid,
}
impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: std::env::temp_dir().join(format!("buddy-memory-cli-{}", Uuid::new_v4())),
            profile: Uuid::new_v4(),
        };
        fs::create_dir_all(&fixture.root).unwrap();
        fs::write(fixture.root.join("config.json"), serde_json::to_vec(&json!({"paths":fixture.paths(),"memory":{"profile":fixture.profile,"enabled":false}})).unwrap()).unwrap();
        fixture
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.root.join("data"),
            cache: self.root.join("cache"),
            credentials: self.root.join("credentials"),
        }
    }
    fn fact(&self) -> Fact {
        Fact {
            key: Key {
                profile: self.profile,
                scope: Scope::Global,
                fact: Uuid::new_v4(),
            },
            claim: "physics units".into(),
            value: "Use SI gravity units.\nα and β stay Unicode.".into(),
            subject: "physics".into(),
            tags: vec![],
            provenance: Provenance {
                basis: Basis::UserConfirmed,
                description: "Explicit host fixture assertion".into(),
                conversation: None,
                turn: None,
                source: None,
            },
            confidence_basis_points: 8000,
            observed_ms: 10,
            expires_ms: None,
        }
    }
    fn command(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .current_dir(&self.root)
            .args(["--memory-operation", operation, "--memory-input"])
            .arg(self.root.join("input.json"))
            .env("REMARKABLE_BUDDIES_CONFIG", self.root.join("config.json"))
            .env("OPENAI_API_KEY", "private-provider-sentinel")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("RUST_LOG", "trace")
            .env("READER_BUDDY_DEBUG_DUMP", "invalid-unused-setting");
        command
    }
    fn run(&self, operation: &str, input: &impl serde::Serialize) -> Value {
        fs::write(
            self.root.join("input.json"),
            serde_json::to_vec(input).unwrap(),
        )
        .unwrap();
        let output = self.command(operation).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["report_version"], 1);
        report
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn selected_config(fixture: &Fixture) -> catalog::Identity {
    let identity = catalog::Identity {
        group: Uuid::new_v4(),
        profile: fixture.profile,
        binding_sha256: storage::digest(b"explicit host fixture binding, not a cloud account"),
    };
    fs::write(fixture.root.join("config.json"), serde_json::to_vec(&json!({
        "paths": fixture.paths(), "memory": {"enabled":false,"profile":fixture.profile,"selection":identity}
    })).unwrap()).unwrap();
    identity
}

#[test]
fn actual_selected_cli_explicit_empty_catalog_and_default_local_setup_refusal() {
    let local = Fixture::new();
    let input = json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"local setup"),"registrations":[]});
    assert_eq!(
        local.run("initialize-selection", &input)["status"],
        "unsupported"
    );
    assert!(!local.paths().data.exists());
    let fixture = Fixture::new();
    selected_config(&fixture);
    let first = fixture.run("initialize-selection", &input);
    assert_eq!(first["status"], "ok");
    assert_eq!(first["data"]["acknowledgment"], "published");
    let retry = fixture.run("initialize-selection", &input);
    assert_eq!(retry["data"]["transaction"], first["data"]["transaction"]);
    assert_eq!(retry["data"]["acknowledgment"], "replayed");
    let query = fixture.run("query", &Query::new("gravity".into(), 100));
    assert_eq!(query["data"]["query_status"], "complete-empty");
    let missing = fixture.fact();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: Default::default(),
                fact: missing
            }
        )["status"],
        "unavailable"
    );
}

#[test]
fn actual_selected_cli_orphans_registration_adoption_retry_and_forgetting_preserve_other_keys() {
    let fixture = Fixture::new();
    selected_config(&fixture);
    let first = fixture.fact();
    let create = json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"accepted host descriptor"),"fact":first});
    let initialized = fixture.run("initialize-aggregate", &create);
    assert_eq!(initialized["status"], "ok");
    assert_eq!(
        fixture.run("query", &Query::new("gravity".into(), 100))["status"],
        "unavailable"
    );
    assert_eq!(
        fixture.run("initialize-aggregate", &create)["data"]["acknowledgment"],
        "replayed"
    );
    let root = json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"catalog descriptor"),
        "registrations":[initialized["data"]["registration"].clone()]});
    assert_eq!(fixture.run("initialize-selection", &root)["status"], "ok");
    let mut second = fixture.fact();
    second.claim = "gravity convention".into();
    second.value = "Use kelvin in gravity calculations".into();
    let orphan = fixture.run("initialize-aggregate", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"second aggregate"),"fact":second}));
    assert_eq!(orphan["status"], "ok");
    assert_eq!(
        fixture.run("query", &Query::new("gravity".into(), 100))["data"]["context"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let register =
        json!({"operation":Uuid::new_v4(),"registration":orphan["data"]["registration"].clone()});
    let published = fixture.run("register-selection", &register);
    assert_eq!(published["data"]["acknowledgment"], "published");
    let retry = fixture.run("register-selection", &register);
    assert_eq!(retry["data"]["acknowledgment"], "replayed");
    assert_eq!(
        retry["data"]["transaction"],
        published["data"]["transaction"]
    );
    let adopted = fixture.run(
        "register-selection",
        &json!({"operation":Uuid::new_v4(),"registration":orphan["data"]["registration"].clone()}),
    );
    assert_eq!(adopted["data"]["acknowledgment"], "adopted");
    assert!(adopted["data"]["transaction"].is_null());
    let inspected = fixture.run("inspect", &first.key);
    let parents: BTreeSet<_> = inspected["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Uuid::parse_str(e["revision_id"].as_str().unwrap()).unwrap())
        .collect();
    let mut correction = first.clone();
    correction.value = "Corrected SI gravity units".into();
    let update = Mutation {
        operation: Uuid::new_v4(),
        expected: parents,
        fact: correction,
    };
    let receipt = fixture.run("put", &update);
    assert_eq!(receipt["status"], "ok");
    let current = fixture.run("inspect", &first.key);
    let parents = current["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Uuid::parse_str(e["revision_id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        fixture.run(
            "forget",
            &Forget {
                operation: Uuid::new_v4(),
                expected: parents,
                key: first.key.clone()
            }
        )["status"],
        "ok"
    );
    assert_eq!(
        fixture.run("put", &update)["data"]["transaction"],
        receipt["data"]["transaction"]
    );
    let remaining = fixture.run("query", &Query::new("gravity".into(), 100));
    assert_eq!(
        remaining["data"]["context"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        remaining["data"]["context"]["facts"][0]["fact"]["key"]["fact"],
        json!(second.key.fact)
    );
    let mut collided = update.clone();
    collided.fact.value = "Changed replay payload".into();
    assert_eq!(fixture.run("put", &collided)["status"], "unavailable");
    assert_eq!(
        fixture.run("query", &Query::new("gravity".into(), 100))["data"]["context"],
        remaining["data"]["context"]
    );
}

#[test]
fn actual_selected_cli_invalid_setup_refuses_before_store_creation() {
    let fixture = Fixture::new();
    selected_config(&fixture);
    let bad =
        json!({"operation":Uuid::nil(),"accepted_base_sha256":"NOT-A-DIGEST","registrations":[]});
    assert_eq!(
        fixture.run("initialize-selection", &bad)["status"],
        "invalid-input"
    );
    assert!(!fixture.paths().data.exists());
    assert!(!fixture.paths().cache.exists());
    let mut fact = fixture.fact();
    fact.key.profile = Uuid::new_v4();
    assert_eq!(fixture.run("initialize-aggregate", &json!({"operation":Uuid::new_v4(),"accepted_base_sha256":storage::digest(b"base"),"fact":fact}))["status"], "invalid-input");
    assert!(!fixture.paths().data.exists());
}

#[test]
fn actual_cli_put_query_inspect_cas_forget_and_exact_retry_are_offline() {
    let fixture = Fixture::new();
    let fact = fixture.fact();
    let mutation = Mutation {
        operation: Uuid::new_v4(),
        expected: BTreeSet::new(),
        fact: fact.clone(),
    };
    let first = fixture.run("put", &mutation);
    assert_eq!(first["status"], "ok");
    assert_eq!(fixture.run("put", &mutation)["data"], first["data"]);
    let queried = fixture.run("query", &Query::new("physics gravity units".into(), 100));
    assert_eq!(queried["status"], "ok");
    assert_eq!(
        queried["data"]["context"]["facts"][0]["fact"]["value"],
        fact.value
    );
    let inspection = fixture.run("inspect", &fact.key);
    let revision: Uuid =
        serde_json::from_value(inspection["data"][0]["revision_id"].clone()).unwrap();
    assert_eq!(inspection["status"], "ok");
    let stale = Mutation {
        operation: Uuid::new_v4(),
        ..mutation.clone()
    };
    let conflict = fixture.run("put", &stale);
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["data"]["current_heads"], json!([revision]));
    let forgetting = Forget {
        operation: Uuid::new_v4(),
        expected: BTreeSet::from([revision]),
        key: fact.key.clone(),
    };
    let forgotten = fixture.run("forget", &forgetting);
    assert_eq!(forgotten["status"], "ok");
    assert_eq!(
        fixture.run("forget", &forgetting)["data"],
        forgotten["data"]
    );
    let query = fixture.run("query", &Query::new("physics gravity units".into(), 100));
    assert_eq!(query["data"]["query_status"], "complete-empty");
    assert_eq!(
        fixture.run("inspect", &fact.key)["data"][0]["kind"],
        "tombstone"
    );
    assert!(!fs::read_to_string(fixture.root.join("config.json"))
        .unwrap()
        .contains("private-provider"));
}

#[test]
fn missing_and_partial_state_never_initialize_from_read_forget_or_put() {
    let fixture = Fixture::new();
    let fact = fixture.fact();
    assert_eq!(fixture.run("inspect", &fact.key)["status"], "unavailable");
    assert_eq!(
        fixture.run("query", &Query::new("physics".into(), 100))["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.exists());
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::from([Uuid::new_v4()]),
                fact: fact.clone()
            }
        )["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.exists());
    let forget = Forget {
        operation: Uuid::new_v4(),
        expected: BTreeSet::from([Uuid::new_v4()]),
        key: fact.key.clone(),
    };
    assert_eq!(fixture.run("forget", &forget)["status"], "unavailable");
    assert!(!fixture.paths().data.exists());
    fs::create_dir_all(&fixture.paths().data).unwrap();
    fs::write(
        fixture.paths().data.join("unowned-sentinel"),
        b"private original",
    )
    .unwrap();
    let put = Mutation {
        operation: Uuid::new_v4(),
        expected: BTreeSet::new(),
        fact,
    };
    assert_eq!(fixture.run("put", &put)["status"], "unavailable");
    assert!(!fixture.paths().data.join("identity.json").exists());
    assert_eq!(
        fs::read(fixture.paths().data.join("unowned-sentinel")).unwrap(),
        b"private original"
    );
}

#[test]
fn pure_invalid_input_and_profile_checks_do_not_create_store_or_echo_private_values() {
    let fixture = Fixture::new();
    let mut fact = fixture.fact();
    fact.key.profile = Uuid::new_v4();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::nil(),
                expected: BTreeSet::new(),
                fact: fixture.fact()
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fixture.run(
            "forget",
            &Forget {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                key: fixture.fact().key
            }
        )["status"],
        "invalid-input"
    );
    for bytes in [
        b"private parser value".to_vec(),
        vec![b'x'; storage::MAX_RECORD + 1],
    ] {
        fs::write(fixture.root.join("input.json"), bytes).unwrap();
        let output = fixture.command("put").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains("private parser value"));
        assert!(!stdout.contains("private-provider"));
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).unwrap()["status"],
            "invalid-input"
        );
    }
    assert!(!fixture.paths().data.exists());
}

#[test]
fn busy_store_returns_report_without_service_stop_or_file_mutation() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    let identity = fs::read(fixture.paths().data.join("identity.json")).unwrap();
    let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    let report = fixture.run(
        "put",
        &Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: fixture.fact(),
        },
    );
    assert_eq!(report["status"], "busy");
    assert!(store.manifests().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.paths().data.join("identity.json")).unwrap(),
        identity
    );
    assert_eq!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        current
    );
}

#[test]
fn missing_marker_in_partly_owned_store_refuses_put_without_recovery() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    drop(store);
    fs::remove_file(fixture.paths().data.join("CURRENT")).unwrap();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact: fixture.fact()
            }
        )["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.join("CURRENT").exists());
}

#[test]
fn poison_dotenv_and_execution_overrides_cannot_enter_provider_or_runtime() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join(".env"),
        format!(
            "REMARKABLE_BUDDIES_CONFIG={}\nOPENAI_API_KEY=private-dotenv\n",
            fixture.root.join("config.json").display()
        ),
    )
    .unwrap();
    fs::write(
        fixture.root.join("input.json"),
        serde_json::to_vec(&Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: fixture.fact(),
        })
        .unwrap(),
    )
    .unwrap();
    let output = fixture
        .command("put")
        .env_remove("REMARKABLE_BUDDIES_CONFIG")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "unavailable"
    );
    assert!(!fixture.paths().data.exists());
    let output = fixture
        .command("put")
        .args(["--api-key", "private-cli-key"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-cli-key"));
    assert!(!fixture.paths().data.exists());
}

#[test]
fn selected_store_is_explicitly_unsupported_even_with_sync_disabled() {
    use storage::selection::{SelectionChange, SelectionScope};
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let memory = LocalMemory::new_local(store.clone(), fixture.profile).unwrap();
    let fact = fixture.fact();
    memory
        .put(Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: fact.clone(),
        })
        .unwrap();
    drop(memory);
    let mut selected = store.manifests().unwrap().remove(0);
    selected.scope = storage::Scope::SelectedRecords;
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: storage::digest(b"fixture-scope"),
        binding_sha256: storage::digest(b"fixture-binding"),
    };
    store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"fixture-base"),
                selected,
                retained: vec![],
            },
            Default::default(),
        )
        .unwrap();
    drop(store);
    assert_eq!(fixture.run("inspect", &fact.key)["status"], "unsupported");
}

#[test]
fn partial_withheld_query_and_unavailable_recovery_never_become_healthy_empty() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let memory = LocalMemory::new_local(store.clone(), fixture.profile).unwrap();
    let first = fixture.fact();
    let mut second = first.clone();
    second.key.fact = Uuid::new_v4();
    second.value = "Conflicting cgs units".into();
    for value in [first.clone(), second] {
        memory
            .put(Mutation {
                operation: Uuid::new_v4(),
                expected: BTreeSet::new(),
                fact: value,
            })
            .unwrap();
    }
    drop(memory);
    drop(store);
    let report = fixture.run("query", &Query::new("physics units".into(), 100));
    assert_eq!(report["status"], "partial");
    assert_eq!(report["incomplete"], true);
    assert!(report["data"]["context"]["facts"]
        .as_array()
        .unwrap()
        .is_empty());
    let generations = fs::read_dir(fixture.paths().data.join("generations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let commit = fs::read_dir(generations.join("commits"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(commit, b"private corrupt commit contents").unwrap();
    let report = fixture.run("query", &Query::new("physics units".into(), 100));
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["incomplete"], true);
}

#[test]
fn entire_response_cap_includes_newline_and_never_returns_partial_refresh_data() {
    use remarkable_reader_buddy::memory::maintenance::{
        Operation, Report, ReportStatus, MAX_RESPONSE,
    };
    let report = Report {
        report_version: 1,
        operation: Operation::Inspect,
        status: ReportStatus::Ok,
        incomplete: false,
        data: Some(Value::String("private-value".repeat(MAX_RESPONSE / 10))),
    };
    let bytes = report.encoded().unwrap();
    assert!(bytes.len() <= MAX_RESPONSE);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let limited: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(limited["status"], "unavailable");
    assert_eq!(limited["incomplete"], true);
    assert!(limited["data"].is_null());
}

#[test]
fn self_parent_put_and_forget_are_invalid_before_store_staging_cleanup() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    drop(store);
    let stage = fixture
        .paths()
        .data
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&stage, b"private interrupted staging evidence").unwrap();
    let operation = Uuid::new_v4();
    let revision = Uuid::new_v5(&operation, b"remarkable-buddies:subject-memory:revision:v1");
    let fact = fixture.fact();
    assert_eq!(
        fixture.run(
            "put",
            &Mutation {
                operation,
                expected: BTreeSet::from([revision]),
                fact: fact.clone()
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"private interrupted staging evidence"
    );
    assert_eq!(
        fixture.run(
            "forget",
            &Forget {
                operation,
                expected: BTreeSet::from([revision]),
                key: fact.key
            }
        )["status"],
        "invalid-input"
    );
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"private interrupted staging evidence"
    );
}

#[cfg(unix)]
#[test]
fn fifo_lock_is_refused_before_a_blocking_open_in_actual_cli() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.paths()).unwrap();
    drop(store);
    let lock = fixture.paths().data.join("store.lock");
    fs::remove_file(&lock).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&lock)
        .status()
        .unwrap()
        .success());
    assert_eq!(
        fixture.run("query", &Query::new("physics units".into(), 100))["status"],
        "unavailable"
    );
}
