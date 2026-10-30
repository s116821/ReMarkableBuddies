//! Portable preference and one-shot maintenance through the existing config writer.
use super::Config;
use crate::storage::{digest, files, valid_digest, Store, MAX_RECORD};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_IO_BYTES: usize = 128 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Personalization {
    pub schema_version: u32,
    pub enabled: bool,
    pub text: String,
}
impl Default for Personalization {
    fn default() -> Self {
        Self {
            schema_version: 1,
            enabled: false,
            text: String::new(),
        }
    }
}
impl std::fmt::Debug for Personalization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Personalization")
            .field("schema_version", &self.schema_version)
            .field("enabled", &self.enabled)
            .field("text", &"[REDACTED]")
            .finish()
    }
}
impl Personalization {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported personalization version"
        );
        ensure!(
            self.text.len() <= MAX_TEXT_BYTES && !self.text.contains('\0'),
            "invalid personalization text"
        );
        Ok(())
    }
    pub fn snapshot(&self) -> Result<PersonalizationSnapshot> {
        self.validate()?;
        Ok(PersonalizationSnapshot(self.clone()))
    }
}
/// Immutable validated preference; callers must keep it separate from authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonalizationSnapshot(Personalization);
impl PersonalizationSnapshot {
    pub fn schema_version(&self) -> u32 {
        self.0.schema_version
    }
    pub fn effective_text(&self) -> Option<&str> {
        (self.0.enabled && !self.0.text.is_empty()).then_some(self.0.text.as_str())
    }
}

#[derive(Clone, Copy, Debug, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Set,
    Disable,
    Clear,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    InvalidInput,
    Unavailable,
    Unsupported,
    Busy,
    Conflict,
    Failed,
}
#[derive(Serialize)]
pub struct Report {
    pub report_version: u32,
    pub operation: Operation,
    pub status: Status,
    pub disk_reload_verified: bool,
    pub runtime_activation: &'static str,
    pub config_sha256: Option<String>,
    pub personalization: Option<Personalization>,
}
impl Report {
    fn new(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            disk_reload_verified: false,
            runtime_activation: "unverified",
            config_sha256: None,
            personalization: None,
        }
    }
    pub fn write_to(&self, output: &mut impl std::io::Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_IO_BYTES,
            "personalization report exceeds bound"
        );
        output.write_all(&bytes)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    contract_version: u32,
    expected_config_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    contract_version: u32,
    expected_config_sha256: String,
    enabled: bool,
    text: String,
}
fn load(path: &Path) -> Result<(Config, String)> {
    let bytes = files::read(path, MAX_RECORD as u64)?;
    let config: Config = serde_json::from_slice(&bytes)?;
    config.validate()?;
    Ok((config, digest(&bytes)))
}
/// No runtime, credentials, provider or device startup. Failures contain no text/path.
/// A failure after publication is an uncertain outcome, never proof of rollback.
pub fn run(operation: Operation, path: Option<PathBuf>, input: &mut impl Read) -> Report {
    let Some(path) = path else {
        return Report::new(operation, Status::Unavailable);
    };
    let (mut config, revision) = match load(&path) {
        Ok(value) => value,
        Err(_) => return Report::new(operation, Status::Unavailable),
    };
    if !matches!(operation, Operation::Inspect) {
        let mut raw = Vec::new();
        if input
            .take((MAX_IO_BYTES + 1) as u64)
            .read_to_end(&mut raw)
            .is_err()
            || raw.len() > MAX_IO_BYTES
        {
            return Report::new(operation, Status::InvalidInput);
        }
        let expected = match operation {
            Operation::Set => {
                let value: Set = match serde_json::from_slice(&raw) {
                    Ok(v) => v,
                    Err(_) => return Report::new(operation, Status::InvalidInput),
                };
                if value.contract_version != 1 {
                    return Report::new(operation, Status::InvalidInput);
                }
                config.personalization = Personalization {
                    schema_version: 1,
                    enabled: value.enabled,
                    text: value.text,
                };
                value.expected_config_sha256
            }
            Operation::Disable | Operation::Clear => {
                let value: Change = match serde_json::from_slice(&raw) {
                    Ok(v) => v,
                    Err(_) => return Report::new(operation, Status::InvalidInput),
                };
                if value.contract_version != 1 {
                    return Report::new(operation, Status::InvalidInput);
                }
                if matches!(operation, Operation::Clear) {
                    config.personalization = Personalization::default()
                } else {
                    config.personalization.enabled = false
                }
                value.expected_config_sha256
            }
            Operation::Inspect => unreachable!(),
        };
        if !valid_digest(&expected) || config.validate().is_err() {
            return Report::new(operation, Status::InvalidInput);
        }
        if expected != revision {
            return Report::new(operation, Status::Conflict);
        }
        // Shared storage owns admission and keeps the same lease through readback.
        let store = match Store::open_existing(config.paths.clone()) {
            Ok(store) => store,
            Err(refusal) => {
                use crate::storage::maintenance::Refusal;
                return Report::new(
                    operation,
                    match refusal {
                        Refusal::Busy => Status::Busy,
                        Refusal::Unsupported => Status::Unsupported,
                        Refusal::Unavailable => Status::Unavailable,
                    },
                );
            }
        };
        let published = match store.replace_config(&path, Some(&expected), &config) {
            Ok(value) => value,
            // Publication can precede an I/O/durability failure. Do not infer
            // that no write happened from a missing successful receipt.
            Err(_) => return Report::new(operation, Status::Failed),
        };
        let (reloaded, observed) = match load(&path) {
            Ok(value) => value,
            Err(_) => return Report::new(operation, Status::Failed),
        };
        if observed != published || reloaded.personalization != config.personalization {
            return Report::new(operation, Status::Failed);
        }
        config = reloaded;
        return Report {
            report_version: 1,
            operation,
            status: Status::Ok,
            disk_reload_verified: true,
            runtime_activation: "restart-required",
            config_sha256: Some(observed),
            personalization: Some(config.personalization),
        };
    }
    Report {
        report_version: 1,
        operation,
        status: Status::Ok,
        disk_reload_verified: true,
        runtime_activation: "unverified",
        config_sha256: Some(revision),
        personalization: Some(config.personalization),
    }
}
