//! Upload-only ordinary isolated publisher under the existing Drive Worker.
use super::*;
use crate::storage::recovery::isolated_backup::{
    package_members, PackageMembers, PublicationSource, PublicationSourceChanged, MARKER,
};
use serde_json::Value;

type Properties = BTreeMap<String, String>;
const CHECKPOINT_LIMIT: u64 = MAX_METADATA as u64;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    actor: Uuid,
    device: Uuid,
    backup: Uuid,
    epoch: Uuid,
    grant: String,
    root: String,
}

impl Binding {
    fn matches(&self, store: &Store, mode: &mode::SyncModeSnapshot) -> bool {
        self.actor == store.actor_id
            && mode.device == Some(self.device)
            && mode.epoch == Some(self.epoch)
            && matches!(&mode.mode, Some(mode::SyncMode::IsolatedBackup { backup, binding_sha256 }) if *backup == self.backup && *binding_sha256 == self.grant)
    }

    fn properties(&self, kind: &str) -> Properties {
        BTreeMap::from([
            ("buddy_isolated".into(), "1".into()),
            ("root".into(), self.root.clone()),
            ("backup".into(), self.backup.to_string()),
            ("kind".into(), kind.into()),
        ])
    }

    fn root_properties(&self) -> Properties {
        let mut properties = self.properties("root");
        properties.insert("device".into(), self.device.to_string());
        properties.insert("grant".into(), self.grant.clone());
        properties
    }

    fn object_properties(&self, reference: &ObjectRef) -> Properties {
        let mut properties = self.properties("object");
        properties.insert("sha256".into(), reference.sha256.clone());
        properties
    }

    fn children(&self, predecessor: &str) -> Properties {
        let mut properties = self.properties("marker");
        properties.insert("predecessor".into(), predecessor.into());
        properties
    }

