use super::*;
use std::{fs, io};
fn frame(report: Report) -> Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
    serde_json::from_slice(&bytes).unwrap()
}
#[test]
fn real_files_preserve_declarations_exclude_bindings_and_redact_inspection() {
    let dir = Temp::new();
    let config = dir.path().join("saved.json");
    let artifact = dir.path().join("portable.json");
    let settings = json!({"product_settings":{"writer_enabled":false},"personalization":{"enabled":false,"text":"PRIVATE é\n日本語"},"research_policy":null});
    let mut source = settings.clone();
    source["paths"] = json!({"data":dir.path().join("absent-data"),"cache":dir.path().join("absent-cache"),"credentials":dir.path().join("absent-credentials")});
    source["base_url"] = json!("https://excluded.invalid/v1");
    source["model_credential"] = json!("excluded_key");
    source["model"] = json!("excluded-model");
    source["log_level"] = json!("debug");
    let bytes = serde_json::to_vec(&source).unwrap();
    fs::write(&config, &bytes).unwrap();
    let exported = frame(export(config.clone()));
    assert_eq!(exported["status"], "ok");
    assert_eq!(exported["artifact"]["settings"], settings);
    let frozen = serde_json::to_vec(&exported["artifact"]).unwrap();
    fs::write(&artifact, &frozen).unwrap();
    let inspected = frame(inspect(artifact));
    assert_eq!(inspected["summary"], exported["summary"]);
    assert_eq!(inspected["summary"]["artifact_sha256"], digest(&frozen));
    assert_eq!(inspected["summary"]["declared_settings"], 3);
    assert_eq!(inspected["artifact"], Value::Null);
    for private in [
        "PRIVATE",
        "excluded.invalid",
        "excluded_key",
        "excluded-model",
        "absent-data",
    ] {
        assert!(!inspected.to_string().contains(private));
    }
    assert_eq!(fs::read(config).unwrap(), bytes);
    for root in ["absent-data", "absent-cache", "absent-credentials"] {
        assert!(!dir.path().join(root).exists());
    }
    let saved = inspection::document(&bytes).unwrap();
    assert_eq!(saved.1["personalization"], settings["personalization"]);
}
#[test]
fn omitted_sparse_null_and_disabled_preferences_do_not_materialize_defaults() {
    for settings in [
        json!({}),
        json!({"product_settings":null,"research_policy":null}),
        json!({"personalization":{}}),
        json!({"product_settings":{}}),
        json!({"personalization":{"text":"kept","enabled":false}}),
        json!({"research_policy":"disabled"}),
        json!({"research_policy":"required"}),
    ] {
        let projected = project(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(projected["settings"], settings);
        assert_eq!(
            validate(&serde_json::to_vec(&projected).unwrap()).unwrap(),
            projected
        );
    }
}
#[test]
fn whole_source_validation_and_recursive_duplicate_refusal_precede_projection() {
    for bytes in [
        br#"{"log_level":"bad"}"#.as_slice(),
        br#"{"reader":{"unexpected":1}}"#,
        br#"{"personalization":{"text":"a","text":"b"}}"#,
        br#"{"research_policy":null,"research_policy":null}"#,
        br#"{"future":1}"#,
        br#"{"personalization":[]}"#,
        br#"{"personalization":null}"#,
    ] {
        assert!(project(bytes).is_err());
    }
    assert!(matches!(
        project(br#"{"config_schema_version":2}"#),
        Err(Status::Unsupported)
    ));
}
#[test]
fn offline_artifact_is_closed_complete_and_versioned() {
    for bytes in [br#"[]"#.as_slice(),br#"{"portable_configuration_version":1,"settings":{}}{}"#,br#"{"portable_configuration_version":1.0,"settings":{}}"#,br#"{"portable_configuration_version":true,"settings":{}}"#,br#"{"portable_configuration_version":1,"settings":null}"#,br#"{"portable_configuration_version":1,"settings":{"paths":{}}}"#,br#"{"portable_configuration_version":1,"settings":{"product_settings":{"future":false}}}"#,br#"{"portable_configuration_version":1,"settings":{"research_policy":null,"research_policy":null}}"#,br#"{"portable_configuration_version":1,"settings":{},"extra":null}"#] { assert!(validate(bytes).is_err()); }
    assert!(matches!(
        validate(br#"{"portable_configuration_version":2,"settings":{}}"#),
        Err(Status::Unsupported)
    ));
    let a = validate(br#"{ "settings":{}, "portable_configuration_version":1 }"#).unwrap();
    let b = validate(br#"{"portable_configuration_version":1,"settings":{}}"#).unwrap();
    assert_eq!(
        frame(Report::success(Operation::Inspect, a)),
        frame(Report::success(Operation::Inspect, b))
    );
}
#[test]
fn inclusive_input_bounds_maximum_escaped_text_and_unavailable_files() {
    let dir = Temp::new();
    let path = dir.path().join("input.json");
    let mut bytes =
        serde_json::to_vec(&json!({"portable_configuration_version":1,"settings":{}})).unwrap();
    bytes.resize(MAX_IO, b' ');
    fs::write(&path, &bytes).unwrap();
    assert_eq!(frame(inspect(path.clone()))["status"], "ok");
    bytes.push(b' ');
    fs::write(&path, &bytes).unwrap();
    assert_eq!(frame(inspect(path.clone()))["status"], "unavailable");
    let mut config = b"{}".to_vec();
    config.resize(MAX_RECORD, b' ');
    fs::write(&path, &config).unwrap();
    assert_eq!(frame(export(path.clone()))["status"], "ok");
    config.push(b' ');
    fs::write(&path, &config).unwrap();
    assert_eq!(frame(export(path.clone()))["status"], "unavailable");
    let text = "\u{1}".repeat(super::super::personalization::MAX_TEXT_BYTES);
    let artifact =
        project(&serde_json::to_vec(&json!({"personalization":{"text":text}})).unwrap()).unwrap();
    let report = frame(Report::success(Operation::Export, artifact));
    assert_eq!(
        report["artifact"]["settings"]["personalization"]["text"],
        text
    );
    for path in [
        PathBuf::from("relative.json"),
        PathBuf::from("-"),
        dir.path().join("missing"),
        dir.path().to_owned(),
    ] {
        let report = frame(export(path));
        assert_eq!(report["status"], "unavailable");
        assert!(report["artifact"].is_null());
    }
}
struct Failing {
    bytes: Vec<u8>,
    prefix: bool,
    writes: usize,
    flushes: usize,
}
impl Write for Failing {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        if self.prefix {
            if self.writes > 1 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.bytes.extend_from_slice(&b[..7]);
            Ok(7)
        } else {
            self.bytes.extend_from_slice(b);
            Ok(b.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Err(io::ErrorKind::BrokenPipe.into())
    }
}
#[test]
fn output_failures_never_append_replacement_or_retry_the_operation() {
    for prefix in [true, false] {
        let mut writer = Failing {
            bytes: vec![],
            prefix,
            writes: 0,
            flushes: 0,
        };
        let result =
            Report::success(Operation::Export, project(b"{}").unwrap()).write_to(&mut writer);
        assert_eq!(result.unwrap_err().to_string(), "output-failure");
        if prefix {
            assert_eq!(writer.bytes.len(), 7);
            assert_eq!(writer.writes, 2);
            assert_eq!(writer.flushes, 0);
        } else {
            assert_eq!(writer.writes, 1);
            assert_eq!(writer.flushes, 1);
            assert_eq!(writer.bytes.iter().filter(|&&b| b == b'\n').count(), 1);
        }
    }
}
#[cfg(unix)]
#[test]
fn symlink_input_refuses_without_following_source() {
    let dir = Temp::new();
    let source = dir.path().join("source");
    let link = dir.path().join("link");
    fs::write(&source, b"{}").unwrap();
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert_eq!(frame(export(link))["status"], "unavailable");
    assert_eq!(fs::read(source).unwrap(), b"{}");
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buddy-portable-config-{}",
            crate::storage::Uuid::new_v4()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
