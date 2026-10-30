use super::{
    linux::{Access, Failure},
    report::*,
    Inspection,
};
use crate::{
    conversation::{Ledger, Record},
    storage::{
        inspection::{InspectionStatus, StorageKind},
        *,
    },
};
use serde::de::DeserializeOwned;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::Metadata,
    path::{Path, PathBuf},
};

struct FileFact {
    bytes: u64,
    kind: StorageKind,
}
struct RecordFact {
    namespace: Namespace,
    bytes: u64,
    media: Vec<Media>,
    notebook: Option<NotebookKind>,
}
#[derive(Default)]
struct MediaFact {
    bytes: u64,
    namespaces: BTreeSet<Namespace>,
    omitted: BTreeSet<Namespace>,
}
struct Scan {
    report: Report,
    access: Access,
    entries: usize,
    read_bytes: usize,
    files: BTreeMap<PathBuf, FileFact>,
    records: BTreeMap<(Uuid, String), RecordFact>,
    media: BTreeMap<(Uuid, String), MediaFact>,
    generations: BTreeMap<Uuid, PathBuf>,
    history: BTreeSet<(Uuid, String)>,
}
impl Scan {
    fn issue(&mut self, code: &'static str) {
        self.report.issue(code);
    }
    fn failure(&mut self, error: Failure) {
        self.issue(match error {
            Failure::Missing => "missing-entry",
            Failure::Unsafe => "unsafe-entry",
            Failure::Unreadable => "unreadable-entry",
            Failure::Changed => "state-changed",
            Failure::TooLarge => "metadata-limit",
            Failure::WorkLimit => "entry-limit",
        });
    }
    fn meta(&mut self, path: &Path) -> Option<Metadata> {
        match self.access.metadata(path) {
            Ok(m) => Some(m),
            Err(e) => {
                self.failure(e);
                None
            }
        }
    }
    fn exists(&mut self, path: &Path) -> bool {
        match self.access.exists(path) {
            Ok(v) => v,
            Err(e) => {
                self.failure(e);
                false
            }
        }
    }
    fn entries(&mut self, path: &Path) -> Vec<PathBuf> {
        let remaining = MAX_ITEMS.saturating_sub(self.entries);
        if remaining == 0 {
            self.issue("entry-limit");
            return vec![];
        }
        match self.access.entries(path, remaining) {
            Ok(v) => {
                self.entries += v.len();
                v
            }
            Err(e) => {
                // A failing enumeration may already have visited its entire
                // remaining allowance. Never renew that allowance elsewhere.
                self.entries = MAX_ITEMS;
                self.failure(e);
                vec![]
            }
        }
    }
    fn read(&mut self, path: &Path, limit: usize) -> Option<Vec<u8>> {
        let remaining = MAX_METADATA.saturating_sub(self.read_bytes);
        if remaining == 0 {
            self.issue("metadata-limit");
            return None;
        }
        let (result, charged) = self.access.read(path, limit.min(remaining));
        self.read_bytes += charged;
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.failure(e);
                None
            }
        }
    }
    fn json<T: DeserializeOwned>(&mut self, path: &Path, limit: usize) -> Option<T> {
        let bytes = self.read(path, limit)?;
        match serde_json::from_slice(&bytes) {
            Ok(v) => Some(v),
            Err(_) => {
                self.issue("invalid-metadata");
                None
            }
        }
    }
    fn file(&mut self, path: &Path, kind: StorageKind) {
        if self.files.contains_key(path) {
            return;
        }
        if self.files.len() >= MAX_ITEMS {
            self.issue("entry-limit");
            return;
        }
        if let Some(meta) = self.meta(path) {
            if meta.is_file() {
                self.files.insert(
                    path.to_owned(),
                    FileFact {
                        bytes: meta.len(),
                        kind,
                    },
                );
            } else {
                self.issue("unsafe-entry");
            }
        }
    }
    fn opaque(&mut self, root: &Path, kind: StorageKind) {
        let mut pending = vec![root.to_owned()];
        while let Some(path) = pending.pop() {
            if self.entries >= MAX_ITEMS {
                self.issue("entry-limit");
                break;
            }
            for child in self.entries(&path) {
                if let Some(m) = self.meta(&child) {
                    if m.is_dir() {
                        pending.push(child);
                    } else if m.is_file() {
                        self.file(&child, kind);
                    } else {
                        self.issue("unsafe-entry");
                    }
                }
            }
        }
    }
    fn feature(
        &mut self,
        root: &Path,
        name: &str,
        required: bool,
        actor: Uuid,
        generation: Uuid,
    ) -> bool {
        let path = root.join(name);
        let present = self.exists(&path);
        if !present {
            if required {
                self.issue("unsupported-generation");
            }
            return !required;
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Feature {
            format: u32,
            actor: Uuid,
            generation: Uuid,
        }
        let valid = self
            .json::<Feature>(&path, MAX_RECORD)
            .is_some_and(|v| v.format == 1 && v.actor == actor && v.generation == generation);
        if !valid {
            self.issue("unsupported-generation");
        }
        valid
    }
    fn notebook(
        &mut self,
        envelope: &Envelope,
        version: u32,
        feedback: bool,
    ) -> Option<Option<NotebookKind>> {
        if envelope.validate().is_err() {
            self.issue("invalid-reference");
            return None;
        }
        if envelope.namespace == Namespace::UnassociatedFeedback
            && (!feedback
                || crate::unassociated_feedback::validate_envelope(
                    envelope,
                    self.report.owner_actor.unwrap_or_default(),
                )
                .is_err())
        {
            self.issue("invalid-reference");
            return None;
        }
        if envelope.kind == Kind::Tombstone {
            return Some(None);
        }
        if envelope.domain_schema_version == 3
            && (version != 4 || Some(envelope.actor_id) != self.report.owner_actor)
        {
            self.issue("invalid-reference");
            return None;
        }
        if matches!(
            envelope.namespace,
            Namespace::Source | Namespace::Conversation | Namespace::ExportAssociation
        ) {
            match Ledger::decode(envelope) {
                Ok(record) => {
                    return Some(match record {
                        Record::ArchivedNormalizedSource(_) => {
                            Some(NotebookKind::ArchivedNormalizedSource)
                        }
                        Record::NotebookPlan(_) => Some(NotebookKind::NotebookPlan),
                        Record::NotebookStage(_) => Some(NotebookKind::NotebookStage),
                        _ => None,
                    });
                }
                Err(_) => {
                    self.issue("unclassified-object");
                    return None;
                }
            }
        }
        Some(None)
    }
    fn manifest(
        &mut self,
        generation: Uuid,
        root: &Path,
        manifest: &Manifest,
        version: u32,
        feedback: bool,
    ) {
        // Validate bounds before invoking the shared pure validator (which sums
        // record lengths). No filesystem reads occur inside Manifest::validate.
        if manifest.records.len() > MAX_ITEMS
            || manifest.media.len() > MAX_ITEMS
            || manifest.media_coverage.len() > MAX_ITEMS
            || manifest.validate().is_err()
        {
            self.issue("invalid-metadata");
            return;
        }
        let mut descriptors = BTreeMap::new();
        for reference in &manifest.records {
            let Some(namespace) = manifest.record_namespaces.get(&reference.sha256).copied() else {
                self.issue("invalid-reference");
                continue;
            };
            let key = (generation, reference.sha256.clone());
            let media = if let Some(prior) = self.records.get(&key) {
                if prior.namespace != namespace || prior.bytes != reference.bytes {
                    self.issue("inconsistent-reference");
                    continue;
                }
                prior.media.clone()
            } else {
                if self.records.len() + self.media.len() >= MAX_ITEMS {
                    self.issue("entry-limit");
                    break;
                }
                let path = root.join("objects").join(&reference.sha256);
                let Some(bytes) = self.read(&path, MAX_RECORD) else {
                    continue;
                };
                if bytes.len() as u64 != reference.bytes || digest(&bytes) != reference.sha256 {
                    self.issue("invalid-reference");
                    continue;
                }
                let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) else {
                    self.issue("invalid-metadata");
                    continue;
                };
                if envelope.namespace != namespace {
                    self.issue("invalid-reference");
                    continue;
                }
                let Some(notebook) = self.notebook(&envelope, version, feedback) else {
                    continue;
                };
                let media = envelope.media_descriptors;
                self.records.insert(
                    key,
                    RecordFact {
                        namespace,
                        bytes: reference.bytes,
                        media: media.clone(),
                        notebook,
                    },
                );
                media
            };
            for media in media {
                if descriptors
                    .insert(media.sha256.clone(), media.clone())
                    .is_some_and(|old| old != media)
                {
                    self.issue("inconsistent-reference");
                }
                let coverage = manifest
                    .media_coverage
                    .iter()
                    .find(|v| v.hash() == media.sha256);
                if coverage.is_none() {
                    self.issue("invalid-reference");
                }
                let key = (generation, media.sha256.clone());
                if !self.media.contains_key(&key)
                    && self.records.len() + self.media.len() >= MAX_ITEMS
                {
                    self.issue("entry-limit");
                    continue;
                }
                let fact = self.media.entry(key).or_insert_with(|| MediaFact {
                    bytes: media.bytes,
                    ..Default::default()
                });
                if fact.bytes != media.bytes {
                    self.issue("inconsistent-reference");
                    continue;
                }
                fact.namespaces.insert(namespace);
                if matches!(coverage, Some(Coverage::Omitted { .. })) {
                    fact.omitted.insert(namespace);
                }
            }
        }
        if descriptors
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != manifest.media_coverage.iter().map(Coverage::hash).collect()
        {
            self.issue("invalid-reference");
        }
        if manifest.media.iter().any(|m| {
            !descriptors
                .get(&m.sha256)
                .is_some_and(|d| d.bytes == m.bytes)
        }) {
            self.issue("invalid-reference");
        }
    }
    fn generation(&mut self, root: &Path, id: Uuid, actor: Uuid, current: bool) {
        let Some(version) = self.json::<u32>(&root.join("format.json"), 64) else {
            return;
        };
        if !matches!(version, 1..=4) {
            self.issue("unsupported-generation");
            if current {
                self.report.status = InspectionStatus::Unsupported;
            }
            return;
        }
        let selection = self.exists(&root.join("selection-feature.json"));
        let feedback = self.exists(&root.join("feedback-feature.json"));
        let notebook = self.exists(&root.join("notebook-feature.json"));
        let valid = self.feature(root, "selection-feature.json", version == 2, actor, id)
            & self.feature(root, "feedback-feature.json", version == 3, actor, id)
            & self.feature(root, "notebook-feature.json", version == 4, actor, id);
        if !valid
            || version == 1 && (selection || feedback || notebook)
            || version != 4 && notebook
            || version < 3 && feedback
        {
            self.issue("unsupported-generation");
            if current {
                self.report.status = InspectionStatus::Unsupported;
            }
            return;
        }
        if current {
            self.report.current_generation_format = Some(version);
        } else if !self.report.retained_generation_formats.contains(&version) {
            self.report.retained_generation_formats.push(version);
            self.report.retained_generation_formats.sort_unstable();
        }
        self.generations.insert(id, root.to_owned());
        for required in ["objects", "commits"] {
            if !self.meta(&root.join(required)).is_some_and(|m| m.is_dir()) {
                self.issue("missing-entry");
            }
        }
        if version == 1
            && (self.exists(&root.join("selections"))
                || self.exists(&root.join("selection-history")))
        {
            self.issue("unsupported-generation");
        }
        let kind = if current {
            StorageKind::Structural
        } else {
            StorageKind::RetainedGeneration
        };
        for path in self.entries(root) {
            let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
            match name {
                "format.json"
                | "selection-feature.json"
                | "feedback-feature.json"
                | "notebook-feature.json"
                | "restored-config.json" => self.file(&path, kind),
                "objects" => {
                    for object in self.entries(&path) {
                        if object
                            .file_name()
                            .and_then(|v| v.to_str())
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
                        } else if stage(&object) {
                            self.file(&object, StorageKind::Staging);
                        } else {
                            self.issue("unexpected-entry");
                        }
                    }
                }
                "commits" | "selections" | "selection-history" => {
                    for item in self.entries(&path) {
                        if stage(&item) {
                            self.file(&item, StorageKind::Staging);
                            continue;
                        }
                        let filename = item.file_name().and_then(|v| v.to_str()).unwrap_or("");
                        let recognized = match name {
                            "commits" => filename.strip_suffix(".json").is_some_and(uuid_name),
                            "selections" => {
                                filename.strip_suffix(".json").is_some_and(valid_digest)
                            }
                            _ => valid_digest(filename),
                        };
                        if !recognized {
                            self.issue("unexpected-entry");
                            continue;
                        }
                        self.file(&item, kind);
                        if name == "commits" {
                            if let Some(manifest) = self.json::<Manifest>(&item, MAX_METADATA) {
                                if manifest.transaction_id.to_string() + ".json" != filename {
                                    self.issue("invalid-reference");
                                } else {
                                    self.manifest(id, root, &manifest, version, feedback);
                                }
                            }
                        } else if let Some(bytes) = self.read(&item, MAX_METADATA) {
                            let Ok(transaction) =
                                serde_json::from_slice::<selection::SelectionTransaction>(&bytes)
                            else {
                                self.issue("invalid-metadata");
                                continue;
                            };
                            if transaction.validate(MAX_ITEMS).is_err()
                                || transaction.store_generation != id
                            {
                                self.issue("invalid-metadata");
                                continue;
                            }
                            let hash = digest(&bytes);
                            let expected = if name == "selection-history" {
                                hash.clone()
                            } else {
                                digest(&serde_json::to_vec(&transaction.scope).unwrap_or_default())
                                    + ".json"
                            };
                            if expected != filename {
                                self.issue("invalid-reference");
                                continue;
                            }
                            if self.history.len() >= MAX_ITEMS {
                                self.issue("entry-limit");
                                continue;
                            }
                            if let Some(previous) = transaction.previous_sha256 {
                                self.history.insert((id, previous));
                            }
                            for manifest in
                                std::iter::once(&transaction.selected).chain(&transaction.retained)
                            {
                                self.manifest(id, root, manifest, version, feedback);
                            }
                        }
                    }
                }
                _ if stage(&path) => self.file(&path, StorageKind::Staging),
                _ => self.issue("unexpected-entry"),
            }
        }
    }
    fn totals(&mut self, current: Uuid) {
        if !self.generations.contains_key(&current) {
            self.issue("missing-entry");
        }
        let incomplete = [
            "entry-limit",
            "metadata-limit",
            "unsafe-entry",
            "unreadable-entry",
            "unexpected-entry",
        ]
        .iter()
        .any(|v| self.report.issues.contains_key(v));
        for (generation, hash) in &self.history {
            if !self.generations.get(generation).is_some_and(|p| {
                self.files
                    .contains_key(&p.join("selection-history").join(hash))
            }) {
                self.report.issue(if incomplete {
                    "uninspected-reference"
                } else {
                    "missing-entry"
                });
            }
        }
        for ((generation, hash), record) in &self.records {
            let Some(root) = self.generations.get(generation) else {
                continue;
            };
            if let Some(file) = self.files.get_mut(&root.join("objects").join(hash)) {
                if file.bytes == record.bytes {
                    let d = self
                        .report
                        .observed_domains
                        .entry(record.namespace)
                        .or_default();
                    d.observed_record_files.0 += 1;
                    d.observed_record_bytes.add(file.bytes);
                    if let Some(notebook) = record.notebook {
                        self.report
                            .observed_notebook_records
                            .entry(notebook)
                            .or_default()
                            .add(file.bytes);
                    }
                    if *generation == current {
                        file.kind = StorageKind::RecordMetadata;
                    }
                } else {
                    self.report.issue("size-mismatch");
                }
            } else {
                self.report.issue(if incomplete {
                    "uninspected-reference"
                } else {
                    "missing-object"
                });
            }
        }
        for ((generation, hash), media) in &self.media {
            let Some(root) = self.generations.get(generation) else {
                continue;
            };
            let path = root.join("objects").join(hash);
            let present = self.files.get(&path).map(|v| v.bytes);
            if present == Some(media.bytes) && media.namespaces.len() > 1 {
                self.report.shared_media_files.0 += 1;
                self.report.shared_media_apparent_bytes.add(media.bytes);
            }
            if *generation == current && !self.records.contains_key(&(*generation, hash.clone())) {
                if let Some(file) = self.files.get_mut(&path) {
                    file.kind = StorageKind::MediaObject;
                }
            }
            for namespace in &media.namespaces {
                let d = self.report.observed_domains.entry(*namespace).or_default();
                d.declared_media_bytes.add(media.bytes);
                match present {
                    Some(bytes) => {
                        d.present_referenced_media_bytes.add(bytes);
                        if bytes != media.bytes {
                            d.size_mismatched_references.0 += 1;
                        }
                    }
                    None => {
                        if incomplete {
                            d.uninspected_references.0 += 1;
                        } else {
                            d.missing_references.0 += 1;
                        }
                    }
                }
                if media.omitted.contains(namespace) {
                    d.omitted_references.0 += 1;
                }
            }
            if present.is_none() {
                self.report.issue(if incomplete {
                    "uninspected-reference"
                } else {
                    "missing-object"
                });
            }
            if present.is_some_and(|v| v != media.bytes) {
                self.report.issue("size-mismatch");
            }
            if !media.omitted.is_empty() {
                self.report.issue("omitted-media");
            }
        }
        for (path, file) in &mut self.files {
            if let Some((generation, _)) = self
                .generations
                .iter()
                .find(|(_, p)| path.parent() == Some(p.join("objects").as_path()))
            {
                let hash = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
                if valid_digest(hash)
                    && !self.records.contains_key(&(*generation, hash.to_owned()))
                    && !self.media.contains_key(&(*generation, hash.to_owned()))
                {
                    file.kind = StorageKind::UnclassifiedObject;
                    self.report.issue("unclassified-object");
                }
            }
            self.report.observed_files.0 += 1;
            self.report.observed_apparent_bytes.add(file.bytes);
            self.report
                .storage
                .entry(file.kind)
                .or_default()
                .add(file.bytes);
        }
        if self.report.overflowed() {
            self.issue("byte-count-overflow");
        }
        if self.access.verify().is_err() {
            self.issue("state-changed");
        }
        if self.report.status != InspectionStatus::Unsupported {
            self.report.status = if self.report.issues.is_empty() {
                InspectionStatus::Complete
            } else {
                InspectionStatus::Partial
            };
        }
        if self.report.status == InspectionStatus::Complete {
            self.report.domain_accounting_complete = true;
            self.report.complete_apparent_bytes = Some(Count(
                self.report
                    .observed_apparent_bytes
                    .value()
                    .unwrap_or_default(),
            ));
            for n in [
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
                Namespace::SubjectMemory,
                Namespace::Handwriting,
                Namespace::UnassociatedFeedback,
            ] {
                self.report.observed_domains.entry(n).or_default();
            }
            for n in [
                NotebookKind::ArchivedNormalizedSource,
                NotebookKind::NotebookPlan,
                NotebookKind::NotebookStage,
            ] {
                self.report.observed_notebook_records.entry(n).or_default();
            }
            for k in [
                StorageKind::Structural,
                StorageKind::RecordMetadata,
                StorageKind::MediaObject,
                StorageKind::UnclassifiedObject,
                StorageKind::RetainedGeneration,
                StorageKind::Cache,
                StorageKind::Diagnostics,
                StorageKind::Staging,
            ] {
                self.report.storage.entry(k).or_default();
            }
        }
    }
}
fn stage(path: &Path) -> bool {
    path.file_name()
        .and_then(|v| v.to_str())
        .and_then(|v| v.strip_prefix(".stage-"))
        .is_some_and(uuid_name)
}
fn uuid_name(name: &str) -> bool {
    Uuid::parse_str(name).is_ok_and(|id| !id.is_nil() && id.to_string() == name)
}

