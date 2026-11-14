//! One local Writer request scoped to the installed activation and a page.
use super::{libc, open_at, recovery, Lease};
use crate::{
    storage::{files, Uuid},
    writer::Request,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{ErrorKind, Read},
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, Instant},
};

const LIMIT: u64 = 32 * 1024;
const LIFETIME_SECS: u64 = 120;

#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Submission {
    RefineOnTablet {
        document: Uuid,
        page: Uuid,
        refinement: Request,
    },
    GenerateRegionOnTablet {
        document: Uuid,
        page: Uuid,
        operation: Uuid,
        request: String,
        guidance: Option<String>,
    },
    AdoptLegacyPage {
        approval: LegacyAdoption,
    },
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyAdoption {
    pub operation: Uuid,
    pub activation: Uuid,
    pub document: Uuid,
    pub source: Uuid,
    pub target: Uuid,
    pub scene_sha256: String,
}

impl LegacyAdoption {
    fn validate(&self, lease: &Lease) -> Result<()> {
        ensure!(
            self.activation == lease.record().activation()
                && !self.operation.is_nil()
                && !self.document.is_nil()
                && !self.source.is_nil()
                && !self.target.is_nil()
                && self.source != self.target
                && self.scene_sha256.len() == 64
                && self
                    .scene_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "legacy adoption approval is outside the installed activation"
        );
        Ok(())
    }

    fn spent_name(&self) -> String {
        format!(
            "legacy-adoption-spent-{}-{}",
            self.activation, self.operation
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    submission: Submission,
    expires: u64,
    #[serde(skip)]
    digest: String,
}

impl Submission {
    pub(crate) fn identities(&self) -> (Uuid, Uuid) {
        match self {
            Self::RefineOnTablet { refinement, .. } => {
                (refinement.conversation(), refinement.operation())
            }
            Self::GenerateRegionOnTablet { operation, .. } => (Uuid::nil(), *operation),
            Self::AdoptLegacyPage { approval } => (Uuid::nil(), approval.operation),
        }
    }
}

fn boot_seconds() -> Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(time.tv_sec.try_into()?)
}

fn lock(lease: &Lease) -> Result<()> {
    if unsafe { libc::flock(lease.directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error()).context("Writer request slot busy");
    }
    lease.current()
}

fn name(lease: &Lease) -> String {
    format!("writer-request-{}.json", lease.record().activation())
}

fn adoption_name(lease: &Lease) -> String {
    format!("legacy-adoption-{}.json", lease.record().activation())
}

fn read(lease: &Lease, name: &str) -> Result<Option<Pending>> {
    let file = match open_at(&lease.directory, name, false, false) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.nlink() == 1
            && meta.mode() & 0o7777 == 0o600
            && meta.len() <= LIMIT,
        "invalid Writer request slot"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == meta.len(),
        "Writer request slot changed"
    );
    let mut pending: Pending = serde_json::from_slice(&bytes)?;
    pending.digest = crate::storage::digest(&bytes);
    Ok(Some(pending))
}

