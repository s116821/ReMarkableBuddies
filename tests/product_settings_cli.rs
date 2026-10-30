//! Actual host command admission; no model, Mem, tablet or service operation.
use remarkable_reader_buddy::storage::{Store, StorePaths, Uuid};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: PathBuf,
    config: PathBuf,
    paths: StorePaths,
}
impl Fixture {
    fn new(settings: Value) -> Self {
        let root = std::env::temp_dir().join(format!("product-cli-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let config = root.join("config.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({"paths":paths,"product_settings":settings})).unwrap(),
        )
        .unwrap();
        Self {
            root,
            config,
            paths,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .current_dir(&self.root)
            .args(args)
            .env("REMARKABLE_BUDDIES_CONFIG", &self.config)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("MEM_API_KEY")
            .env_remove("READER_BUDDY_DEBUG_DUMP")
            .env_remove("RUST_LOG")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("WATCHDOG_USEC")
            .env_remove("WATCHDOG_PID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn invoke(&self, args: &[&str], input: Option<&Value>) -> Output {
        let mut child = self.command(args).spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(input).unwrap())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }
    fn report(&self, args: &[&str], input: &Value) -> Value {
        let output = self.invoke(args, Some(input));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn no_store(&self) {
        assert!(!self.paths.data.exists());
        assert!(!self.paths.cache.exists());
        assert!(!self.paths.credentials.exists());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn identities() -> (Uuid, Uuid, Uuid, Uuid, Uuid, Uuid) {
    (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
}

#[test]
fn reader_off_stops_before_poisoned_credentials_or_store_and_reader_on_is_independent() {
    let f = Fixture::new(json!({"reader_enabled":false,"writer_enabled":true}));
    fs::write(
        f.root.join(".env"),
        b"OPENAI_API_KEY=PRIVATE_POISON\nREADER_BUDDY_DEBUG_DUMP=not-a-bool\n",
    )
    .unwrap();
    let output = f.invoke(&[], None);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Reader is disabled in saved settings"));
    assert!(!stderr.contains("PRIVATE_POISON"));
    f.no_store();
    let f = Fixture::new(
        json!({"reader_enabled":true,"writer_enabled":false,"export_destination":"disabled"}),
    );
    let output = f.invoke(&[], None);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Reader is disabled"));
    f.no_store();
}

#[test]
fn disabled_writer_gates_each_actual_command_family_before_credentials_or_store() {
    let f = Fixture::new(
        json!({"reader_enabled":true,"writer_enabled":false,"export_destination":"mem"}),
    );
    let (conversation, operation, draft, correlation, effect, outer) = identities();
    let refinement = json!({"command":"refine","conversation":conversation,"operation":operation,"draft":draft,"request":"PRIVATE_REQUEST"});
    let notebook = json!({"command":"refine-notebook","conversation":conversation,"operation":operation,"draft":draft,"request":"PRIVATE_REQUEST","max_model_calls":1,"history_range":null});
    let generation = serde_json::to_value(remarkable_reader_buddy::writer::Request::Generate {
        conversation,
        operation,
        request: "PRIVATE_REQUEST".into(),
        document: remarkable_reader_buddy::retrieval::NormalizedDocument {
            document: Uuid::new_v4(),
            format: remarkable_reader_buddy::retrieval::SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "synthetic-v1".into(),
            units: vec![remarkable_reader_buddy::retrieval::SourceUnit {
                page: Some(1),
                section: None,
                text: "synthetic".into(),
            }],
        },
        selection: remarkable_reader_buddy::conversation::TextScope::SuppliedNotebook,
        guidance: None,
    })
    .unwrap();
    let notebook_generation = serde_json::to_value(
        remarkable_reader_buddy::writer::notebook::command::Request::GenerateNotebook(Box::new(
            remarkable_reader_buddy::writer::notebook::command::Generate {
                conversation,
                operation,
                request: "PRIVATE_REQUEST".into(),
                document_file: f.root.join("must-not-read.json"),
                document_sha256: "a".repeat(64),
                selection: remarkable_reader_buddy::conversation::TextScope::SuppliedNotebook,
                guidance: None,
                max_model_calls: 1,
                history_range: None,
            },
        )),
    )
    .unwrap();
    for request in [
        generation.clone(),
        json!({"command":"generate-and-export","outer_operation":outer,"generation":generation,"correlation":correlation,"effect_operation":effect}),
        notebook_generation.clone(),
        json!({"command":"generate-notebook-and-export","outer_operation":outer,"generation":notebook_generation,"correlation":correlation,"effect_operation":effect}),
        refinement.clone(),
        json!({"command":"prepare-export","conversation":conversation,"draft":draft,"correlation":correlation,"effect_operation":effect}),
        json!({"command":"refine-and-export","outer_operation":outer,"refinement":refinement,"correlation":correlation,"effect_operation":effect}),
        json!({"command":"export-draft","outer_operation":outer,"conversation":conversation,"draft":draft,"correlation":correlation,"effect_operation":effect}),
        notebook.clone(),
        json!({"command":"refine-notebook-and-export","outer_operation":outer,"refinement":notebook,"correlation":correlation,"effect_operation":effect}),
    ] {
        let report = f.report(&["--writer-note"], &request);
        assert_eq!(report["failure"], "unsupported", "{request}: {report}");
        assert!(!report.to_string().contains("PRIVATE_REQUEST"));
        f.no_store();
    }
    let export = json!({"command":"export","note":{"conversation":conversation,"correlation":correlation,"effect_operation":effect,"source_scope":"synthetic","source_revision":"r1","title":"Title","markdown":"PRIVATE_REQUEST"}});
    assert_eq!(
        f.report(&["--export-note"], &export)["failure"],
        "unsupported"
    );
    f.no_store();
}

#[test]
fn backend_off_blocks_joined_work_before_model_and_allows_original_reconciliation() {
    let f = Fixture::new(
        json!({"reader_enabled":false,"writer_enabled":true,"export_destination":"disabled"}),
    );
    let (conversation, operation, draft, correlation, effect, outer) = identities();
    let refine = json!({"command":"refine","conversation":conversation,"operation":operation,"draft":draft,"request":"synthetic"});
    // Local draft work is still admitted to its original credential gate.
    assert_eq!(
        f.report(&["--writer-note"], &refine)["failure"],
        "missing-credentials"
    );
    let joined = json!({"command":"refine-and-export","outer_operation":outer,"refinement":refine,"correlation":correlation,"effect_operation":effect});
    assert_eq!(
        f.report(&["--writer-note"], &joined)["failure"],
        "unsupported"
    );
    let notebook = json!({"command":"refine-notebook","conversation":conversation,"operation":operation,"draft":draft,"request":"synthetic","max_model_calls":1,"history_range":null});
    let joined = json!({"command":"refine-notebook-and-export","outer_operation":outer,"refinement":notebook,"correlation":correlation,"effect_operation":effect});
    assert_eq!(
        f.report(&["--writer-note"], &joined)["failure"],
        "unsupported"
    );
    f.no_store();
    let export = json!({"command":"export","note":{"conversation":conversation,"correlation":correlation,"effect_operation":effect,"source_scope":"synthetic","source_revision":"r1","title":"Title","markdown":"synthetic"}});
    assert_eq!(
        f.report(&["--export-note"], &export)["failure"],
        "unsupported"
    );
    f.no_store();
    // With no token, allowed original-only reconciliation stops at the historical
    // credential gate. No remote lookup or new effect can occur in this test.
    for writer in [false, true] {
        let f = Fixture::new(
            json!({"reader_enabled":false,"writer_enabled":writer,"export_destination":"disabled"}),
        );
        let reconcile = json!({"command":"reconcile-export","conversation":conversation,"correlation":correlation});
        assert_eq!(
            f.report(&["--writer-note"], &reconcile)["failure"],
            "missing-credentials"
        );
        let reconcile =
            json!({"command":"reconcile","conversation":conversation,"correlation":correlation});
        assert_eq!(
            f.report(&["--export-note"], &reconcile)["failure"],
            "missing-credentials"
        );
        f.no_store();
    }
}

#[test]
fn maintenance_can_reenable_all_disabled_without_credentials_and_selector_is_exclusive() {
    let f = Fixture::new(
        json!({"reader_enabled":false,"writer_enabled":false,"export_destination":"disabled"}),
    );
    let output = f.invoke(&["--product-settings-operation", "inspect"], None);
    assert!(output.status.success());
    let inspect: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspect["status"], "ok");
    f.no_store();
    drop(Store::open(f.paths.clone()).unwrap());
    let request = json!({"product_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":inspect["data"]["base_sha256"],"config_path_sha256":inspect["data"]["config_path_sha256"],"changes":{"reader_enabled":{"action":"clear"},"writer_enabled":{"action":"clear"},"export_destination":{"action":"clear"}}});
    let prepared = f.report(
        &[
            "--product-settings-operation",
            "prepare",
            "--product-settings-input",
            "-",
        ],
        &request,
    );
    assert_eq!(prepared["outcome"], "prepared");
    let applied = f.report(
        &[
            "--product-settings-operation",
            "apply",
            "--product-settings-input",
            "-",
        ],
        &json!({"intent":prepared["data"]["intent"]}),
    );
    assert_eq!(applied["outcome"], "verified");
    assert_eq!(
        applied["data"]["current"]["reader_enabled"],
        json!({"origin":"default","value":true})
    );
    for peer in [
        "--writer-note",
        "--export-note",
        "--handwriting",
        "--inspect-owned-data",
    ] {
        assert!(!f
            .invoke(&["--product-settings-operation", "inspect", peer], None)
            .status
            .success());
    }
    let output = f.invoke(
        &[
            "--product-settings-operation",
            "prepare",
            "--product-settings-input",
            "nonexistent-private-input",
        ],
        None,
    );
    let refused: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(refused["status"], "unsupported");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_reader_gate_precedes_notification_connect_and_keeps_trusted_predotenv_settings() {
    use std::os::unix::net::UnixDatagram;
    let disabled = Fixture::new(json!({"reader_enabled":false}));
    let missing = disabled.root.join("missing-notification.sock");
    let output = disabled
        .command(&[])
        .env("NOTIFY_SOCKET", &missing)
        .env("WATCHDOG_USEC", "120000000")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("Reader is disabled in saved settings"),
        "{error}"
    );
    assert!(!error.contains("notification socket"));
    disabled.no_store();

    let enabled = Fixture::new(json!({"reader_enabled":true}));
    let output = enabled
        .command(&[])
        .env("NOTIFY_SOCKET", &missing)
        .env("WATCHDOG_USEC", "120000000")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Connect service notification socket"));
    enabled.no_store();

    // An untrusted .env must neither introduce notification settings into an
    // unsupervised process nor replace the original valid process settings.
    fs::write(
        enabled.root.join(".env"),
        b"NOTIFY_SOCKET=/nonexistent/private-sentinel.sock\nWATCHDOG_USEC=poison\nWATCHDOG_PID=1\n",
    )
    .unwrap();
    let output = enabled.command(&[]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        !error.contains("notification")
            && !error.contains("watchdog")
            && !error.contains("private-sentinel"),
        "{error}"
    );
    enabled.no_store();
    let bound = enabled.root.join("notify.sock");
    let receiver = UnixDatagram::bind(&bound).unwrap();
    let output = enabled
        .command(&[])
        .env("NOTIFY_SOCKET", &bound)
        .env("WATCHDOG_USEC", "120000000")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        !error.contains("notification")
            && !error.contains("watchdog")
            && !error.contains("private-sentinel"),
        "{error}"
    );
    enabled.no_store();
    drop(receiver);
}
