//! One original-bound derived slot. No Store recovery or runtime authority.
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
#[cfg(target_os = "linux")]
pub(crate) mod linux;
pub const MAX_IO: usize = 16384;
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize)]
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
    Applied,
    Absent,
    Unchanged,
    Conflict,
    Unknown,
    InvalidInput,
    Unavailable,
    Unsupported,
    Busy,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    intent_version: u32,
    operation: String,
    document: String,
    config_path_sha256: String,
    config_bytes_sha256: String,
    roots_sha256: String,
    actor: String,
    generation: String,
    store_owner_sha256: String,
    cache_owner_sha256: String,
    namespace_owner_sha256: String,
    namespace_identity_sha256: String,
    entry_identity_sha256: String,
    entry_bytes: u64,
    entry_sha256: String,
    intent_sha256: String,
}
fn frame(domain: &str, parts: &[String]) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
        bytes.extend_from_slice(part.as_bytes());
    }
    digest(&bytes)
}
fn uuid(raw: &str) -> std::result::Result<Uuid, Status> {
    Uuid::parse_str(raw)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == raw)
        .ok_or(Status::InvalidInput)
}
impl Intent {
    fn hash(&self) -> String {
        frame(
            "buddy-source-cache-cleanup-intent-v1",
            &[
                self.intent_version.to_string(),
                self.operation.clone(),
                self.document.clone(),
                self.config_path_sha256.clone(),
                self.config_bytes_sha256.clone(),
                self.roots_sha256.clone(),
                self.actor.clone(),
                self.generation.clone(),
                self.store_owner_sha256.clone(),
                self.cache_owner_sha256.clone(),
                self.namespace_owner_sha256.clone(),
                self.namespace_identity_sha256.clone(),
                self.entry_identity_sha256.clone(),
                self.entry_bytes.to_string(),
                self.entry_sha256.clone(),
            ],
        )
    }
    fn validate(&self) -> std::result::Result<(), Status> {
        for value in [
            &self.operation,
            &self.document,
            &self.actor,
            &self.generation,
        ] {
            uuid(value)?;
        }
        if self.intent_version != 1
            || !(1..=33554432).contains(&self.entry_bytes)
            || [
                &self.config_path_sha256,
                &self.config_bytes_sha256,
                &self.roots_sha256,
                &self.store_owner_sha256,
                &self.cache_owner_sha256,
                &self.namespace_owner_sha256,
                &self.namespace_identity_sha256,
                &self.entry_identity_sha256,
                &self.entry_sha256,
                &self.intent_sha256,
            ]
            .iter()
            .any(|s| !valid_digest(s))
            || self.hash() != self.intent_sha256
        {
            return Err(Status::InvalidInput);
        }
        Ok(())
    }
    fn same_headers(&self, other: &Self) -> bool {
        let mut copy = other.clone();
        copy.entry_identity_sha256 = self.entry_identity_sha256.clone();
        copy.entry_sha256 = self.entry_sha256.clone();
        copy.entry_bytes = self.entry_bytes;
        copy.hash() == self.intent_sha256
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    request_version: u32,
    document: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    request_version: u32,
    operation: String,
    document: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    request_version: u32,
    intent: Intent,
}
struct Request {
    document: Uuid,
    operation: Uuid,
    original: Option<Intent>,
}
fn parse(operation: Operation, bytes: &[u8]) -> std::result::Result<Request, Status> {
    let raw = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    if !raw.is_object() {
        return Err(Status::InvalidInput);
    }
    match operation {
        Operation::Inspect => {
            let v: Selection = serde_json::from_value(raw).map_err(|_| Status::InvalidInput)?;
            if v.request_version != 1 {
                return Err(Status::InvalidInput);
            }
            let document = uuid(&v.document)?;
            Ok(Request {
                document,
                operation: document,
                original: None,
            })
        }
        Operation::Prepare => {
            let v: Preparation = serde_json::from_value(raw).map_err(|_| Status::InvalidInput)?;
            if v.request_version != 1 {
                return Err(Status::InvalidInput);
            }
            Ok(Request {
                document: uuid(&v.document)?,
                operation: uuid(&v.operation)?,
                original: None,
            })
        }
        _ => {
            if !raw.get("intent").is_some_and(|v| v.is_object()) {
                return Err(Status::InvalidInput);
            }
            let v: Confirmation = serde_json::from_value(raw).map_err(|_| Status::InvalidInput)?;
            if v.request_version != 1 {
                return Err(Status::InvalidInput);
            }
            v.intent.validate()?;
            Ok(Request {
                document: uuid(&v.intent.document)?,
                operation: uuid(&v.intent.operation)?,
                original: Some(v.intent),
            })
        }
    }
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    pub status: Status,
    runtime_unverified: bool,
    possible_effect: bool,
    document: Option<Uuid>,
    intent: Option<Intent>,
    entry_bytes: Option<u64>,
    entry_sha256: Option<String>,
    #[cfg(target_os = "linux")]
    #[serde(skip)]
    custody: Option<linux::Custody>,
}
impl Report {
    fn new(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            runtime_unverified: true,
            possible_effect: false,
            document: None,
            intent: None,
            entry_bytes: None,
            entry_sha256: None,
            #[cfg(target_os = "linux")]
            custody: None,
        }
    }
    /// Consuming output retains custody on short writes and errors too.
    pub fn write_to(self, out: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_IO, "cache report exceeds bound");
        let result = out.write_all(&bytes);
        #[cfg(target_os = "linux")]
        let _keep_until_write_finishes = &self.custody;
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
        return Report::new(operation, Status::Unsupported);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, input);
        Report::new(operation, Status::Unsupported)
    }
    #[cfg(target_os = "linux")]
    {
        let mut bytes = Vec::new();
        if input
            .take(MAX_IO as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_IO
        {
            return Report::new(operation, Status::InvalidInput);
        }
        let request = match parse(operation, &bytes) {
            Ok(v) => v,
            Err(s) => return Report::new(operation, s),
        };
        linux::run(operation, config, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_contract_vectors_and_one_field_negative_control() {
        let v: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/source_cache_cleanup_digest_vectors.json"
        ))
        .unwrap();
        for row in v["vectors"].as_array().unwrap() {
            let parts = row["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                frame(row["domain"].as_str().unwrap(), &parts),
                row["sha256"].as_str().unwrap()
            );
        }
        let mut raw = v["synthetic_intent_without_digest"].clone();
        raw["intent_sha256"] = v["vectors"][5]["sha256"].clone();
        let original: Intent = serde_json::from_value(raw).unwrap();
        original.validate().unwrap();
        let mut changed = original.clone();
        changed.entry_bytes += 1;
        assert_eq!(changed.validate(), Err(Status::InvalidInput));
        assert_ne!(original.hash(), changed.hash());
    }
    #[test]
    fn closed_objects_duplicates_versions_ids_and_intents() {
        let id = "11111111-1111-4111-8111-111111111111";
        for op in [
            Operation::Inspect,
            Operation::Prepare,
            Operation::Apply,
            Operation::Status,
        ] {
            for bytes in [
                b"[]".as_slice(),
                b"{}",
                b"{\"request_version\":1,\"request_version\":1}",
                b"{\"request_version\":1,\"intent\":[]}",
                b"null",
                b"{} {}",
                &[0xff],
            ] {
                assert!(parse(op, bytes).is_err());
            }
        }
        for bad in [
            "00000000-0000-0000-0000-000000000000",
            "11111111111141118111111111111111",
            "ABCDEFAB-1111-4111-8111-111111111111",
        ] {
            assert!(parse(
                Operation::Inspect,
                format!("{{\"request_version\":1,\"document\":\"{bad}\"}}").as_bytes()
            )
            .is_err());
        }
        assert!(parse(
            Operation::Inspect,
            format!("{{\"request_version\":1,\"document\":\"{id}\"}}").as_bytes()
        )
        .is_ok());
        assert!(parse(
            Operation::Prepare,
            format!("{{\"request_version\":1,\"document\":\"{id}\",\"operation\":\"{id}\"}}")
                .as_bytes()
        )
        .is_ok());
    }
    struct NoRead;
    impl Read for NoRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("input must not be read")
        }
    }
    #[test]
    fn nonliteral_selector_never_reads() {
        for op in [
            Operation::Inspect,
            Operation::Prepare,
            Operation::Apply,
            Operation::Status,
        ] {
            for alias in ["./-", "-/", "-/.", "not-stdin.json"] {
                assert_eq!(
                    run(op, Path::new(alias), None, &mut NoRead).status,
                    Status::Unsupported
                );
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_host_never_reads_stdin_or_config() {
        for op in [
            Operation::Inspect,
            Operation::Prepare,
            Operation::Apply,
            Operation::Status,
        ] {
            assert_eq!(
                run(
                    op,
                    Path::new("-"),
                    Some("/missing/poison.json".into()),
                    &mut NoRead
                )
                .status,
                Status::Unsupported
            );
        }
    }
}