pub(crate) fn submit(submission: Submission) -> Result<()> {
    if let Submission::RefineOnTablet {
        document,
        page,
        refinement,
    } = &submission
    {
        ensure!(!document.is_nil() && !page.is_nil(), "empty Writer source");
        ensure!(
            matches!(refinement, Request::Refine { .. }),
            "explicit Writer refinement required"
        );
        refinement.validate()?;
    }
    if let Submission::GenerateRegionOnTablet {
        document,
        page,
        operation,
        request,
        guidance,
    } = &submission
    {
        RegionalRequest {
            document: *document,
            page: *page,
            operation: *operation,
            request: request.clone(),
            guidance: guidance.clone(),
        }
        .validate()?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let pin = recovery::activation::InstalledReaderSelection::load(deadline)?
        .context("installed Writer requires resident selection")?
        .output(deadline)?
        .context("installed Writer output is unavailable")?;
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    lock(&lease)?;
    ensure!(
        pin.digest() == lease.record.recovery_profile_sha256,
        "Writer activation pin changed"
    );
    lease.record.group.current_process()?;
    let name = match &submission {
        Submission::RefineOnTablet { .. } | Submission::GenerateRegionOnTablet { .. } => {
            name(&lease)
        }
        Submission::AdoptLegacyPage { approval } => {
            approval.validate(&lease)?;
            match open_at(&lease.directory, &approval.spent_name(), false, false) {
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                _ => anyhow::bail!("legacy adoption operation already spent"),
            }
            adoption_name(&lease)
        }
    };
    let now = boot_seconds()?;
    ensure!(
        read(&lease, &name)?.is_none_or(|request| request.expires <= now),
        "a local request is already awaiting its page"
    );
    files::atomic_json(
        &lease.path.join(name),
        &Pending {
            submission,
            expires: now + LIFETIME_SECS,
            digest: String::new(),
        },
    )?;
    lease.current()
}

/// Explicit installed intent; it never confers rendering permission.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegionalRequest {
    pub document: Uuid,
    pub page: Uuid,
    pub operation: Uuid,
    pub request: String,
    pub guidance: Option<String>,
}
impl RegionalRequest {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.document.is_nil() && !self.page.is_nil() && !self.operation.is_nil(),
            "empty regional Writer identity"
        );
        crate::writer::validate_request(&self.request)?;
        ensure!(
            self.guidance
                .as_ref()
                .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
            "invalid regional Writer guidance"
        );
        Ok(())
    }
}
pub(crate) enum InstalledRequest {
    Refine(Request),
    GenerateRegion(RegionalRequest),
}
impl Pending {
    fn request(self) -> Result<InstalledRequest> {
        match self.submission {
            Submission::RefineOnTablet { refinement, .. } => {
                Ok(InstalledRequest::Refine(refinement))
            }
            Submission::GenerateRegionOnTablet {
                document,
                page,
                operation,
                request,
                guidance,
            } => {
                let request = RegionalRequest {
                    document,
                    page,
                    operation,
                    request,
                    guidance,
                };
                request.validate()?;
                Ok(InstalledRequest::GenerateRegion(request))
            }
            _ => anyhow::bail!("unexpected command in Writer slot"),
        }
    }
    fn page(&self) -> Result<(Uuid, Uuid)> {
        match &self.submission {
            Submission::RefineOnTablet { document, page, .. }
            | Submission::GenerateRegionOnTablet { document, page, .. } => Ok((*document, *page)),
            _ => anyhow::bail!("unexpected command in Writer slot"),
        }
    }
}

/// Pins the precise wire bytes and activation before selecting a release policy.
#[derive(Clone)]
pub(crate) struct RegionalPin {
    activation: uuid::Uuid,
    digest: String,
    request: RegionalRequest,
    #[cfg(test)]
    fixture_path: Option<std::path::PathBuf>,
}
fn caller(activation: uuid::Uuid) -> Result<Lease> {
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    ensure!(
        lease.record().activation() == activation,
        "Writer activation changed"
    );
    lease.record.group.current_caller()?;
    lock(&lease)?;
    Ok(lease)
}
pub(crate) fn peek(activation: uuid::Uuid) -> Result<Option<RegionalPin>> {
    let lease = caller(activation)?;
    let Some(pending) = read(&lease, &name(&lease))? else {
        return Ok(None);
    };
    if !matches!(
        pending.submission,
        Submission::GenerateRegionOnTablet { .. }
    ) {
        return Ok(None);
    }
    // An expired regional request still chooses Writer, then refuses; it cannot
    // silently reinterpret the user's release as Reader.
    let digest = pending.digest.clone();
    let InstalledRequest::GenerateRegion(request) = pending.request()? else {
        anyhow::bail!("regional Writer intent changed")
    };
    Ok(Some(RegionalPin {
        activation,
        digest,
        request,
        #[cfg(test)]
        fixture_path: None,
    }))
}

