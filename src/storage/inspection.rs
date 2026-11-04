//! Read-only existing-file accounting. No Store startup, recovery or effect authority.
use super::*;
use serde::de::DeserializeOwned;
use std::io;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InspectionStatus {
    Complete,
    Partial,
    Unavailable,
    Unsupported,
    Busy,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum InspectionIssue {
    ConfigurationUnavailable,
    InvalidPaths,
    MissingOwner,
    InvalidOwner,
    MissingCurrent,
    MissingLock,
    BusyWriter,
    LockUnavailable,
    LockUnsupported,
    LeaseIdentityChanged,
    UnsupportedGeneration,
    MissingEntry,
    UnreadableEntry,
    UnsafeEntry,
    UnexpectedEntry,
    EntryLimit,
    MetadataLimit,
    ByteCountOverflow,
    InvalidMetadata,
    InvalidReference,
    MissingObject,
    UninspectedReference,
    OmittedMedia,
    SizeMismatch,
    InconsistentReference,
    StateChanged,
    StateUnverifiable,
    CacheUnavailable,
    CacheOwnerMismatch,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum StorageKind {
    Structural,
    RecordMetadata,
    MediaObject,
    UnclassifiedObject,
    RetainedGeneration,
    Cache,
    Diagnostics,
    Staging,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct FileUsage {
    pub files: u64,
    pub apparent_bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct DomainUsage {
    pub observed_record_files: u64,
    pub observed_record_bytes: u64,
    /// Distinct declared references within this namespace, not an additive total.
    pub declared_media_bytes: u64,
    pub present_referenced_media_bytes: u64,
    pub missing_references: u64,
    pub uninspected_references: u64,
    pub size_mismatched_references: u64,
    pub omitted_references: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OwnedDataReport {
    pub report_version: u32,
    pub status: InspectionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_generation_format: Option<Option<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_generation_formats: Option<Vec<u32>>,
    pub owner_actor: Option<Uuid>,
    pub current_generation: Option<Uuid>,
    pub observed_files: u64,
    pub observed_apparent_bytes: u64,
    pub complete_apparent_bytes: Option<u64>,
    pub storage: BTreeMap<StorageKind, FileUsage>,
    pub observed_domains: BTreeMap<Namespace, DomainUsage>,
    pub domain_accounting_complete: bool,
    pub cache_inspected: bool,
    pub shared_media_files: u64,
    pub shared_media_apparent_bytes: u64,
    pub issues: BTreeMap<InspectionIssue, u64>,
    pub byte_basis: &'static str,
    pub reference_totals_additive: bool,
    pub media_content_verified: bool,
    pub logical_liveness_evaluated: bool,
    pub credentials_inspected: bool,
    pub native_documents_inspected: bool,
    pub lease_identity_check: &'static str,
    pub excluded_scopes: &'static [&'static str],
}
impl OwnedDataReport {
    fn empty(status: InspectionStatus) -> Self {
        Self { report_version: 1, status, current_generation_format: None, retained_generation_formats: None, owner_actor: None, current_generation: None,
            observed_files: 0, observed_apparent_bytes: 0, complete_apparent_bytes: None,
            storage: BTreeMap::new(), observed_domains: BTreeMap::new(), domain_accounting_complete: false,
            cache_inspected: false, shared_media_files: 0, shared_media_apparent_bytes: 0,
            issues: BTreeMap::new(), byte_basis: "apparent-file-length-per-owned-path; data-and-cache-only; no-allocated-or-reclaimable-claim",
            reference_totals_additive: false, media_content_verified: false, logical_liveness_evaluated: false,
            credentials_inspected: false, native_documents_inspected: false,
            lease_identity_check: if cfg!(unix) { "device-inode" } else { "metadata-bracket; no-stable-file-id-guarantee" },
            excluded_scopes: &["credentials", "native-documents", "legacy-cache", "external-transient-diagnostics"],
        }
    }
    pub fn configuration_unavailable() -> Self {
        let mut report = Self::empty(InspectionStatus::Unavailable);
        report
            .issues
            .insert(InspectionIssue::ConfigurationUnavailable, 1);
        report
    }
}

struct Budget {
    entries: usize,
    json_bytes: usize,
}
impl Budget {
    fn entry(&mut self) -> bool {
        self.entries += 1;
        self.entries <= MAX_ITEMS
    }
    fn json(&mut self, bytes: usize) -> bool {
        self.json_bytes = self.json_bytes.saturating_add(bytes);
        self.json_bytes <= MAX_METADATA
    }
}
struct FileFact {
    bytes: u64,
    kind: StorageKind,
}
struct RecordFact {
    reference: ObjectRef,
    namespace: Namespace,
    media: Vec<Media>,
}
#[derive(Default)]
struct MediaFact {
    bytes: u64,
    namespaces: BTreeSet<Namespace>,
    omitted: BTreeSet<Namespace>,
    included: BTreeSet<Namespace>,
}
struct Scan {
    report: OwnedDataReport,
    budget: Budget,
    files: BTreeMap<PathBuf, FileFact>,
    records: BTreeMap<(Uuid, String), RecordFact>,
    media: BTreeMap<(Uuid, String), MediaFact>,
    generations: BTreeMap<Uuid, PathBuf>,
    history_references: BTreeSet<(Uuid, String)>,
}
impl Scan {
    fn issue(&mut self, code: InspectionIssue) {
        *self.report.issues.entry(code).or_default() += 1;
    }
    fn json<T: DeserializeOwned>(&mut self, path: &Path, limit: usize) -> Option<T> {
        let bytes = self.read_metadata(path, limit)?;
        match serde_json::from_slice(&bytes) {
            Ok(value) => Some(value),
            Err(_) => {
                self.issue(InspectionIssue::InvalidMetadata);
                None
            }
        }
    }
    fn read_metadata(&mut self, path: &Path, limit: usize) -> Option<Vec<u8>> {
        let metadata = self.metadata(path)?;
        if !metadata.is_file() {
            self.issue(InspectionIssue::UnsafeEntry);
            return None;
        }
        if metadata.len() > limit as u64 {
            self.issue(InspectionIssue::InvalidMetadata);
            return None;
        }
        let remaining = MAX_METADATA.saturating_sub(self.budget.json_bytes);
        if remaining == 0 {
            self.issue(InspectionIssue::MetadataLimit);
            return None;
        }
        if metadata.len() > remaining as u64 {
            self.issue(InspectionIssue::MetadataLimit);
            return None;
        }
        let bytes = match files::read(path, limit.min(remaining) as u64) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.issue(InspectionIssue::UnreadableEntry);
                return None;
            }
        };
        if !self.budget.json(bytes.len()) {
            self.issue(InspectionIssue::MetadataLimit);
            return None;
        }
        Some(bytes)
    }
    fn entries(&mut self, root: &Path) -> Vec<PathBuf> {
        if self.budget.entries >= MAX_ITEMS {
            self.issue(InspectionIssue::EntryLimit);
            return vec![];
        }
        if files::safe_path(root).is_err() {
            self.issue(InspectionIssue::UnsafeEntry);
            return vec![];
        }
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => {
                self.issue(InspectionIssue::UnreadableEntry);
                return vec![];
            }
        };
        let mut paths = vec![];
        for entry in entries {
            if !self.budget.entry() {
                self.issue(InspectionIssue::EntryLimit);
                break;
            }
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(_) => self.issue(InspectionIssue::UnreadableEntry),
            }
        }
        paths.sort();
        paths
    }
    fn metadata(&mut self, path: &Path) -> Option<fs::Metadata> {
        if files::safe_path(path).is_err() {
            self.issue(InspectionIssue::UnsafeEntry);
            return None;
        }
        match fs::symlink_metadata(path) {
            Ok(meta) => Some(meta),
            Err(e) => {
                self.issue(if e.kind() == io::ErrorKind::NotFound {
                    InspectionIssue::MissingEntry
                } else {
                    InspectionIssue::UnreadableEntry
                });
                None
            }
        }
    }
    fn file(&mut self, path: &Path, kind: StorageKind) {
        if self.files.contains_key(path) {
            return;
        }
        if let Some(meta) = self.metadata(path) {
            if meta.is_file() {
                self.files.insert(
                    path.to_owned(),
                    FileFact {
                        bytes: meta.len(),
                        kind,
                    },
                );
            } else {
                self.issue(InspectionIssue::UnexpectedEntry);
            }
        }
    }
    fn opaque_tree(&mut self, root: &Path, kind: StorageKind) {
        let mut pending = vec![root.to_owned()];
        while let Some(directory) = pending.pop() {
            for path in self.entries(&directory) {
                if let Some(meta) = self.metadata(&path) {
                    if meta.is_dir() {
                        pending.push(path);
                    } else if meta.is_file() {
                        self.file(&path, kind);
                    } else {
                        self.issue(InspectionIssue::UnexpectedEntry);
                    }
                }
            }
            if self.budget.entries > MAX_ITEMS {
                break;
            }
        }
    }
    fn record_ref(
        &mut self,
        generation: Uuid,
        reference: &ObjectRef,
        namespace: Namespace,
        media: Vec<Media>,
    ) {
        let key = (generation, reference.sha256.clone());
        if let Some(prior) = self.records.get(&key) {
            if prior.reference != *reference || prior.namespace != namespace {
                self.issue(InspectionIssue::InconsistentReference);
            }
        } else if self.records.len() + self.media.len() < MAX_ITEMS {
            self.records.insert(
                key,
                RecordFact {
                    reference: reference.clone(),
                    namespace,
                    media,
                },
            );
        } else {
            self.issue(InspectionIssue::EntryLimit);
        }
    }
    fn media_ref(&mut self, generation: Uuid, media: &Media, namespace: Namespace, omitted: bool) {
        let key = (generation, media.sha256.clone());
        if !self.media.contains_key(&key) && self.records.len() + self.media.len() >= MAX_ITEMS {
            self.issue(InspectionIssue::EntryLimit);
            return;
        }
        let item = self.media.entry(key).or_insert_with(|| MediaFact {
            bytes: media.bytes,
            ..Default::default()
        });
        if item.bytes != media.bytes {
            self.issue(InspectionIssue::InconsistentReference);
            return;
        }
        item.namespaces.insert(namespace);
        if omitted {
            item.omitted.insert(namespace);
        } else {
            item.included.insert(namespace);
        }
    }
    fn manifest(&mut self, generation: Uuid, root: &Path, manifest: &Manifest) {
        if manifest.validate().is_err() {
            self.issue(InspectionIssue::InvalidMetadata);
            return;
        }
        let mut descriptors = BTreeMap::new();
        for reference in &manifest.records {
            let Some(namespace) = manifest.record_namespaces.get(&reference.sha256).copied() else {
                self.issue(InspectionIssue::InvalidReference);
                continue;
            };
            if namespace == Namespace::UnassociatedFeedback {
                self.report.report_version = 2;
                self.report.current_generation_format.get_or_insert(None);
                self.report
                    .retained_generation_formats
                    .get_or_insert_with(Vec::new);
            }
            let key = (generation, reference.sha256.clone());
            let descriptor_media = if let Some(prior) = self.records.get(&key) {
                if prior.reference != *reference || prior.namespace != namespace {
                    self.issue(InspectionIssue::InconsistentReference);
                    continue;
                }
                prior.media.clone()
            } else {
                let path = root.join("objects").join(&reference.sha256);
                if reference.bytes > MAX_RECORD as u64 {
                    self.issue(InspectionIssue::InvalidReference);
                    continue;
                }
                let bytes = match self.read_metadata(&path, MAX_RECORD) {
                    Some(bytes) => bytes,
                    None => {
                        continue;
                    }
                };
                if bytes.len() as u64 != reference.bytes || digest(&bytes) != reference.sha256 {
                    self.issue(InspectionIssue::InvalidReference);
                    continue;
                }
                let envelope = match serde_json::from_slice::<Envelope>(&bytes) {
                    Ok(envelope) => envelope,
                    Err(_) => {
                        self.issue(InspectionIssue::InvalidMetadata);
                        continue;
                    }
                };
                let feedback_invalid = namespace == Namespace::UnassociatedFeedback
                    && (self.json::<u32>(&root.join("format.json"), 64)
                        != Some(feedback_generation::VERSION)
                        || crate::unassociated_feedback::validate_envelope(
                            &envelope,
                            self.report.owner_actor.unwrap_or_default(),
                        )
                        .is_err());
                if envelope.validate().is_err()
                    || envelope.namespace != namespace
                    || feedback_invalid
                {
                    self.issue(InspectionIssue::InvalidReference);
                    continue;
                }
                self.record_ref(
                    generation,
                    reference,
                    namespace,
                    envelope.media_descriptors.clone(),
                );
                envelope.media_descriptors
            };
            for media in &descriptor_media {
                if descriptors
                    .insert(media.sha256.clone(), media.clone())
                    .is_some_and(|old| old != *media)
                {
                    self.issue(InspectionIssue::InconsistentReference);
                }
                let coverage = manifest
                    .media_coverage
                    .iter()
                    .find(|coverage| coverage.hash() == media.sha256);
                if coverage.is_none() {
                    self.issue(InspectionIssue::InvalidReference);
                }
                self.media_ref(
                    generation,
                    media,
                    namespace,
                    matches!(coverage, Some(Coverage::Omitted { .. })),
                );
            }
        }
        if descriptors
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != manifest.media_coverage.iter().map(Coverage::hash).collect()
        {
            self.issue(InspectionIssue::InvalidReference);
        }
        for included in &manifest.media {
            if !descriptors
                .get(&included.sha256)
                .is_some_and(|descriptor| descriptor.bytes == included.bytes)
            {
                self.issue(InspectionIssue::InvalidReference);
            }
        }
    }
    fn generation(&mut self, root: &Path, generation: Uuid, actor: Uuid, current: bool) {
        let Some(version) = self.json::<u32>(&root.join("format.json"), 64) else {
            return;
        };
        let feedback_feature = root.join(feedback_generation::MARKER);
        if version == feedback_generation::VERSION
            || fs::symlink_metadata(&feedback_feature).is_ok()
        {
            self.report.report_version = 2;
            self.report.current_generation_format.get_or_insert(None);
            self.report
                .retained_generation_formats
                .get_or_insert_with(Vec::new);
        }
        if self.report.report_version == 2 && current {
            self.report.current_generation_format =
                Some(matches!(version, 1..=3).then_some(version));
        }
        // Account the existing pure validator's repeated bounded metadata reads.
        let feature = root.join("selection-feature.json");
        let repeated = fs::symlink_metadata(&feature).map_or(64, |meta| {
            64 + usize::try_from(meta.len()).unwrap_or(MAX_METADATA)
        });
        let feedback_repeated = fs::symlink_metadata(&feedback_feature).map_or(0, |meta| {
            usize::try_from(meta.len()).unwrap_or(MAX_METADATA)
        });
        if !self.budget.json(repeated.saturating_add(feedback_repeated)) {
            self.issue(InspectionIssue::MetadataLimit);
            return;
        }
        if !matches!(version, 1..=3)
            || selection::validate_generation(root, actor, generation).is_err()
        {
            self.issue(InspectionIssue::UnsupportedGeneration);
            if current {
                self.report.status = InspectionStatus::Unsupported;
            }
            return;
        }
        if !current && self.report.report_version == 2 {
            let formats = self
                .report
                .retained_generation_formats
                .get_or_insert_with(Vec::new);
            if !formats.contains(&version) {
                formats.push(version);
                formats.sort_unstable();
            }
        }
        self.generations.insert(generation, root.to_owned());
        for required in ["objects", "commits"] {
            if !self
                .metadata(&root.join(required))
                .is_some_and(|metadata| metadata.is_dir())
            {
                self.issue(InspectionIssue::MissingEntry);
            }
        }
        for path in self.entries(root) {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            match name {
                "format.json"
                | "selection-feature.json"
                | "feedback-feature.json"
                | "restored-config.json" => self.file(
                    &path,
                    if current {
                        StorageKind::Structural
                    } else {
                        StorageKind::RetainedGeneration
                    },
                ),
                "objects" => {
                    for object in self.entries(&path) {
                        if object
                            .file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(valid_digest)
                        {
                            self.file(
                                &object,
                                if current {
                                    StorageKind::UnclassifiedObject
                                } else {
                                    StorageKind::RetainedGeneration
                                },
                            );
                        } else if stage_name(&object) {
                            self.file(&object, StorageKind::Staging);
                        } else {
                            self.issue(InspectionIssue::UnexpectedEntry);
                        }
                    }
                }
                "commits" | "selections" | "selection-history" => {
                    for item in self.entries(&path) {
                        if stage_name(&item) {
                            self.file(&item, StorageKind::Staging);
                            continue;
                        }
                        let filename = item.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        let recognized = match name {
                            "commits" => filename.strip_suffix(".json").is_some_and(uuid_name),
                            "selections" => {
                                filename.strip_suffix(".json").is_some_and(valid_digest)
                            }
                            _ => valid_digest(filename),
                        };
                        if !recognized {
                            self.issue(InspectionIssue::UnexpectedEntry);
                            continue;
                        }
                        self.file(
                            &item,
                            if current {
                                StorageKind::Structural
                            } else {
                                StorageKind::RetainedGeneration
                            },
                        );
                        if name == "commits" {
                            if let Some(manifest) = self.json::<Manifest>(&item, MAX_METADATA) {
                                if manifest.transaction_id.to_string() + ".json" != filename {
                                    self.issue(InspectionIssue::InvalidReference);
                                } else {
                                    self.manifest(generation, root, &manifest);
                                }
                            }
                        } else if let Some(transaction) =
                            self.json::<selection::SelectionTransaction>(&item, MAX_METADATA)
                        {
                            if transaction.validate(MAX_ITEMS).is_err()
                                || transaction.store_generation != generation
                            {
                                self.issue(InspectionIssue::InvalidMetadata);
                                continue;
                            }
                            let Some(bytes) = self.read_metadata(&item, MAX_METADATA) else {
                                continue;
                            };
                            let transaction_hash = digest(&bytes);
                            if name == "selection-history" {
                                if digest(&bytes) != filename {
                                    self.issue(InspectionIssue::InvalidReference);
                                    continue;
                                }
                            } else if digest(
                                &serde_json::to_vec(&transaction.scope).unwrap_or_default(),
                            ) + ".json"
                                != filename
                            {
                                self.issue(InspectionIssue::InvalidReference);
                                continue;
                            }
                            if self.history_references.len() < MAX_ITEMS {
                                if name == "selection-history" {
                                    self.history_references
                                        .insert((generation, transaction_hash));
                                }
                                if let Some(previous) = &transaction.previous_sha256 {
                                    self.history_references
                                        .insert((generation, previous.clone()));
                                }
                            } else {
                                self.issue(InspectionIssue::EntryLimit);
                            }
                            for manifest in
                                std::iter::once(&transaction.selected).chain(&transaction.retained)
                            {
                                self.manifest(generation, root, manifest);
                            }
                        }
                    }
                }
                _ if stage_name(&path) => self.file(&path, StorageKind::Staging),
                _ => self.issue(InspectionIssue::UnexpectedEntry),
            }
        }
    }
    fn finish(mut self, current: Uuid) -> OwnedDataReport {
        if !self.generations.contains_key(&current) {
            self.issue(InspectionIssue::MissingEntry);
        }
        let mut overflow = false;
        let traversal_incomplete = [
            InspectionIssue::EntryLimit,
            InspectionIssue::MetadataLimit,
            InspectionIssue::UnsafeEntry,
            InspectionIssue::UnreadableEntry,
            InspectionIssue::UnexpectedEntry,
        ]
        .iter()
        .any(|issue| self.report.issues.contains_key(issue));
        for (generation, hash) in &self.history_references {
            if !self.generations.get(generation).is_some_and(|root| {
                self.files
                    .contains_key(&root.join("selection-history").join(hash))
            }) {
                *self
                    .report
                    .issues
                    .entry(if traversal_incomplete {
                        InspectionIssue::UninspectedReference
                    } else {
                        InspectionIssue::MissingEntry
                    })
                    .or_default() += 1;
            }
        }
        for ((generation, hash), record) in &self.records {
            let reference = &record.reference;
            let namespace = &record.namespace;
            let Some(root) = self.generations.get(generation) else {
                continue;
            };
            let path = root.join("objects").join(hash);
            let domain = self.report.observed_domains.entry(*namespace).or_default();
            if let Some(file) = self.files.get_mut(&path) {
                if file.bytes == reference.bytes {
                    domain.observed_record_files += 1;
                    domain.observed_record_bytes += file.bytes;
                    if *generation == current {
                        file.kind = StorageKind::RecordMetadata;
                    }
                } else {
                    *self
                        .report
                        .issues
                        .entry(InspectionIssue::SizeMismatch)
                        .or_default() += 1;
                }
            } else {
                *self
                    .report
                    .issues
                    .entry(if traversal_incomplete {
                        InspectionIssue::UninspectedReference
                    } else {
                        InspectionIssue::MissingObject
                    })
                    .or_default() += 1;
            }
        }
        for ((generation, hash), media) in &self.media {
            let Some(root) = self.generations.get(generation) else {
                continue;
            };
            let path = root.join("objects").join(hash);
            let present = self.files.get(&path).map(|file| file.bytes);
            let matching = present == Some(media.bytes);
            if present.is_some() && !matching {
                *self
                    .report
                    .issues
                    .entry(InspectionIssue::SizeMismatch)
                    .or_default() += 1;
            }
            if matching && media.namespaces.len() > 1 {
                self.report.shared_media_files += 1;
                self.report.shared_media_apparent_bytes += media.bytes;
            }
            if *generation == current && !self.records.contains_key(&(*generation, hash.clone())) {
                if let Some(file) = self.files.get_mut(&path) {
                    file.kind = StorageKind::MediaObject;
                }
            }
            for namespace in &media.namespaces {
                let domain = self.report.observed_domains.entry(*namespace).or_default();
                domain.declared_media_bytes += media.bytes;
                if let Some(bytes) = present {
                    add_bytes(
                        &mut domain.present_referenced_media_bytes,
                        bytes,
                        &mut overflow,
                    );
                    if !matching {
                        domain.size_mismatched_references += 1;
                    }
                } else {
                    if traversal_incomplete {
                        domain.uninspected_references += 1;
                    } else {
                        domain.missing_references += 1;
                    }
                }
                if media.omitted.contains(namespace) {
                    domain.omitted_references += 1;
                }
            }
            if !media.omitted.is_empty() {
                *self
                    .report
                    .issues
                    .entry(InspectionIssue::OmittedMedia)
                    .or_default() += 1;
            }
            if present.is_none() && !media.included.is_empty() {
                *self
                    .report
                    .issues
                    .entry(if traversal_incomplete {
                        InspectionIssue::UninspectedReference
                    } else {
                        InspectionIssue::MissingObject
                    })
                    .or_default() += 1;
            }
        }
        for file in self.files.values() {
            self.report.observed_files += 1;
            add_bytes(
                &mut self.report.observed_apparent_bytes,
                file.bytes,
                &mut overflow,
            );
            let kind = self.report.storage.entry(file.kind).or_default();
            kind.files += 1;
            add_bytes(&mut kind.apparent_bytes, file.bytes, &mut overflow);
        }
        if overflow {
            self.issue(InspectionIssue::ByteCountOverflow);
        }
        if self.report.report_version == 2 {
            let current_root = self
                .report
                .current_generation
                .and_then(|id| self.generations.get(&id).cloned());
            let current_format = current_root
                .as_ref()
                .and_then(|root| self.json::<u32>(&root.join("format.json"), 64));
            self.report.current_generation_format =
                Some(current_format.filter(|v| matches!(v, 1..=3)));
            let roots: Vec<_> = self
                .generations
                .iter()
                .filter(|(id, _)| Some(**id) != self.report.current_generation)
                .map(|(_, p)| p.clone())
                .collect();
            let mut formats = BTreeSet::new();
            for root in roots {
                if let Some(version) = self.json::<u32>(&root.join("format.json"), 64) {
                    if matches!(version, 1..=3) {
                        formats.insert(version);
                    }
                }
            }
            self.report.retained_generation_formats = Some(formats.into_iter().collect());
            if current_format != Some(feedback_generation::VERSION) {
                self.issue(InspectionIssue::UnsupportedGeneration);
            }
        }
        self.report.status = if self.report.status == InspectionStatus::Unsupported {
            InspectionStatus::Unsupported
        } else if self.report.issues.is_empty() {
            InspectionStatus::Complete
        } else {
            InspectionStatus::Partial
        };
        self.report.domain_accounting_complete = self.report.status == InspectionStatus::Complete;
        if self.report.domain_accounting_complete {
            self.report.complete_apparent_bytes = Some(self.report.observed_apparent_bytes);
            if self.report.report_version == 2 {
                self.report
                    .observed_domains
                    .entry(Namespace::UnassociatedFeedback)
                    .or_default();
            }
            for namespace in [
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
                Namespace::SubjectMemory,
                Namespace::Handwriting,
            ] {
                self.report.observed_domains.entry(namespace).or_default();
            }
        }
        self.report
    }
}