pub(super) fn inspect(paths: &StorePaths) -> Inspection {
    let access = match Access::new(paths) {
        Ok(v) => v,
        Err(e) => {
            return Inspection::failure(
                if e == Failure::Missing {
                    InspectionStatus::Unavailable
                } else {
                    InspectionStatus::Unsupported
                },
                if e == Failure::Missing {
                    "missing-owner"
                } else {
                    "invalid-paths"
                },
            )
        }
    };
    let mut scan = Scan {
        report: Report::empty(InspectionStatus::Unavailable),
        access,
        entries: 0,
        read_bytes: 0,
        files: BTreeMap::new(),
        records: BTreeMap::new(),
        media: BTreeMap::new(),
        generations: BTreeMap::new(),
        history: BTreeSet::new(),
    };
    let lease = match scan.access.lease(&paths.data.join("store.lock")) {
        Ok(v) => v,
        Err(error) => {
            return Inspection::failure(
                if error == Failure::Unsafe {
                    InspectionStatus::Unsupported
                } else {
                    InspectionStatus::Unavailable
                },
                match error {
                    Failure::Unsafe => "unsafe-entry",
                    Failure::Missing => "missing-lock",
                    _ => "lock-unavailable",
                },
            )
        }
    };
    match lease.try_lock() {
        Ok(()) => (),
        Err(std::fs::TryLockError::WouldBlock) => {
            return Inspection::failure(InspectionStatus::Busy, "busy-writer")
        }
        Err(std::fs::TryLockError::Error(_)) => {
            return Inspection::failure(InspectionStatus::Unsupported, "lock-unsupported")
        }
    };
    scan.report.lease_identity_check = "device-inode";
    let identity = scan.json::<Identity>(&paths.data.join("identity.json"), MAX_RECORD);
    let current = scan.json::<Uuid>(&paths.data.join("CURRENT"), 128);
    if !identity
        .as_ref()
        .is_some_and(|v| v.format == 1 && !v.actor.is_nil())
        || !current.is_some_and(|v| !v.is_nil())
    {
        scan.issue("invalid-owner");
        return Inspection {
            report: scan.report,
            custody: Some((scan.access, lease)),
        };
    }
    let identity = identity.unwrap();
    let current = current.unwrap();
    scan.report.owner_actor = Some(identity.actor);
    scan.report.current_generation = Some(current);
    for path in scan.entries(&paths.data) {
        match path.file_name().and_then(|v| v.to_str()).unwrap_or("") {
            "identity.json"
            | "CURRENT"
            | "store.lock"
            | "capabilities.json"
            | "contract-v1.json"
            | "contract-v2.json"
            | "contract-v3.json"
            | "config.snapshot.json"
            | "effective-config.json" => scan.file(&path, StorageKind::Structural),
            "generations" => {
                for root in scan.entries(&path) {
                    let name = root.file_name().and_then(|v| v.to_str()).unwrap_or("");
                    if uuid_name(name) {
                        let id = Uuid::parse_str(name).unwrap();
                        scan.generation(&root, id, identity.actor, id == current);
                    } else {
                        scan.issue("unexpected-entry");
                    }
                }
            }
            "quarantine" => scan.opaque(&path, StorageKind::Diagnostics),
            "sync" => {
                for child in scan.entries(&path) {
                    match child.file_name().and_then(|v| v.to_str()).unwrap_or("") {
                        "state.json" | "health.json" | "mode.json" => {
                            scan.file(&child, StorageKind::Structural)
                        }
                        "incoming" => scan.opaque(&child, StorageKind::Staging),
                        _ if stage(&child) => scan.file(&child, StorageKind::Staging),
                        _ => scan.issue("unexpected-entry"),
                    }
                }
            }
            _ if stage(&path) => scan.file(&path, StorageKind::Staging),
            _ => scan.issue("unexpected-entry"),
        }
    }
    if scan
        .json::<Identity>(&paths.cache.join("owner.json"), MAX_RECORD)
        .is_some_and(|v| v.format == 1 && v.actor == identity.actor)
    {
        scan.report.cache_inspected = true;
        scan.opaque(&paths.cache, StorageKind::Cache);
    } else {
        scan.issue("cache-owner-mismatch");
    }
    // Notebook current capabilities are checked against the exact already
    // delivered shape, built from admitted feature observations only.
    if scan.report.current_generation_format == Some(4)
        || scan.report.retained_generation_formats.contains(&4)
    {
        let contract = scan.read(&paths.data.join("contract-v3.json"), MAX_RECORD);
        if contract.as_deref() != Some(include_bytes!("../contract-v3.json").as_slice()) {
            scan.issue("invalid-metadata");
        }
    }
    if scan.report.current_generation_format == Some(4) {
        let root = paths.data.join("generations").join(current.to_string());
        let feedback = scan.exists(&root.join("feedback-feature.json"));
        let selected = scan.exists(&root.join("selection-feature.json"));
        let mut namespaces = vec![
            "conversation",
            "source",
            "export-association",
            "subject-memory",
            "handwriting",
        ];
        if feedback {
            namespaces.push("unassociated-feedback");
        }
        let mut expected = serde_json::json!({"contract_version":3,"schema_file":"contract-v3.json","store_versions":[1,2,3,4],"envelope_versions":[1],"config_versions":[1],"owner_actor":identity.actor,"current_generation":current,"notebook_feature":"actor-generation-bound-v1","paths":paths,"lock":"stable-inode-exclusive-os-lease","linux_lock":"flock","maintenance":"stop-confirm-lock-stage-commit-release-restart","namespaces":namespaces,"sync":"unsupported-notebook-generation"});
        if feedback {
            expected["unassociated_feedback_feature"] =
                serde_json::json!("actor-generation-bound-v1");
        }
        if selected {
            expected["retained_selected_feature"] = serde_json::json!("actor-generation-bound-v1");
        }
        if scan.json::<serde_json::Value>(&paths.data.join("capabilities.json"), MAX_RECORD)
            != Some(expected)
        {
            scan.issue("invalid-metadata");
        }
    }
    scan.totals(current);
    Inspection {
        report: scan.report,
        custody: Some((scan.access, lease)),
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn failed_directory_enumeration_exhausts_the_global_remaining_allowance() {
        let root = std::env::temp_dir().join(format!("report3-entries-{}", Uuid::new_v4()));
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        drop(Store::open(paths.clone()).unwrap());
        let first = paths.cache.join("first");
        let second = paths.cache.join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        for n in 0..3 {
            std::fs::write(first.join(n.to_string()), b"x").unwrap();
        }
        std::fs::write(second.join("must-not-be-traversed"), b"x").unwrap();
        let mut scan = Scan {
            report: Report::empty(InspectionStatus::Unavailable),
            access: Access::new(&paths).unwrap(),
            entries: MAX_ITEMS - 2,
            read_bytes: 0,
            files: BTreeMap::new(),
            records: BTreeMap::new(),
            media: BTreeMap::new(),
            generations: BTreeMap::new(),
            history: BTreeSet::new(),
        };
        assert!(scan.entries(&first).is_empty());
        assert_eq!(scan.entries, MAX_ITEMS);
        assert!(scan.entries(&second).is_empty());
        assert_eq!(scan.entries, MAX_ITEMS);
        assert_eq!(scan.report.issues["entry-limit"].0, 2);
        assert!(scan
            .entries(&paths.cache.join("nonexistent-after-exhaustion"))
            .is_empty());
        assert!(!scan.report.issues.contains_key("missing-entry"));
        drop(scan);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn every_repeated_metadata_read_consumes_the_same_cumulative_budget() {
        let root = std::env::temp_dir().join(format!("report3-budget-{}", Uuid::new_v4()));
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        drop(Store::open(paths.clone()).unwrap());
        let path = paths.cache.join("opaque-test-input");
        std::fs::write(&path, vec![b'x'; MAX_RECORD]).unwrap();
        let mut scan = Scan {
            report: Report::empty(InspectionStatus::Unavailable),
            access: Access::new(&paths).unwrap(),
            entries: 0,
            read_bytes: 0,
            files: BTreeMap::new(),
            records: BTreeMap::new(),
            media: BTreeMap::new(),
            generations: BTreeMap::new(),
            history: BTreeSet::new(),
        };
        for _ in 0..8 {
            assert_eq!(scan.read(&path, MAX_RECORD).unwrap().len(), MAX_RECORD);
        }
        assert_eq!(scan.read_bytes, MAX_METADATA);
        assert!(scan.read(&path, MAX_RECORD).is_none());
        assert!(scan.report.issues.contains_key("metadata-limit"));
        drop(scan);
        std::fs::remove_dir_all(root).unwrap();
    }
}