/// Owns the slot lock through P. Consume directly without re-entering the lock;
/// dropping releases it before inference and normal Workflow finalization.
pub(crate) struct RegionalClaim {
    lease: Lease,
    pub request: RegionalRequest,
    digest: String,
}
impl RegionalPin {
    pub(crate) fn request(&self) -> &RegionalRequest {
        &self.request
    }
    pub(crate) fn claim(&self, document: Uuid, page: Uuid) -> Result<RegionalClaim> {
        #[cfg(test)]
        if let Some(path) = &self.fixture_path {
            let lease = Lease::reopen(path)?;
            lock(&lease)?;
            return self.claim_locked(lease, document, page);
        }
        let lease = caller(self.activation)?;
        self.claim_locked(lease, document, page)
    }
    fn claim_locked(&self, lease: Lease, document: Uuid, page: Uuid) -> Result<RegionalClaim> {
        ensure!(
            lease.record().activation() == self.activation,
            "regional Writer activation changed"
        );
        let pending = read(&lease, &name(&lease))?.context("regional Writer slot disappeared")?;
        ensure!(
            pending.digest == self.digest,
            "regional Writer slot changed"
        );
        if boot_seconds()? >= pending.expires {
            discard_locked(&lease, &name(&lease))?;
            anyhow::bail!("regional Writer slot expired");
        }
        ensure!(
            pending.page()? == (document, page),
            "regional Writer source page differs"
        );
        let InstalledRequest::GenerateRegion(request) = pending.request()? else {
            anyhow::bail!("regional Writer intent changed")
        };
        Ok(RegionalClaim {
            lease,
            request,
            digest: self.digest.clone(),
        })
    }
}
impl RegionalClaim {
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
    pub(crate) fn consume(self) -> Result<()> {
        self.lease.current()?;
        let name = name(&self.lease);
        let pending = read(&self.lease, &name)?.context("regional Writer slot disappeared")?;
        ensure!(
            pending.digest == self.digest && boot_seconds()? < pending.expires,
            "regional Writer claim changed or expired"
        );
        discard_locked(&self.lease, &name)?;
        self.lease.current()
    }
}

pub(crate) fn take(
    activation: uuid::Uuid,
    document: Uuid,
    page: Uuid,
) -> Result<Option<InstalledRequest>> {
    let lease = caller(activation)?;
    take_locked(&lease, document, page)
}

// The installed caller owns the existing slot lock. Keep this read/dispatch
// common to production and real-file ingress fixtures.
fn take_locked(lease: &Lease, document: Uuid, page: Uuid) -> Result<Option<InstalledRequest>> {
    let name = name(lease);
    let Some(pending) = read(lease, &name)? else {
        return Ok(None);
    };
    let expired = boot_seconds()? >= pending.expires;
    // An observed regional intent always belongs to the selected receiver.
    // Refinement's expiry/page None results mean Reader may continue, so they
    // must never hide this intent. Keep unexpired (including foreign) bytes;
    // retire an expired owned slot, but still refuse this ordinary attempt.
    if matches!(
        pending.submission,
        Submission::GenerateRegionOnTablet { .. }
    ) {
        if expired {
            discard_locked(lease, &name)?;
        }
        lease.current()?;
        anyhow::bail!("fresh regional Writer requires the selected receiver");
    }
    if !expired && pending.page()? != (document, page) {
        return Ok(None);
    }
    discard_locked(lease, &name)?;
    if expired {
        return Ok(None);
    }
    lease.current()?;
    Ok(Some(pending.request()?))
}

fn legacy_pending(lease: &Lease) -> Result<Option<LegacyAdoption>> {
    let Some(pending) = read(lease, &adoption_name(lease))? else {
        return Ok(None);
    };
    if boot_seconds()? >= pending.expires {
        return Ok(None);
    }
    let Submission::AdoptLegacyPage { approval } = pending.submission else {
        anyhow::bail!("unexpected command in adoption slot");
    };
    approval.validate(lease)?;
    Ok(Some(approval))
}

pub(crate) fn inspect_legacy(
    document: Uuid,
    source: Uuid,
    target: Uuid,
) -> Result<Option<LegacyAdoption>> {
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    lease.record.group.current_caller()?;
    lock(&lease)?;
    Ok(legacy_pending(&lease)?.filter(|approval| {
        (approval.document, approval.source, approval.target) == (document, source, target)
    }))
}

pub(crate) fn claim_legacy(approval: &LegacyAdoption) -> Result<()> {
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    lease.record.group.current_caller()?;
    lock(&lease)?;
    ensure!(
        legacy_pending(&lease)?.as_ref() == Some(approval),
        "legacy approval changed or expired"
    );
    open_at(&lease.directory, &approval.spent_name(), false, true)?.sync_all()?;
    lease.directory.sync_all()?;
    discard_locked(&lease, &adoption_name(&lease))?;
    lease.current()
}