fn uuid_name(name: &str) -> bool {
    Uuid::parse_str(name).is_ok_and(|id| !id.is_nil() && id.to_string() == name)
}
fn add_bytes(total: &mut u64, bytes: u64, overflow: &mut bool) {
    match total.checked_add(bytes) {
        Some(value) => *total = value,
        None => {
            *total = u64::MAX;
            *overflow = true;
        }
    }
}
fn stage_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix(".stage-"))
        .is_some_and(uuid_name)
}
fn same_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        a.is_file() == b.is_file()
            && a.len() == b.len()
            && a.created().ok() == b.created().ok()
            && a.modified().ok() == b.modified().ok()
    }
}
fn canonical_roots_disjoint(paths: &StorePaths) -> bool {
    fn canonical(path: &Path) -> Option<PathBuf> {
        if path.exists() {
            return path.canonicalize().ok();
        }
        let parent = canonical(path.parent()?)?;
        Some(parent.join(path.file_name()?))
    }
    let roots: Option<Vec<_>> = [&paths.data, &paths.cache, &paths.credentials]
        .into_iter()
        .map(|path| canonical(path))
        .collect();
    let Some(roots) = roots else {
        return false;
    };
    for (index, root) in roots.iter().enumerate() {
        for other in &roots[index + 1..] {
            if root.starts_with(other) || other.starts_with(root) {
                return false;
            }
        }
    }
    true
}

