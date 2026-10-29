//! Separate report3 wire. Released report1/2 types are not modified.
use crate::storage::{inspection::{InspectionStatus, StorageKind}, Namespace};
use serde::{Serialize, Serializer};
use std::{collections::BTreeMap, io::{self, Write}};
use uuid::Uuid;

pub(super) const MAX_WIRE: usize = 32768;
pub(super) const ISSUE_CODES: &[&str] = &["configuration-unavailable", "invalid-paths", "missing-owner", "invalid-owner", "missing-current", "missing-lock", "busy-writer", "lock-unavailable", "lock-unsupported", "lease-identity-changed", "unsupported-generation", "missing-entry", "unreadable-entry", "unsafe-entry", "unexpected-entry", "entry-limit", "metadata-limit", "byte-count-overflow", "invalid-metadata", "invalid-reference", "missing-object", "uninspected-reference", "omitted-media", "size-mismatch", "inconsistent-reference", "state-changed", "state-unverifiable", "cache-unavailable", "cache-owner-mismatch", "unsupported-platform", "report-limit", "unclassified-object"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Count(pub u64);
impl Serialize for Count {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// At most4096 bounded file/reference lengths are aggregated. u128 retains the
/// exact sum; only a representable u64 is emitted, otherwise null, never saturation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Bytes(pub u128);
impl Bytes {
    pub(super) fn add(&mut self, value: u64) { self.0 += u128::from(value); }
    pub(super) fn value(self) -> Option<u64> { u64::try_from(self.0).ok() }
}
impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.value() {
            Some(value) => serializer.serialize_str(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct FileUsage { pub files: Count, pub apparent_bytes: Bytes }
impl FileUsage {
    pub(super) fn add(&mut self, bytes: u64) {
        self.files.0 += 1;
        self.apparent_bytes.add(bytes);
    }
}
#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct DomainUsage {
    pub observed_record_files: Count,
    pub observed_record_bytes: Bytes,
    pub declared_media_bytes: Bytes,
    pub present_referenced_media_bytes: Bytes,
    pub missing_references: Count,
    pub uninspected_references: Count,
    pub size_mismatched_references: Count,
    pub omitted_references: Count,
}
impl DomainUsage {
    pub(super) fn overflowed(&self) -> bool {
        [self.observed_record_bytes, self.declared_media_bytes, self.present_referenced_media_bytes]
            .iter().any(|bytes| bytes.value().is_none())
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all="kebab-case")]
pub(super) enum NotebookKind { ArchivedNormalizedSource, NotebookPlan, NotebookStage }

#[derive(Clone, Debug, Serialize)]
pub(super) struct Report {
    pub report_version: u32,
    pub status: InspectionStatus,
    pub owner_actor: Option<Uuid>,
    pub current_generation: Option<Uuid>,
    pub current_generation_format: Option<u32>,
    pub retained_generation_formats: Vec<u32>,
    pub observed_files: Count,
    pub observed_apparent_bytes: Bytes,
    pub complete_apparent_bytes: Option<Count>,
    pub storage: BTreeMap<StorageKind, FileUsage>,
    pub observed_domains: BTreeMap<Namespace, DomainUsage>,
    pub observed_notebook_records: BTreeMap<NotebookKind, FileUsage>,
    pub domain_accounting_complete: bool,
    pub cache_inspected: bool,
    pub shared_media_files: Count,
    pub shared_media_apparent_bytes: Bytes,
    pub issues: BTreeMap<&'static str, Count>,
    pub byte_basis: &'static str,
    pub reference_totals_additive: bool,
    pub notebook_record_totals_additive: bool,
    pub media_content_verified: bool,
    pub logical_liveness_evaluated: bool,
    pub credentials_inspected: bool,
    pub native_documents_inspected: bool,
    pub lease_identity_check: &'static str,
    pub excluded_scopes: [&'static str; 4],
}
impl Report {
    pub(super) fn empty(status: InspectionStatus) -> Self {
        Self {
            report_version: 3, status, owner_actor: None, current_generation: None,
            current_generation_format: None, retained_generation_formats: Vec::new(),
            observed_files: Count::default(), observed_apparent_bytes: Bytes::default(),
            complete_apparent_bytes: None, storage: BTreeMap::new(), observed_domains: BTreeMap::new(),
            observed_notebook_records: BTreeMap::new(), domain_accounting_complete: false,
            cache_inspected: false, shared_media_files: Count::default(), shared_media_apparent_bytes: Bytes::default(),
            issues: BTreeMap::new(), byte_basis: "apparent-file-length-per-owned-path; data-and-cache-only; no-allocated-or-reclaimable-claim",
            reference_totals_additive: false, notebook_record_totals_additive: false,
            media_content_verified: false, logical_liveness_evaluated: false,
            credentials_inspected: false, native_documents_inspected: false,
            lease_identity_check: "unavailable",
            excluded_scopes: ["credentials", "native-documents", "legacy-cache", "external-transient-diagnostics"],
        }
    }
    pub(super) fn issue(&mut self, code: &'static str) {
        assert!(ISSUE_CODES.contains(&code), "unknown fixed report3 issue code");
        self.issues.entry(code).or_default().0 += 1;
    }
    pub(super) fn failure(status: InspectionStatus, code: &'static str) -> Self {
        let mut report = Self::empty(status);
        report.issue(code);
        report
    }
    pub(super) fn overflowed(&self) -> bool {
        self.observed_apparent_bytes.value().is_none() || self.shared_media_apparent_bytes.value().is_none()
            || self.storage.values().any(|usage| usage.apparent_bytes.value().is_none())
            || self.observed_domains.values().any(DomainUsage::overflowed)
            || self.observed_notebook_records.values().any(|usage| usage.apparent_bytes.value().is_none())
    }
    pub(super) fn write_to(&self, output: &mut impl Write) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(self).map_err(|_| io::Error::other("report3 encoding unavailable"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_WIRE { return Err(io::Error::other("report3 output bound")); }
        output.write_all(&bytes)
    }
}
