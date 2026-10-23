//! Closed candidate wire. Public callers must use the custody-preserving writer.
use super::{engine::Custody, Status};
use crate::storage::{conversation_cleanup::CandidateRefusal, Uuid};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::io::Write;

const MAX_OUTPUT: usize = 8192;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum InventoryStatus {
    Complete,
    TooLarge,
    Unsupported,
    Busy,
    Unavailable,
    Conflict,
}
impl InventoryStatus {
    pub(super) fn observation(s: Status) -> Self {
        match s {
            Status::Busy => Self::Busy,
            Status::Unsupported => Self::Unsupported,
            Status::Conflict => Self::Conflict,
            _ => Self::Unavailable,
        }
    }
    pub(super) fn admission(s: Status) -> Self {
        match s {
            Status::Busy => Self::Busy,
            Status::Unsupported => Self::Unsupported,
            _ => Self::Unavailable,
        }
    }
    pub(super) fn discovery(r: CandidateRefusal) -> Self {
        match r {
            CandidateRefusal::TooLarge => Self::TooLarge,
            CandidateRefusal::Unsupported => Self::Unsupported,
            CandidateRefusal::Conflict => Self::Conflict,
            CandidateRefusal::Unavailable => Self::Unavailable,
        }
    }
}
#[derive(Serialize)]
struct Wire {
    report_version: u32,
    status: InventoryStatus,
    scope: &'static str,
    complete: bool,
    candidates: Option<Vec<String>>,
    candidate_count: Option<u8>,
    eligibility_evaluated: bool,
    mutation_authority: bool,
    native_authority: bool,
    runtime_unverified: bool,
    physical_reclamation: bool,
    local_only: bool,
    service_observation: &'static str,
}
impl Wire {
    fn refused(status: InventoryStatus) -> Self {
        Self {
            report_version: 1,
            status: if status == InventoryStatus::Complete {
                InventoryStatus::Unavailable
            } else {
                status
            },
            scope: "ordinary-current-root-candidates",
            complete: false,
            candidates: None,
            candidate_count: None,
            eligibility_evaluated: false,
            mutation_authority: false,
            native_authority: false,
            runtime_unverified: true,
            physical_reclamation: false,
            local_only: true,
            service_observation: "unverified",
        }
    }
    fn observed(ids: Vec<Uuid>) -> Self {
        if ids.len() > 16 || ids.iter().any(Uuid::is_nil) {
            return Self::refused(InventoryStatus::Unsupported);
        }
        let mut ids: Vec<_> = ids.iter().map(Uuid::to_string).collect();
        ids.sort();
        ids.dedup();
        let mut wire = Self::refused(InventoryStatus::Unavailable);
        wire.status = InventoryStatus::Complete;
        wire.complete = true;
        wire.candidate_count = Some(ids.len() as u8);
        wire.candidates = Some(ids);
        wire.service_observation = "inactive";
        wire
    }
}

/// Opaque report, deliberately not Serialize/Clone: final output revalidates custody.
pub struct InventoryReport {
    wire: Wire,
    custody: Option<Custody>,
}
impl InventoryReport {
    pub(super) fn refused(status: InventoryStatus, custody: Option<Custody>) -> Self {
        Self {
            wire: Wire::refused(status),
            custody,
        }
    }
    pub(super) fn complete(ids: Vec<Uuid>, custody: Custody) -> Self {
        Self {
            wire: Wire::observed(ids),
            custody: Some(custody),
        }
    }
    #[cfg(test)]
    pub(super) fn config_read_budget(
        &self,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicUsize>> {
        self.custody
            .as_ref()
            .and_then(Custody::inventory_read_budget)
    }
    pub fn write_to(mut self, out: &mut impl Write) -> Result<()> {
        if let Some(custody) = &mut self.custody {
            if let Err(s) = custody.check_inventory() {
                self.wire = Wire::refused(InventoryStatus::observation(s));
            }
        }
        let mut bytes = serde_json::to_vec(&self.wire)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_OUTPUT,
            "conversation inventory report bound"
        );
        let result = out.write_all(&bytes);
        let _hold_until_output_finishes = &self.custody;
        result?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_original_closed_vectors() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/conversation_inventory_report_vectors.json"
        ))
        .unwrap();
        for case in vectors["cases"].as_array().unwrap() {
            let value = &case["report"];
            let wire = match value["status"].as_str().unwrap() {
                "complete" => Wire::observed(
                    value["candidates"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| Uuid::parse_str(v.as_str().unwrap()).unwrap())
                        .collect(),
                ),
                "too-large" => Wire::refused(InventoryStatus::TooLarge),
                "unsupported" => Wire::refused(InventoryStatus::Unsupported),
                _ => panic!("unknown original vector"),
            };
            let mut encoded = serde_json::to_vec(&wire).unwrap();
            encoded.push(b'\n');
            assert_eq!(serde_json::to_value(&wire).unwrap(), *value);
            assert_eq!(std::str::from_utf8(&encoded).unwrap(), case["encoded_utf8"]);
            assert_eq!(crate::storage::digest(&encoded), case["sha256"]);
            assert_eq!(value.as_object().unwrap().len(), 13);
        }
    }
}