    fn marker_properties(&self, predecessor: &str, reference: &ObjectRef) -> Properties {
        let mut properties = self.children(predecessor);
        properties.insert("sha256".into(), reference.sha256.clone());
        properties
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    id: String,
    reference: ObjectRef,
    verified: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    id: String,
    predecessor: String,
    marker: ObjectRef,
    complete_media: bool,
    source: PublicationSource,
    verified_at_unix_seconds: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    id: Uuid,
    predecessor: String,
    source: PublicationSource,
    marker: Option<ObjectRef>,
    objects: BTreeMap<String, Upload>,
    marker_id: Option<String>,
    marker_observed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    format: u32,
    binding: Binding,
    latest: Option<Revision>,
    attempt: Option<Attempt>,
    // Cleanup applies only to a settled package, after the new head is durable.
    settled_stage: Option<Uuid>,
}

fn integrity<T>(result: Result<T>) -> Result<T> {
    result.map_err(|_| InvalidRemoteData.into())
}

fn absent(path: &Path) -> Result<bool> {
    files::safe_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn source_valid(source: &PublicationSource) -> bool {
    !source.generation.is_nil()
        && [&source.history, &source.config_path, &source.config_sha256]
            .into_iter()
            .all(|digest| valid_digest(digest))
}

fn marker_valid(marker: &ObjectRef) -> bool {
    valid_digest(&marker.sha256) && (1..=16 * 1024).contains(&marker.bytes)
}

impl Checkpoint {
    fn predecessor(&self) -> &str {
        self.latest
            .as_ref()
            .map_or(&self.binding.root, |latest| &latest.id)
    }

    fn validate(&self, binding: &Binding) -> Result<()> {
        ensure!(
            self.format == 1 && &self.binding == binding,
            "publisher custody changed"
        );
        ensure!(
            self.settled_stage.is_none_or(|stage| !stage.is_nil())
                && (self.settled_stage.is_none() || self.attempt.is_none()),
            "invalid settled package custody"
        );
        if let Some(latest) = &self.latest {
            id(&latest.id)?;
            validate_properties(&binding.marker_properties(&latest.predecessor, &latest.marker))?;
            ensure!(
                latest.id != binding.root
                    && latest.predecessor != latest.id
                    && marker_valid(&latest.marker)
                    && source_valid(&latest.source),
                "invalid verified revision custody"
            );
        }
        if let Some(attempt) = &self.attempt {
            let mut unique = BTreeSet::from([binding.root.as_str(), attempt.predecessor.as_str()]);
            ensure!(
                !attempt.id.is_nil()
                    && source_valid(&attempt.source)
                    && attempt.predecessor == self.predecessor()
                    && attempt.objects.len() <= MAX_ITEMS + 2
                    && attempt.marker.as_ref().is_none_or(marker_valid),
                "invalid original publication attempt"
            );
            ensure!(
                attempt.marker.is_some()
                    || (attempt.objects.is_empty() && attempt.marker_id.is_none()),
                "missing original marker"
            );
            if let Some(marker_id) = &attempt.marker_id {
                id(marker_id)?;
                validate_properties(&binding.children(marker_id))?;
                ensure!(
                    unique.insert(marker_id)
                        && attempt.objects.values().all(|upload| upload.verified),
                    "invalid final allocated identity"
                );
            }
            ensure!(
                !attempt.marker_observed || attempt.marker_id.is_some(),
                "missing observed marker ID"
            );
            for (digest, upload) in &attempt.objects {
                id(&upload.id)?;
                ensure!(
                    valid_digest(digest)
                        && *digest == upload.reference.sha256
                        && (1..=MAX_MEDIA).contains(&upload.reference.bytes)
                        && unique.insert(&upload.id),
                    "duplicate allocated identity"
                );
            }
        }
        Ok(())
    }
}

// Search state is only used within one running sequence. All actual upload
// decisions are tied to the journal's exact IDs and exact readback.
#[derive(Default)]
struct Search {
    properties: Properties,
    pages: BTreeSet<String>,
    next: Option<String>,
    candidate: Option<RemoteFile>,
}

enum Found {
    More,
    Result(Option<RemoteFile>),
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "root provisioning has no production constructor")
)]
struct Engine {
    store: Arc<Store>,
    lease: mode::SyncModeLease,
    binding: Binding,
    saved_config: PathBuf,
    checkpoint: PathBuf,
    staging: PathBuf,
    sessions: PathBuf,
    transport: GoogleDrive,
    policy: SyncPolicy,
    cancel: Arc<AtomicBool>,
    state: Checkpoint,
    search: Search,
    members: Option<(Uuid, PackageMembers)>,
    // Remote closure verification restarts after any failed step or process exit.
    checked: usize,
    observe_cursor: usize,
    stale: bool,
    _publisher: File,
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "root provisioning has no production constructor")
)]
impl Engine {
    fn read_checkpoint(path: &Path, binding: &Binding) -> Result<Checkpoint> {
        let bytes = files::read(path, CHECKPOINT_LIMIT)?;
        let value = crate::config::inspection::strict_json(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid isolated publisher checkpoint"))?;
        let state: Checkpoint = serde_json::from_value(value)?;
        state.validate(binding)?;
        Ok(state)
    }

    fn save(&self) -> Result<()> {
        integrity(self.state.validate(&self.binding))?;
        let bytes = serde_json::to_vec(&self.state)?;
        integrity(
            (bytes.len() as u64 <= CHECKPOINT_LIMIT)
                .then_some(())
                .context("isolated publisher checkpoint bound"),
        )?;
        files::atomic(&self.checkpoint, &bytes)
    }

    fn find(&mut self, properties: &Properties) -> Result<Found> {
        if &self.search.properties != properties {
            self.search = Search {
                properties: properties.clone(),
                ..Search::default()
            };
        }
        let page = self
            .transport
            .list_properties(properties, self.search.next.as_deref())?;
        integrity(
            (page.items.len() <= 100)
                .then_some(())
                .context("oversized isolated listing"),
        )?;
        for file in page.items {
            let expected = if properties.get("kind").is_some_and(|kind| kind == "marker") {
                let sha = file
                    .app_properties
                    .get("sha256")
                    .context("missing marker digest");
                let sha = integrity(sha)?;
                integrity(
                    valid_digest(sha)
                        .then_some(())
                        .context("invalid marker digest"),
                )?;
                let mut exact = properties.clone();
                exact.insert("sha256".into(), sha.clone());
                exact
            } else {
                properties.clone()
            };
            integrity(id(&file.id))?;
            if file.trashed || file.app_properties != expected || self.search.candidate.is_some() {
                return Err(InvalidRemoteData.into());
            }
            self.search.candidate = Some(file);
        }
        if let Some(next) = page.next {
            if next.is_empty()
                || next.len() > 2048
                || self.search.pages.len() >= MAX_ITEMS
                || !self.search.pages.insert(next.clone())
            {
                return Err(InvalidRemoteData.into());
            }
            self.search.next = Some(next);
            Ok(Found::More)
        } else {
            let found = self.search.candidate.take();
            self.search = Search::default();
            Ok(Found::Result(found))
        }
    }

    fn authorized(&mut self) -> Result<()> {
        self.transport.authorize_grant(&self.binding.grant)?;
        if !self.transport.read_immutable(
            &self.binding.root,
            &self.binding.root_properties(),
            None,
        )? {
            return Err(InvalidRemoteData.into());
        }
        Ok(())
    }

    fn current_source(&self) -> Result<Option<PublicationSource>> {
        self.store
            .isolated_publication_source(&self.lease, &self.saved_config)
    }

    fn still_current(&self, source: &PublicationSource) -> bool {
        Self::same_source(self.current_source(), source)
    }

    fn same_source(current: Result<Option<PublicationSource>>, source: &PublicationSource) -> bool {
        current
            .ok()
            .flatten()
            .is_some_and(|current| source.same_generation_and_config(&current))
    }

    fn check_latest(&mut self) -> Result<()> {
        if let Some(latest) = &self.state.latest {
            if !self.transport.read_immutable(
                &latest.id,
                &self
                    .binding
                    .marker_properties(&latest.predecessor, &latest.marker),
                Some(&latest.marker),
            )? {
                return Err(InvalidRemoteData.into());
            }
        }
        Ok(())
    }

    fn check_local_custody(&self) -> Result<()> {
        let inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let current: Uuid = files::json(&self.store.paths.data.join("CURRENT"), 128)?;
        ensure!(
            inner.generation == current && inner.index.unavailable == 0,
            "isolated publisher Store custody is corrupt"
        );
        mode::load(&self.store)?;
        Ok(())
    }

    fn load_members(&mut self, attempt: &Attempt, marker: &ObjectRef) -> Result<()> {
        if self
            .members
            .as_ref()
            .is_none_or(|(id, _)| *id != attempt.id)
        {
            let package = integrity(package_members(
                &self.staging.join(attempt.id.to_string()),
                &marker.sha256,
            ))?;
            let mut references = BTreeMap::new();
            for (_, reference) in &package.objects {
                if references
                    .insert(&reference.sha256, reference)
                    .is_some_and(|old| old != reference)
                {
                    return Err(InvalidRemoteData.into());
                }
            }
            if package.marker != *marker
                || attempt
                    .objects
                    .iter()
                    .any(|(digest, upload)| references.get(digest) != Some(&&upload.reference))
            {
                return Err(InvalidRemoteData.into());
            }
            if attempt.marker_id.is_some() && references.len() != attempt.objects.len() {
                return Err(InvalidRemoteData.into());
            }
            self.members = Some((attempt.id, package));
        }
        Ok(())
    }

    fn stage(&mut self, attempt: &Attempt) -> Result<Status> {
        self.stage_after_check(attempt, || Ok(()))
    }

    fn stage_after_check(
        &mut self,
        attempt: &Attempt,
        before_capture: impl FnOnce() -> Result<()>,
    ) -> Result<Status> {
        let root = self.staging.join(attempt.id.to_string());
        if !integrity(absent(&root))? {
            // An interrupted local package with no journaled marker cannot
            // authorize its first provider mutation.
            return Err(InvalidRemoteData.into());
        }
        if !self.still_current(&attempt.source) {
            self.stale = true;
            return Ok(Status::Pending);
        }
        before_capture()?;
        let captured = self.store.freeze_isolated_package(
            &self.lease,
            &root,
            &self.saved_config,
            &attempt.source,
        );
        if captured
            .as_ref()
            .is_err_and(|error| error.is::<PublicationSourceChanged>())
        {
            self.stale = true;
            return Ok(Status::Pending);
        }
        let Some((source, digest)) = integrity(captured)? else {
            return Err(InvalidRemoteData.into());
        };
        let package = integrity(package_members(&root, &digest))?;
        let staged = self
            .state
            .attempt
            .as_mut()
            .context("missing staged attempt")?;
        staged.source = source;
        staged.marker = Some(package.marker.clone());
        self.members = Some((attempt.id, package));
        self.save()?;
        Ok(Status::Pending)
    }

    fn session_path(&self, attempt: Uuid, id: &str) -> PathBuf {
        // The per-binding credentials subdirectory is private. Paths use
        // digests so provider IDs cannot alias Windows names or case folds.
        self.sessions
            .join(format!("{attempt}-{}.json", digest(id.as_bytes())))
    }

    fn upload(
        &mut self,
        attempt: &Attempt,
        path: &Path,
        reference: &ObjectRef,
        id: &str,
        properties: &Properties,
    ) -> Result<bool> {
        let mut bytes = integrity(files::Source::open(path, reference))?;
        let session_path = self.session_path(attempt.id, id);
        let mut session: UploadSession = if integrity(absent(&session_path))? {
            UploadSession::default()
        } else {
            integrity(files::json(&session_path, 16 * 1024))?
        };
        if session.offset > reference.bytes
            || session.uri.as_ref().is_some_and(|uri| uri.len() > 8192)
            || (session.uri.is_none() && session.offset != 0)
        {
            return Err(InvalidRemoteData.into());
        }
        if let Some(uri) = &session.uri {
            integrity(self.transport.safe_session(uri))?;
        }
        if !self.still_current(&attempt.source) {
            self.stale = true;
            return Ok(false);
        }
        let progress =
            self.transport
                .upload_immutable(id, properties, &mut bytes, &mut session, |drive| {
                    if !drive.original_grant_present(&self.binding.grant) {
                        return Err(TransportFailure {
                            status: 403,
                            retry_after_seconds: None,
                            retryable: false,
                            authorization_required: true,
                        }
                        .into());
                    }
                    if !Self::same_source(
                        self.store
                            .isolated_publication_source(&self.lease, &self.saved_config),
                        &attempt.source,
                    ) {
                        return Err(PublicationSourceChanged.into());
                    }
                    Ok(())
                });
        let progress = match progress {
            Err(error) if error.is::<PublicationSourceChanged>() => {
                self.stale = true;
                return Ok(false);
            }
            result => result?,
        };
        if reference.bytes > RESUMABLE_THRESHOLD {
            files::atomic_json(&session_path, &session)?;
        }
        Ok(matches!(progress, UploadProgress::Complete))
    }

    fn publish_object(
        &mut self,
        attempt: &Attempt,
        member: (PathBuf, ObjectRef),
    ) -> Result<Status> {
        let (path, reference) = member;
        let properties = self.binding.object_properties(&reference);
        let upload = match attempt.objects.get(&reference.sha256) {
            Some(upload) => upload,
            None if self.stale => return Ok(Status::Pending),
            None => {
                let Found::Result(candidate) = self.find(&properties)? else {
                    return Ok(Status::Pending);
                };
                if !self.still_current(&attempt.source) {
                    self.stale = true;
                    return Ok(Status::Pending);
                }
                let upload = if let Some(candidate) = candidate {
                    if !self.transport.read_immutable(
                        &candidate.id,
                        &properties,
                        Some(&reference),
                    )? {
                        return Err(InvalidRemoteData.into());
                    }
                    Upload {
                        id: candidate.id,
                        reference: reference.clone(),
                        verified: true,
                    }
                } else {
                    Upload {
                        id: self.transport.allocate()?,
                        reference: reference.clone(),
                        verified: false,
                    }
                };
                self.state
                    .attempt
                    .as_mut()
                    .context("missing attempt")?
                    .objects
                    .insert(reference.sha256, upload);
                self.save()?;
                return Ok(Status::Pending);
            }
        };
        let observed = self
            .transport
            .read_immutable(&upload.id, &properties, Some(&reference))?;
        if observed
            || (!self.stale
                && self.upload(
                    attempt,
                    &self.staging.join(attempt.id.to_string()).join(path),
                    &reference,
                    &upload.id,
                    &properties,
                )?)
        {
            self.state
                .attempt
                .as_mut()
                .context("missing attempt")?
                .objects
                .get_mut(&reference.sha256)
                .context("missing object")?
                .verified = true;
            self.save()?;
        }
        Ok(Status::Pending)
    }

    fn check_closure(&mut self, attempt: &Attempt) -> Result<bool> {
        let (_, package) = self.members.as_ref().context("missing staged membership")?;
        let Some((_, reference)) = package.objects.get(self.checked) else {
            return Ok(true);
        };
        let upload = integrity(
            attempt
                .objects
                .get(&reference.sha256)
                .filter(|upload| upload.verified)
                .context("missing verified member"),
        )?;
        let properties = self.binding.object_properties(reference);
        // Check both namespace uniqueness and exact ID/bytes. Never derive
        // closure from a listing alone.
        let reference = reference.clone();
        let remote_id = upload.id.clone();
        let Found::Result(candidate) = self.find(&properties)? else {
            return Ok(false);
        };
        if candidate.is_some_and(|file| file.id != remote_id)
            || !self
                .transport
                .read_immutable(&remote_id, &properties, Some(&reference))?
        {
            return Err(InvalidRemoteData.into());
        }
        self.checked += 1;
        Ok(false)
    }

    fn check_siblings(&mut self, attempt: &Attempt) -> Result<bool> {
        match self.find(&self.binding.children(&attempt.predecessor))? {
            Found::More => Ok(false),
            Found::Result(Some(file)) if Some(&file.id) != attempt.marker_id.as_ref() => {
                Err(InvalidRemoteData.into())
            }
            Found::Result(_) => Ok(true),
        }
    }

    fn finish(&mut self, attempt: &Attempt, marker: ObjectRef, id: &str) -> Result<Status> {
        let complete = self
            .members
            .as_ref()
            .context("missing verified package")?
            .1
            .complete_media;
        self.state.latest = Some(Revision {
            id: id.into(),
            predecessor: attempt.predecessor.clone(),
            marker,
            complete_media: complete,
            source: attempt.source.clone(),
            verified_at_unix_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        self.state.attempt = None;
        self.state.settled_stage = Some(attempt.id);
        self.save()?;
        self.members = None;
        self.checked = 0;
        self.observe_cursor = 0;
        Ok(self.settled_status(&attempt.source, complete))
    }

    fn settled_status(&self, source: &PublicationSource, complete: bool) -> Status {
        if self.current_source().ok().flatten().as_ref() != Some(source) {
            Status::Pending
        } else if complete {
            Status::Current
        } else {
            Status::IncompleteMedia
        }
    }

    fn publish_marker(&mut self, attempt: &Attempt, marker: ObjectRef) -> Result<Status> {
        if let Some(id) = &attempt.marker_id {
            let properties = self
                .binding
                .marker_properties(&attempt.predecessor, &marker);
            let observed = match self
                .transport
                .read_immutable(id, &properties, Some(&marker))
            {
                Err(error)
                    if error
                        .downcast_ref::<TransportFailure>()
                        .is_some_and(|error| error.status == 404) =>
                {
                    // Metadata was visible but its media GET was not. Do not
                    // treat that read failure as absence authorizing a create.
                    return Ok(Status::Pending);
                }
                result => result?,
            };
            if observed && !attempt.marker_observed {
                self.state
                    .attempt
                    .as_mut()
                    .context("missing attempt")?
                    .marker_observed = true;
                self.checked = 0;
                self.save()?;
                return Ok(Status::Pending);
            }
            if !observed && attempt.marker_observed {
                return Err(InvalidRemoteData.into());
            }
            if !observed && self.stale {
                return Ok(Status::Pending);
            }
        } else if self.stale {
            return Ok(Status::Pending);
        }
        if !self.check_closure(attempt)? || !self.check_siblings(attempt)? {
            return Ok(Status::Pending);
        }
        self.check_latest()?;
        if attempt.marker_id.is_none() && !self.stale {
            if let Some(latest) = self
                .state
                .latest
                .as_mut()
                .filter(|latest| latest.marker == marker)
            {
                latest.source = attempt.source.clone();
                let complete = latest.complete_media;
                self.state.attempt = None;
                self.state.settled_stage = Some(attempt.id);
                self.save()?;
                self.members = None;
                self.checked = 0;
                return Ok(self.settled_status(&attempt.source, complete));
            }
        }
        if let Some(id) = &attempt.marker_id {
            if attempt.marker_observed {
                return self.finish(attempt, marker, id);
            }
            let properties = self
                .binding
                .marker_properties(&attempt.predecessor, &marker);
            let path = self.staging.join(attempt.id.to_string()).join(MARKER);
            if self.upload(attempt, &path, &marker, id, &properties)? {
                self.state
                    .attempt
                    .as_mut()
                    .context("missing attempt")?
                    .marker_observed = true;
                self.save()?;
                self.checked = 0;
            }
        } else if self.still_current(&attempt.source) {
            self.state
                .attempt
                .as_mut()
                .context("missing attempt")?
                .marker_id = Some(self.transport.allocate()?);
            self.save()?;
            self.checked = 0;
        }
        Ok(Status::Pending)
    }

    fn step_inner(&mut self) -> Result<Status> {
        let state = integrity(Self::read_checkpoint(&self.checkpoint, &self.binding))?;
        self.state = state;
        integrity(self.check_local_custody())?;
        if let Some(stage) = self.state.settled_stage {
            self.release_settled(stage)?;
        }
        if let Some(attempt) = self.state.attempt.clone() {
            self.stale = !self.still_current(&attempt.source);
            let Some(marker) = attempt.marker.clone() else {
                return self.stage(&attempt);
            };
            self.load_members(&attempt, &marker)?;
            let package = &self.members.as_ref().context("missing package")?.1;
            if package.largest_media > self.policy.max_media_bytes {
                return Ok(Status::RecoveryRequired);
            }
            self.authorized()?;
            let package = &self.members.as_ref().context("missing package")?.1;
            let mut pending = package.objects.iter().filter(|(_, reference)| {
                attempt
                    .objects
                    .get(&reference.sha256)
                    .is_none_or(|object| !object.verified)
                    && (!self.stale || attempt.objects.contains_key(&reference.sha256))
            });
            let member = if self.stale {
                let len = pending.clone().count();
                if len == 0 {
                    None
                } else {
                    let item = pending.nth(self.observe_cursor % len);
                    self.observe_cursor = self.observe_cursor.wrapping_add(1);
                    item
                }
            } else {
                pending.next()
            }
            .cloned();
            if let Some(member) = member {
                return self.publish_object(&attempt, member);
            }
            return self.publish_marker(&attempt, marker);
        }
        self.stale = false;
        let Some(source) = integrity(self.current_source())? else {
            return Ok(Status::Current);
        };
        self.authorized()?;
        self.check_latest()?;
        if let Some(latest) = &self.state.latest {
            if latest.source == source {
                return Ok(if latest.complete_media {
                    Status::Current
                } else {
                    Status::IncompleteMedia
                });
            }
        }
        self.state.attempt = Some(Attempt {
            id: Uuid::new_v4(),
            predecessor: self.state.predecessor().into(),
            source,
            marker: None,
            objects: BTreeMap::new(),
            marker_id: None,
            marker_observed: false,
        });
        self.save()?;
        Ok(Status::Pending)
    }

    fn release_settled(&mut self, stage: Uuid) -> Result<()> {
        let root = self.staging.join(stage.to_string());
        // Remove only the publisher's private, settled package and private
        // sessions. Inventory checks reject links, unexpected directories, or
        // foreign entries. A partial cleanup may resume after restart.
        integrity(crate::storage::recovery::isolated_backup::release_package(
            &root,
        ))?;
        for entry in fs::read_dir(&self.sessions)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(digest) = name
                .to_str()
                .and_then(|name| name.strip_prefix(&format!("{stage}-")))
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            let path = entry.path();
            integrity(files::safe_path(&path))?;
            if !valid_digest(digest) || !entry.file_type()?.is_file() {
                return Err(InvalidRemoteData.into());
            }
            fs::remove_file(&path)?;
        }
        files::sync_dir(&self.sessions)?;
        self.state.settled_stage = None;
        self.save()
    }
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "root provisioning has no production constructor")
)]
impl WorkerEngine for Engine {
    fn step(&mut self) -> Result<Status> {
        self.transport.set_request_window(
            Some(std::time::Instant::now() + Duration::from_secs(60)),
            Some(self.cancel.clone()),
        );
        let result = self.step_inner();
        self.transport.set_request_window(None, None);
        match result {
            Err(error) => {
                if let Ok(state) = Self::read_checkpoint(&self.checkpoint, &self.binding) {
                    self.state = state;
                }
                self.search = Search::default();
                self.checked = 0;
                if error.is::<InvalidRemoteData>() {
                    Ok(Status::RecoveryRequired)
                } else {
                    Err(error)
                }
            }
            ok => ok,
        }
    }

    fn health(&self, status: Status) -> Value {
        json!({
            "version":1,
            "status":status,
            "mode":"isolated-backup",
            "device":self.binding.device,
            "backup":self.binding.backup,
            "latest_verified":self.state.latest.as_ref().map(|latest| json!({
                "marker_sha256":latest.marker.sha256,
                "complete_media":latest.complete_media,
                "verified_at_unix_seconds":latest.verified_at_unix_seconds,
            })),
            "pending":self.state.attempt.as_ref().map(|attempt| {
                if self.stale { "observe-only" }
                else if attempt.marker_id.is_some() && !attempt.marker_observed { "marker-outcome-unknown" }
                else { "publishing" }
            }),
        })
    }
}

#[cfg(test)]
mod tests;
