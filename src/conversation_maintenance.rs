//! Explicit original-bound ordinary historical deletion. No native authority.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
use crate::{
    config::inspection,
    storage::{digest, valid_digest, Uuid},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
mod engine;
mod inventory;
mod service;
pub use inventory::InventoryReport;

/// Read-only candidates only; explicit selection still needs fresh inspect/prepare.
pub fn inspect_inventory(config: Option<PathBuf>) -> InventoryReport {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        InventoryReport::refused(inventory::InventoryStatus::Unsupported, None)
    }
    #[cfg(target_os = "linux")]
    {
        engine::inventory(config, std::sync::Arc::new(service::System))
    }
}
pub const MAX_INPUT: usize = 16384;
pub const MAX_OUTPUT: usize = 32768;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Prepare,
    Apply,
    Status,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ready,
    Prepared,
    Acknowledged,
    Unchanged,
    Unresolved,
    InvalidInput,
    Unavailable,
    Unsupported,
    Busy,
    Conflict,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expected {
    pub revision: String,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub id: String,
    pub revision: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Summary {
    pub turns: u64,
    pub source_records: u64,
    pub export_associations: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub intent_version: u32,
    pub operation: String,
    pub conversation: String,
    pub config_path_sha256: String,
    pub config_bytes_sha256: String,
    pub roots_sha256: String,
    pub actor: String,
    pub generation: String,
    pub mode_sha256: String,
    pub target_history_sha256: String,
    pub root: Expected,
    pub binding: Option<Binding>,
    pub summary: Summary,
    pub intent_sha256: String,
}
pub(crate) fn uuid(s: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(s)?;
    ensure!(
        !id.is_nil() && id.to_string() == s,
        "invalid canonical UUID"
    );
    Ok(id)
}
impl Intent {
    pub(crate) fn hash(&self) -> Result<String> {
        // Serialize the typed prefix in its wire field order, omitting ONLY the digest.
        let mut b = serde_json::to_vec(self)?;
        let suffix = format!(
            ",\"intent_sha256\":{}",
            serde_json::to_string(&self.intent_sha256)?
        );
        ensure!(
            b.ends_with(format!("{suffix}}}").as_bytes()),
            "intent encoding"
        );
        b.truncate(b.len() - suffix.len() - 1);
        b.push(b'}');
        let mut framed = b"RMB-CONVERSATION-DELETE-V1\n".to_vec();
        framed.extend(b);
        Ok(digest(&framed))
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.intent_version == 1, "unsupported intent");
        for s in [
            &self.operation,
            &self.conversation,
            &self.actor,
            &self.generation,
            &self.root.revision,
        ] {
            uuid(s)?;
        }
        for h in [
            &self.config_path_sha256,
            &self.config_bytes_sha256,
            &self.roots_sha256,
            &self.mode_sha256,
            &self.target_history_sha256,
            &self.root.sha256,
            &self.intent_sha256,
        ] {
            ensure!(valid_digest(h), "invalid digest");
        }
        if let Some(b) = &self.binding {
            uuid(&b.id)?;
            uuid(&b.revision)?;
            ensure!(valid_digest(&b.sha256), "binding digest");
        }
        ensure!(
            [
                self.summary.turns,
                self.summary.source_records,
                self.summary.export_associations
            ]
            .into_iter()
            .all(|n| n <= 4096),
            "summary bound"
        );
        ensure!(self.hash()? == self.intent_sha256, "intent digest");
        Ok(())
    }
    pub(crate) fn derived(&self, role: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:remarkable-buddies:conversation-cleanup:v1:{}:{role}",
                self.operation
            )
            .as_bytes(),
        )
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    request_version: u32,
    conversation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    request_version: u32,
    operation: String,
    conversation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    intent: Intent,
}
pub(crate) enum Request {
    Inspect(String),
    Prepare(String, String),
    Confirm(Box<Intent>),
}
fn parse(op: Operation, b: &[u8]) -> std::result::Result<Request, Status> {
    let value = inspection::strict_json(b).map_err(|_| Status::InvalidInput)?;
    if !value.is_object() {
        return Err(Status::InvalidInput);
    }
    let parsed = (|| -> Result<Request> {
        match op {
            Operation::Inspect => {
                let r: Inspect = serde_json::from_slice(b)?;
                ensure!(r.request_version == 1, "version");
                uuid(&r.conversation)?;
                Ok(Request::Inspect(r.conversation))
            }
            Operation::Prepare => {
                let r: Prepare = serde_json::from_slice(b)?;
                ensure!(r.request_version == 1, "version");
                uuid(&r.operation)?;
                uuid(&r.conversation)?;
                Ok(Request::Prepare(r.operation, r.conversation))
            }
            _ => {
                ensure!(value["intent"].is_object(), "intent object");
                let i = &value["intent"];
                ensure!(
                    i.get("binding").is_some() && i["root"].is_object() && i["summary"].is_object(),
                    "required objects"
                );
                let r: Confirmation = serde_json::from_slice(b)?;
                r.intent.validate()?;
                Ok(Request::Confirm(Box::new(r.intent)))
            }
        }
    })();
    parsed.map_err(|_| Status::InvalidInput)
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Receipt {
    pub operation: String,
    pub transaction: Uuid,
    pub root_revision: Uuid,
    pub binding_revision: Option<Uuid>,
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    status: Status,
    runtime_unverified: bool,
    local_only: bool,
    physical_reclamation: bool,
    service_observation: &'static str,
    possible_effect: bool,
    conversation: Option<String>,
    intent: Option<Intent>,
    receipt: Option<Receipt>,
    current_state: Option<&'static str>,
    summary: Option<Summary>,
    #[serde(skip)]
    custody: Option<engine::Custody>,
}
impl Report {
    fn new(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            runtime_unverified: true,
            local_only: true,
            physical_reclamation: false,
            service_observation: "unverified",
            possible_effect: false,
            conversation: None,
            intent: None,
            receipt: None,
            current_state: None,
            summary: None,
            custody: None,
        }
    }
    pub fn rejected(operation: Operation) -> Self {
        Self::new(operation, Status::Unsupported)
    }
    pub fn write_to(mut self, out: &mut impl Write) -> Result<()> {
        if let Some(c) = &mut self.custody {
            if let Err(status) = c.check() {
                self.status = if self.possible_effect {
                    Status::Unknown
                } else {
                    status
                };
                self.service_observation = "unverified";
                self.intent = None;
                self.receipt = None;
                self.summary = None;
                self.current_state = None;
            }
        }
        let mut b = serde_json::to_vec(&self)?;
        b.push(b'\n');
        ensure!(b.len() <= MAX_OUTPUT, "conversation report bound");
        let result = out.write_all(&b);
        let _hold_until_output_finishes = &self.custody;
        result?;
        Ok(())
    }
}
pub fn run(
    operation: Operation,
    selector: &Path,
    config: Option<PathBuf>,
    input: &mut impl Read,
) -> Report {
    if selector.as_os_str() != std::ffi::OsStr::new("-") {
        return Report::rejected(operation);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, input);
        Report::rejected(operation)
    }
    #[cfg(target_os = "linux")]
    {
        let mut b = Vec::new();
        if input
            .take(MAX_INPUT as u64 + 1)
            .read_to_end(&mut b)
            .is_err()
            || b.len() > MAX_INPUT
        {
            return Report::new(operation, Status::InvalidInput);
        }
        let request = match parse(operation, &b) {
            Ok(r) => r,
            Err(s) => return Report::new(operation, s),
        };
        engine::run(
            operation,
            config,
            request,
            std::sync::Arc::new(service::System),
        )
    }
}
#[cfg(test)]
mod tests;
