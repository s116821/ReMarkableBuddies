//! Strict portable preference artifacts and explicit private export and inspection.
use super::inspection::{self, Status};
use crate::storage::{digest, files, MAX_RECORD};
use anyhow::{anyhow, ensure, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{io::Write, path::PathBuf};

pub mod import;
pub mod record;
pub mod selected;

pub const MAX_IO: usize = 128 * 1024;
const SETTINGS: [&str; 3] = ["product_settings", "personalization", "research_policy"];

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Operation {
    Export,
    Inspect,
}
#[derive(Serialize)]
struct Summary {
    artifact_sha256: String,
    declared_settings: usize,
    personalization_included: bool,
}
// Deliberately no Debug: export alone contains the selected private text.
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    status: Status,
    runtime_activation: &'static str,
    artifact: Option<Value>,
    summary: Option<Summary>,
}
impl Report {
    fn refusal(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            runtime_activation: "unverified",
            artifact: None,
            summary: None,
        }
    }
    fn success(operation: Operation, artifact: Value) -> Self {
        let settings = artifact["settings"]
            .as_object()
            .expect("validated portable settings");
        let summary = Summary {
            artifact_sha256: digest(&serde_json::to_vec(&artifact).expect("JSON artifact")),
            declared_settings: settings.len(),
            personalization_included: settings.contains_key("personalization"),
        };
        Self {
            report_version: 1,
            operation,
            status: Status::Ok,
            runtime_activation: "unverified",
            artifact: matches!(operation, Operation::Export).then_some(artifact),
            summary: Some(summary),
        }
    }
    /// Consume one complete frame; never append a replacement after uncertain I/O.
    pub fn write_to(self, output: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self).map_err(|_| anyhow!("output-failure"))?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_IO, "output-failure");
        output
            .write_all(&bytes)
            .map_err(|_| anyhow!("output-failure"))?;
        output.flush().map_err(|_| anyhow!("output-failure"))
    }
}
fn project(bytes: &[u8]) -> std::result::Result<Value, Status> {
    project_saved(bytes).map(|(_, artifact)| artifact)
}

pub(crate) fn project_saved(bytes: &[u8]) -> std::result::Result<(super::Config, Value), Status> {
    let (config, raw) = inspection::document(bytes)?;
    let settings: Map<String, Value> = SETTINGS
        .into_iter()
        .filter_map(|key| raw.get(key).map(|v| (key.to_owned(), v.clone())))
        .collect();
    Ok((
        config,
        json!({"portable_configuration_version": 1, "settings": settings}),
    ))
}
fn validate(bytes: &[u8]) -> std::result::Result<Value, Status> {
    if bytes.len() > MAX_IO {
        return Err(Status::Unavailable);
    }
    let artifact = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    validate_value(&artifact)?;
    Ok(artifact)
}
fn validate_value(artifact: &Value) -> std::result::Result<(), Status> {
    let root = artifact.as_object().ok_or(Status::InvalidInput)?;
    if root.len() != 2
        || !root.contains_key("portable_configuration_version")
        || !root.contains_key("settings")
    {
        return Err(Status::InvalidInput);
    }
    let version = root["portable_configuration_version"]
        .as_u64()
        .ok_or(Status::InvalidInput)?;
    if version != 1 {
        return Err(Status::Unsupported);
    }
    let settings = root["settings"].as_object().ok_or(Status::InvalidInput)?;
    if settings.keys().any(|key| !SETTINGS.contains(&key.as_str())) {
        return Err(Status::InvalidInput);
    }
    let mut config = settings.clone();
    config.insert("config_schema_version".into(), json!(1));
    inspection::document(&serde_json::to_vec(&config).map_err(|_| Status::InvalidInput)?)?;
    Ok(())
}

/// Overlay present declarations without materializing defaults.
fn merge_settings(destination: &mut Map<String, Value>, declarations: &Map<String, Value>) {
    for (name, imported) in declarations {
        if let Some(children) = imported.as_object() {
            let current = destination.entry(name.clone()).or_insert(Value::Null);
            if !current.is_object() {
                *current = json!({});
            }
            current
                .as_object_mut()
                .expect("destination setting object")
                .extend(children.clone());
        } else {
            destination.insert(name.clone(), imported.clone());
        }
    }
}
/// No environment, stdin, Store or default-file fallback is accepted.
pub fn export(path: PathBuf) -> Report {
    let result = files::read(&path, MAX_RECORD as u64)
        .map_err(|_| Status::Unavailable)
        .and_then(|bytes| project(&bytes));
    match result {
        Ok(artifact) => Report::success(Operation::Export, artifact),
        Err(status) => Report::refusal(Operation::Export, status),
    }
}
/// The artifact is the only input. Its original source configuration is not needed.
pub fn inspect(path: PathBuf) -> Report {
    let result = files::read(&path, MAX_IO as u64)
        .map_err(|_| Status::Unavailable)
        .and_then(|bytes| validate(&bytes));
    match result {
        Ok(artifact) => Report::success(Operation::Inspect, artifact),
        Err(status) => Report::refusal(Operation::Inspect, status),
    }
}

#[cfg(test)]
mod tests;
