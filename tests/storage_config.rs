use remarkable_reader_buddy::{config::*, llm::openai::DEFAULT_MODEL};
use serde_json::json;

fn host_config(value: serde_json::Value) -> Config {
    let mut config: Config = serde_json::from_value(value).unwrap();
    let root = std::env::temp_dir().join("buddy-config-fixture");
    config.paths = remarkable_reader_buddy::storage::StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("secrets"),
    };
    config
}

#[test]
fn explicit_overrides_file_defaults_and_secret_redaction() {
    let config = host_config(
        json!({"model":"file-model","trigger_corner":"UR","base_url":"https://file.invalid","debug_dump":true,"log_level":"warn"}),
    );
    let effective = resolve(
        &config,
        Overrides::default(),
        Environment {
            api_key: Some("fake-secret".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(effective.model, "file-model");
    assert_eq!(effective.corner_name, "UR");
    assert!(effective.debug_dump);
    assert_eq!(effective.sources["api_key"], "environment");
    assert_eq!(effective.sources["model"], "file");
    assert!(!format!("{:?}", effective.key).contains("fake-secret"));
    let effective = resolve(
        &config,
        Overrides {
            api_key: Some("cli-secret".into()),
            model: Some("cli-model".into()),
            trigger_corner: Some("LL".into()),
            debug_dump: true,
            log_level: Some(log::LevelFilter::Off),
            ..Default::default()
        },
        Environment {
            api_key: Some("env-secret".into()),
            debug_dump: Some("invalid-ignored".into()),
            log_filter: Some("trace".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(effective.model, "cli-model");
    assert_eq!(effective.key.expose(), "cli-secret");
    assert_eq!(effective.log_level, Some(log::LevelFilter::Off));
    let defaults = resolve(
        &host_config(json!({})),
        Overrides::default(),
        Environment {
            api_key: Some("fixture".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(defaults.model, DEFAULT_MODEL);
    assert_eq!(defaults.corner_name, "LL");
    assert!(!defaults.debug_dump);
}

#[test]
fn environment_values_win_and_invalid_selected_secrets_do_not_fall_back() {
    let config =
        host_config(json!({"base_url":"https://file.invalid","debug_dump":true,"log_level":"off"}));
    let e = resolve(
        &config,
        Overrides::default(),
        Environment {
            api_key: Some("fixture".into()),
            base_url: Some("https://env.invalid".into()),
            debug_dump: Some("0".into()),
            log_filter: Some("error,ureq=trace".into()),
        },
    )
    .unwrap();
    assert_eq!(e.base_url.as_deref(), Some("https://env.invalid"));
    assert!(!e.debug_dump);
    assert!(e.log_level.is_none());
    assert!(e.log_filter.unwrap().contains("ureq=trace"));
    let error = resolve(
        &config,
        Overrides {
            api_key: Some(" ".into()),
            ..Default::default()
        },
        Environment {
            api_key: Some("valid-fallback-not-used".into()),
            ..Default::default()
        },
    )
    .err()
    .unwrap();
    assert!(!error.to_string().contains("valid-fallback"));
    assert!(serde_json::from_value::<Config>(json!({"api_key":"must-not-be-here"})).is_err());
    let unsupported = Config {
        config_schema_version: 2,
        ..Default::default()
    };
    assert!(unsupported.validate().is_err());
    let invalid: Config = serde_json::from_value(json!({"model_credential":"../secret"})).unwrap();
    assert!(invalid.validate().is_err());
}

#[cfg(unix)]
#[test]
fn linux_secret_permissions_and_symlinks_are_enforced() {
    use remarkable_reader_buddy::storage::{files, Uuid};
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };
    let root = std::env::temp_dir().join(format!("buddy-secrets-{}", Uuid::new_v4()));
    files::directory(&root).unwrap();
    let path = root.join("model-key");
    files::atomic(&path, b"synthetic-key").unwrap();
    assert!(read_secret_file(&path).is_ok());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_secret_file(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let alias = root.join("alias");
    symlink(&path, &alias).unwrap();
    assert!(read_secret_file(&alias).is_err());
    fs::remove_dir_all(root).unwrap();
}