/// The only admission is a read-only existing-file lease. No new Store is built.
pub fn inspect_owned_data(paths: &StorePaths) -> OwnedDataReport {
    let mut scan = Scan {
        report: OwnedDataReport::empty(InspectionStatus::Unavailable),
        budget: Budget {
            entries: 0,
            json_bytes: 0,
        },
        files: BTreeMap::new(),
        records: BTreeMap::new(),
        media: BTreeMap::new(),
        generations: BTreeMap::new(),
        history_references: BTreeSet::new(),
    };
    if paths.validate().is_err() || !canonical_roots_disjoint(paths) {
        scan.report.status = InspectionStatus::Unsupported;
        scan.issue(InspectionIssue::InvalidPaths);
        return scan.report;
    }
    let identity_path = paths.data.join("identity.json");
    if !identity_path.exists() {
        scan.issue(InspectionIssue::MissingOwner);
        return scan.report;
    }
    let Some(identity) = scan.json::<Identity>(&identity_path, MAX_RECORD) else {
        scan.issue(InspectionIssue::InvalidOwner);
        return scan.report;
    };
    if identity.format != FORMAT || identity.actor.is_nil() {
        scan.report.status = InspectionStatus::Unsupported;
        scan.issue(InspectionIssue::InvalidOwner);
        return scan.report;
    }
    scan.report.owner_actor = Some(identity.actor);
    let lock_path = paths.data.join("store.lock");
    if files::safe_path(&lock_path).is_err() {
        scan.report.status = InspectionStatus::Unsupported;
        scan.issue(InspectionIssue::UnsafeEntry);
        return scan.report;
    }
    let Some(lock_metadata) = scan.metadata(&lock_path) else {
        scan.issue(InspectionIssue::MissingLock);
        return scan.report;
    };
    if !lock_metadata.is_file() {
        scan.report.status = InspectionStatus::Unsupported;
        scan.issue(InspectionIssue::UnsafeEntry);
        return scan.report;
    }
    let lease = match File::open(&lock_path) {
        Ok(file) => file,
        Err(_) => {
            scan.issue(InspectionIssue::MissingLock);
            return scan.report;
        }
    };
    match lease.try_lock() {
        Ok(()) => (),
        Err(fs::TryLockError::WouldBlock) => {
            scan.report.status = InspectionStatus::Busy;
            scan.issue(InspectionIssue::BusyWriter);
            return scan.report;
        }
        Err(fs::TryLockError::Error(error)) => {
            scan.report.status = if error.kind() == io::ErrorKind::Unsupported {
                InspectionStatus::Unsupported
            } else {
                InspectionStatus::Unavailable
            };
            scan.issue(if error.kind() == io::ErrorKind::Unsupported {
                InspectionIssue::LockUnsupported
            } else {
                InspectionIssue::LockUnavailable
            });
            return scan.report;
        }
    }
    let (Ok(handle), Ok(named)) = (lease.metadata(), fs::symlink_metadata(&lock_path)) else {
        scan.issue(InspectionIssue::LeaseIdentityChanged);
        return scan.report;
    };
    if !handle.is_file() || !same_identity(&handle, &named) {
        scan.issue(InspectionIssue::LeaseIdentityChanged);
        return scan.report;
    }
    let current_path = paths.data.join("CURRENT");
    let Some(current) = scan.json::<Uuid>(&current_path, 128) else {
        scan.issue(InspectionIssue::MissingCurrent);
        return scan.report;
    };
    if current.is_nil() {
        scan.issue(InspectionIssue::MissingCurrent);
        return scan.report;
    }
    scan.report.current_generation = Some(current);
    for path in scan.entries(&paths.data) {
        match path.file_name().and_then(|s| s.to_str()).unwrap_or("") {
            "identity.json"
            | "CURRENT"
            | "store.lock"
            | "capabilities.json"
            | "contract-v1.json"
            | "contract-v2.json"
            | "config.snapshot.json"
            | "effective-config.json" => scan.file(&path, StorageKind::Structural),
            "generations" => {
                for generation in scan.entries(&path) {
                    let name = generation
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if uuid_name(name) {
                        scan.generation(
                            &generation,
                            Uuid::parse_str(name).unwrap(),
                            identity.actor,
                            name == current.to_string(),
                        );
                    } else {
                        scan.issue(InspectionIssue::UnexpectedEntry);
                    }
                }
            }
            "quarantine" => scan.opaque_tree(&path, StorageKind::Diagnostics),
            "sync" => {
                for item in scan.entries(&path) {
                    match item.file_name().and_then(|s| s.to_str()).unwrap_or("") {
                        "state.json" | "health.json" => scan.file(&item, StorageKind::Structural),
                        "incoming" => scan.opaque_tree(&item, StorageKind::Staging),
                        _ if stage_name(&item) => scan.file(&item, StorageKind::Staging),
                        _ => scan.issue(InspectionIssue::UnexpectedEntry),
                    }
                }
            }
            _ if stage_name(&path) => scan.file(&path, StorageKind::Staging),
            _ => scan.issue(InspectionIssue::UnexpectedEntry),
        }
    }
    if paths.cache.exists() {
        let owner = scan.json::<Identity>(&paths.cache.join("owner.json"), MAX_RECORD);
        if owner
            .as_ref()
            .is_some_and(|owner| owner.format == FORMAT && owner.actor == identity.actor)
        {
            scan.report.cache_inspected = true;
            scan.opaque_tree(&paths.cache, StorageKind::Cache);
        } else {
            scan.issue(if owner.is_some() {
                InspectionIssue::CacheOwnerMismatch
            } else {
                InspectionIssue::CacheUnavailable
            });
        }
    } else {
        scan.issue(InspectionIssue::CacheUnavailable);
    }
    let after_owner = scan.json::<Identity>(&identity_path, MAX_RECORD);
    let after_current = scan.json::<Uuid>(&current_path, 128);
    if after_owner.is_none() || after_current.is_none() {
        scan.issue(InspectionIssue::StateUnverifiable);
    } else if !after_owner
        .is_some_and(|owner| owner.format == identity.format && owner.actor == identity.actor)
        || after_current != Some(current)
    {
        scan.issue(InspectionIssue::StateChanged);
    }
    if !fs::symlink_metadata(&lock_path).is_ok_and(|after| same_identity(&handle, &after)) {
        scan.issue(InspectionIssue::LeaseIdentityChanged);
    }
    let report = scan.finish(current);
    let _ = lease.unlock();
    report
}
