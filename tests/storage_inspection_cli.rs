use remarkable_reader_buddy::{config::Config, storage::*};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-inspection-cli-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn prepare(&self) -> PathBuf {
        let store = Store::open(self.paths()).unwrap();
        drop(store);
        let config = self.0.join("config.json");
        fs::write(
            &config,
            serde_json::to_vec(&Config {
                paths: self.paths(),
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        config
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    digest(&fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}
fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command
        .current_dir(&fixture.0)
        .env_remove("OPENAI_API_KEY")
        .env_remove("REMARKABLE_BUDDIES_CONFIG");
    command
}
#[test]
fn actual_cli_reports_without_credentials_or_any_store_changes() {
    let fixture = Fixture::new();
    let config = fixture.prepare();
    fs::write(
        fixture.paths().credentials.join("model.key"),
        b"PROTECTED_CLI_CREDENTIAL_SENTINEL",
    )
    .unwrap();
    let before = snapshot(&fixture.0);
    let output = command(&fixture)
        .env("REMARKABLE_BUDDIES_CONFIG", config)
        .arg("--inspect-owned-data")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["report_version"], 1);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["credentials_inspected"], false);
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SENTINEL"));
    assert_eq!(snapshot(&fixture.0), before);
}
#[test]
fn actual_cli_skips_dotenv_and_sanitizes_bad_configuration() {
    let fixture = Fixture::new();
    let config = fixture.prepare();
    fs::write(
        fixture.0.join(".env"),
        format!(
            "REMARKABLE_BUDDIES_CONFIG={}\nOPENAI_API_KEY=DOTENV_CREDENTIAL_SENTINEL\n",
            config.display()
        ),
    )
    .unwrap();
    let output = command(&fixture)
        .arg("--inspect-owned-data")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["owner_actor"].is_null(),
        "inspection must not use the .env configuration"
    );
    fs::write(
        &config,
        b"{\"sync\":{\"enabled\":\"CONFIG_VALUE_SECRET_SENTINEL\"}}",
    )
    .unwrap();
    let before = snapshot(&fixture.0);
    let output = command(&fixture)
        .env("REMARKABLE_BUDDIES_CONFIG", &config)
        .arg("--inspect-owned-data")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "unavailable");
    assert!(report["complete_apparent_bytes"].is_null());
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SENTINEL"));
    assert_eq!(snapshot(&fixture.0), before);
}
#[test]
fn actual_cli_rejects_execution_overrides_without_echoing_secret_values() {
    let fixture = Fixture::new();
    fixture.prepare();
    let output = command(&fixture)
        .args([
            "--inspect-owned-data",
            "--api-key",
            "ARGUMENT_SECRET_SENTINEL",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ARGUMENT_SECRET_SENTINEL"));
    let output = command(&fixture)
        .args(["--inspect-owned-data", "--simulate", "unused.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn actual_cli_refuses_competing_one_shot_selectors_without_dispatch() {
    let fixture = Fixture::new();
    fixture.prepare();
    let before = snapshot(&fixture.0);
    let conflicts: &[&[&str]] = &[
        &["--inspect-owned-data", "--export-note"],
        &["--export-note", "--inspect-owned-data"],
        &[
            "--inspect-owned-data",
            "--personalization-operation",
            "inspect",
        ],
        &[
            "--personalization-operation",
            "inspect",
            "--inspect-owned-data",
        ],
    ];
    for flags in conflicts {
        let output = command(&fixture).args(*flags).output().unwrap();
        assert!(!output.status.success(), "{flags:?}");
        assert!(output.stdout.is_empty(), "one-shot dispatch occurred");
        assert_eq!(snapshot(&fixture.0), before);
    }
}