fn discard_locked(lease: &Lease, name: &str) -> Result<()> {
    match fs::remove_file(lease.path.join(name)) {
        Ok(()) => lease.directory.sync_all().map_err(Into::into),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn discard(lease: &Lease) -> Result<()> {
    // Reopen so this short-lived lock is released before activation archival.
    let locked = Lease::reopen(&lease.path)?;
    lock(&locked)?;
    ensure!(
        locked.record().activation() == lease.record().activation(),
        "Writer cleanup activation changed"
    );
    discard_locked(&locked, &name(&locked))?;
    discard_locked(&locked, &adoption_name(&locked))
}

#[cfg(test)]
pub(crate) struct FixtureSlot {
    _fixture: super::tests::Fixture,
    pub(crate) pin: RegionalPin,
}
#[cfg(test)]
impl FixtureSlot {
    pub(crate) fn new(request: RegionalRequest) -> Self {
        let fixture = super::tests::Fixture::new();
        let path = fixture.lease_dir();
        let lease = Lease::publish_inner(&path, fixture.record()).unwrap();
        let submission = Submission::GenerateRegionOnTablet {
            document: request.document,
            page: request.page,
            operation: request.operation,
            request: request.request.clone(),
            guidance: request.guidance.clone(),
        };
        files::atomic_json(
            &path.join(name(&lease)),
            &Pending {
                submission,
                expires: boot_seconds().unwrap() + LIFETIME_SECS,
                digest: String::new(),
            },
        )
        .unwrap();
        let pending = read(&lease, &name(&lease)).unwrap().unwrap();
        let pin = RegionalPin {
            activation: lease.record().activation(),
            digest: pending.digest,
            request,
            fixture_path: Some(path),
        };
        Self {
            _fixture: fixture,
            pin,
        }
    }
    pub(crate) fn pending(&self) -> bool {
        let lease = Lease::reopen(self.pin.fixture_path.as_ref().unwrap()).unwrap();
        read(&lease, &name(&lease)).unwrap().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn request() -> RegionalRequest {
        RegionalRequest {
            document: Uuid::new_v4(),
            page: Uuid::new_v4(),
            operation: Uuid::new_v4(),
            request: "Make a note from the circled concept".into(),
            guidance: None,
        }
    }
    #[test]
    fn regional_slot_requires_exact_page_digest_activation_and_expiry() {
        for case in [
            "page",
            "document",
            "digest",
            "activation",
            "expired",
            "missing",
            "mode",
            "link",
        ] {
            let request = request();
            let slot = FixtureSlot::new(request.clone());
            let mut pin = slot.pin.clone();
            let lease = Lease::reopen(pin.fixture_path.as_ref().unwrap()).unwrap();
            let path = lease.path.join(name(&lease));
            let (document, page) = match case {
                "page" => (request.document, Uuid::new_v4()),
                "document" => (Uuid::new_v4(), request.page),
                _ => (request.document, request.page),
            };
            match case {
                "digest" => {
                    let mut bytes = fs::read(&path).unwrap();
                    bytes.push(b' ');
                    fs::write(&path, bytes).unwrap();
                }
                "activation" => pin.activation = Uuid::new_v4(),
                "expired" => {
                    let mut pending = read(&lease, &name(&lease)).unwrap().unwrap();
                    pending.expires = 0;
                    files::atomic_json(&path, &pending).unwrap();
                    pin.digest = crate::storage::digest(&fs::read(&path).unwrap());
                }
                "missing" => fs::remove_file(&path).unwrap(),
                "mode" => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
                "link" => fs::hard_link(&path, lease.path.join("extra-link")).unwrap(),
                _ => {}
            }
            assert!(pin.claim(document, page).is_err(), "{case}");
            if !matches!(case, "missing" | "expired") {
                assert!(
                    path.exists(),
                    "refusal must not discard a different/invalid input"
                );
            }
        }
    }
    #[test]
    fn regional_claim_consumes_only_exact_input_without_lock_reentry() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        let claim = slot.pin.claim(request.document, request.page).unwrap();
        assert!(
            slot.pin.claim(request.document, request.page).is_err(),
            "slot held through publication"
        );
        claim.consume().unwrap();
        assert!(!slot.pending());
        assert!(
            slot.pin.claim(request.document, request.page).is_err(),
            "consumed slot cannot become a new live input"
        );
    }
    #[test]
    fn regional_claim_preserves_replacement_on_changed_bytes() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        let claim = slot.pin.claim(request.document, request.page).unwrap();
        let path = claim.lease.path.join(name(&claim.lease));
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(b' ');
        fs::write(&path, &bytes).unwrap();
        assert!(claim.consume().is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    fn ordinary(
        slot: &FixtureSlot,
        document: Uuid,
        page: Uuid,
    ) -> Result<Option<InstalledRequest>> {
        let lease = Lease::reopen(slot.pin.fixture_path.as_ref().unwrap())?;
        lock(&lease)?;
        take_locked(&lease, document, page)
    }
    fn expire(slot: &FixtureSlot) {
        let lease = Lease::reopen(slot.pin.fixture_path.as_ref().unwrap()).unwrap();
        lock(&lease).unwrap();
        let mut pending = read(&lease, &name(&lease)).unwrap().unwrap();
        pending.expires = 0;
        files::atomic_json(&lease.path.join(name(&lease)), &pending).unwrap();
    }
    #[test]
    fn ordinary_ingress_matching_region_refuses_and_preserves_input() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        let result = ordinary(&slot, request.document, request.page);
        assert!(
            result.is_err(),
            "regional intent must explicitly refuse ordinary ingress"
        );
        assert!(
            slot.pending(),
            "unexpired regional input remains for the selected receiver"
        );
    }
    #[test]
    fn ordinary_ingress_expired_region_refuses_instead_of_returning_reader_none() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        expire(&slot);
        let result = ordinary(&slot, request.document, request.page);
        assert!(
            result.is_err(),
            "expired regional intent must not fall through to Reader"
        );
        assert!(!slot.pending(), "owned expired record may be retired");
    }
    #[test]
    fn ordinary_ingress_foreign_region_refuses_and_keeps_exact_bytes() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        let lease = Lease::reopen(slot.pin.fixture_path.as_ref().unwrap()).unwrap();
        let path = lease.path.join(name(&lease));
        let before = fs::read(&path).unwrap();
        for (document, page) in [
            (request.document, Uuid::new_v4()),
            (Uuid::new_v4(), request.page),
        ] {
            assert!(
                ordinary(&slot, document, page).is_err(),
                "foreign-page regional intent must not fall through to Reader"
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                before,
                "foreign queued input must remain exact"
            );
        }
    }
    #[test]
    fn ordinary_ingress_absent_slot_preserves_reader_none() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        slot.pin
            .claim(request.document, request.page)
            .unwrap()
            .consume()
            .unwrap();
        assert!(ordinary(&slot, request.document, request.page)
            .unwrap()
            .is_none());
    }
    #[test]
    fn ordinary_ingress_refinement_retains_page_expiry_and_consume_behavior() {
        let request = request();
        let slot = FixtureSlot::new(request.clone());
        let refinement = Request::Refine {
            conversation: Uuid::new_v4(),
            operation: Uuid::new_v4(),
            request: "Make the note shorter".into(),
            draft: Uuid::new_v4(),
        };
        let lease = Lease::reopen(slot.pin.fixture_path.as_ref().unwrap()).unwrap();
        let path = lease.path.join(name(&lease));
        let pending = Pending {
            submission: Submission::RefineOnTablet {
                document: request.document,
                page: request.page,
                refinement: refinement.clone(),
            },
            expires: boot_seconds().unwrap() + LIFETIME_SECS,
            digest: String::new(),
        };
        files::atomic_json(&path, &pending).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(ordinary(&slot, request.document, Uuid::new_v4())
            .unwrap()
            .is_none());
        assert_eq!(fs::read(&path).unwrap(), before);
        let Some(InstalledRequest::Refine(received)) =
            ordinary(&slot, request.document, request.page).unwrap()
        else {
            panic!("matching refinement was not delivered")
        };
        assert_eq!(
            serde_json::to_vec(&received).unwrap(),
            serde_json::to_vec(&refinement).unwrap()
        );
        assert!(!slot.pending());
        files::atomic_json(&path, &pending).unwrap();
        expire(&slot);
        assert!(ordinary(&slot, request.document, request.page)
            .unwrap()
            .is_none());
        assert!(!slot.pending());
    }
}
