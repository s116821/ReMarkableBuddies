use remarkable_reader_buddy::{
    config::Config,
    storage::{StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn actual_cli_refuses_strict_input_and_credentials_before_store_provider_or_content_logs() {
    let root = std::env::temp_dir().join(format!("rmb-transcription-cli-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let config = Config {
        paths: StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        },
        ..Config::default()
    };
    let path = root.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/handwriting-transcription-v1.json")).unwrap();
    let valid = json!({"transcription_version":1,"invocation":Uuid::new_v4(),"document":Uuid::new_v4(),"page":7,"target_png":fixture["target_png"],"context":null});
    for (input, failure) in [
        (b"{PRIVATE_INPUT_CANARY}".to_vec(), "invalid-input"),
        (
            serde_json::to_vec(&valid).unwrap(),
            "credentials-unavailable",
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .arg("--handwriting-transcription")
            .env("REMARKABLE_BUDDIES_CONFIG", &path)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["failure"], failure);
        assert_eq!(report["provider_attempt"], "not-called");
        assert_eq!(report["confirmed"], false);
        assert!(report["result"].is_null());
        assert!(!config.paths.data.exists());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!stderr.contains("PRIVATE_INPUT_CANARY"));
        assert!(!stderr.contains("iVBOR"));
    }
    for extra in ["--handwriting", "--writer-note", "--inspect-owned-data"] {
        let result = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .args(["--handwriting-transcription", extra])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    let absolute = root.canonicalize().unwrap();
    assert_eq!(
        absolute.parent().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    assert!(absolute
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("rmb-transcription-cli-"));
    std::fs::remove_dir_all(absolute).unwrap();
}
