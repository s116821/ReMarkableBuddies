//! Metadata candidates only. No payload reads, freshness or cleanup authority.
use crate::storage::Uuid;
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{io::Write, path::PathBuf};
pub const MAX_OUTPUT: usize = 8192;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    Unavailable,
    Unsupported,
    Busy,
    Conflict,
}
#[derive(Serialize)]
pub(crate) struct Entry {
    pub document: Uuid,
    pub apparent_bytes: u64,
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: &'static str,
    pub status: Status,
    runtime_unverified: bool,
    cleanup_authority: bool,
    entry_payloads_read: bool,
    entry_payloads_validated: bool,
    source_freshness_verified: bool,
    byte_basis: &'static str,
    actor: Option<Uuid>,
    generation: Option<Uuid>,
    entries: Option<Vec<Entry>>,
    total_apparent_bytes: Option<u64>,
    #[cfg(target_os = "linux")]
    #[serde(skip)]
    custody: Option<crate::source_cache_maintenance::linux::Custody>,
}
impl Report {
    pub(crate) fn refused(status: Status) -> Self {
        Self {
            report_version: 1,
            operation: "inspect-source-cache",
            status,
            runtime_unverified: true,
            cleanup_authority: false,
            entry_payloads_read: false,
            entry_payloads_validated: false,
            source_freshness_verified: false,
            byte_basis: "apparent-file-length-per-owned-slot; no-allocated-or-reclaimable-claim",
            actor: None,
            generation: None,
            entries: None,
            total_apparent_bytes: None,
            #[cfg(target_os = "linux")]
            custody: None,
        }
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn refused_with_custody(
        status: Status,
        c: crate::source_cache_maintenance::linux::Custody,
    ) -> Self {
        let mut r = Self::refused(status);
        r.custody = Some(c);
        r
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn ready(
        actor: Uuid,
        generation: Uuid,
        entries: Vec<Entry>,
        total: u64,
        c: crate::source_cache_maintenance::linux::Custody,
    ) -> Self {
        let mut r = Self::refused_with_custody(Status::Ok, c);
        r.actor = Some(actor);
        r.generation = Some(generation);
        r.entries = Some(entries);
        r.total_apparent_bytes = Some(total);
        r
    }
    pub fn write_to(self, out: &mut impl Write) -> Result<()> {
        let mut b = serde_json::to_vec(&self)?;
        b.push(b'\n');
        ensure!(b.len() <= MAX_OUTPUT, "inventory report exceeds bound");
        let result = out.write_all(&b);
        #[cfg(target_os = "linux")]
        let _keep_until_write_finishes = &self.custody;
        result?;
        Ok(())
    }
}
/// No Read argument: a parameterless observer cannot consume stdin.
pub fn run(config: Option<PathBuf>) -> Report {
    #[cfg(target_os = "linux")]
    {
        crate::source_cache_maintenance::linux::inventory(config)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Report::refused(Status::Unsupported)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_refusal_shape_and_limits() {
        let mut b = Vec::new();
        Report::refused(Status::Unsupported)
            .write_to(&mut b)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 13);
        assert_eq!(v["operation"], "inspect-source-cache");
        assert_eq!(v["runtime_unverified"], true);
        for key in [
            "cleanup_authority",
            "entry_payloads_read",
            "entry_payloads_validated",
            "source_freshness_verified",
        ] {
            assert_eq!(v[key], false);
        }
        for key in ["actor", "generation", "entries", "total_apparent_bytes"] {
            assert!(v[key].is_null());
        }
        assert!(b.len() <= MAX_OUTPUT);
    }
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_config_is_never_admitted() {
        assert_eq!(
            run(Some("/missing/never-open-config".into())).status,
            Status::Unsupported
        );
    }
}
