//! One local Writer request scoped to the installed activation and a page.
use super::{libc, open_at, recovery, Lease};
use crate::{
    device::request_input::boot_seconds,
    storage::{files, Uuid},
    writer::Request,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
}

/// Identity observed when a Writer-only corner gesture is released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectedRefinement {
    activation: Uuid,
    slot_digest: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotAction {
    Inspect,
    Claim,
}

impl Pending {
    fn refinement(&self, activation: Uuid, now: u64) -> Result<Option<SelectedRefinement>> {
        // Legacy and current slots record their queue time in whole boot seconds.
        // A fresh three-second hold orders the request and release unambiguously.
        if self
            .expires
            .checked_sub(LIFETIME_SECS)
            .is_none_or(|queued| queued >= now)
            || now >= self.expires
            || !matches!(
                &self.submission,
                Submission::RefineOnTablet {
                    refinement: Request::Refine { .. },
                    ..
                }
            )
        {
            return Ok(None);
        }
        Ok(Some(SelectedRefinement {
            activation,
            slot_digest: Sha256::digest(serde_json::to_vec(self)?).into(),
        }))
    }
}

impl Submission {
    pub(crate) fn identities(&self) -> (Uuid, Uuid) {
        match self {
            Self::RefineOnTablet { refinement, .. } => {
                (refinement.conversation(), refinement.operation())
            }
            Self::AdoptLegacyPage { approval } => (Uuid::nil(), approval.operation),
        }
    }
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
    Ok(Some(serde_json::from_slice(&bytes)?))
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
        Submission::RefineOnTablet { .. } => name(&lease),
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
        },
    )?;
    lease.current()
}

pub(crate) fn at_release(
    lease: &Lease,
    released_boot_second: u64,
) -> Result<Option<SelectedRefinement>> {
    lease.record.group.current_caller()?;
    // The current draft and its page binding are verified before a claim.
    let pending = read(lease, &name(lease))?;
    lease.current()?;
    match pending {
        Some(slot) if boot_seconds()? < slot.expires => {
            slot.refinement(lease.record().activation(), released_boot_second)
        }
        _ => Ok(None),
    }
}

pub(crate) fn request_for_page(
    activation: Uuid,
    document: Uuid,
    page: Uuid,
    selected: SelectedRefinement,
    action: SlotAction,
) -> Result<Option<Request>> {
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    ensure!(
        lease.record().activation() == activation,
        "Writer activation changed"
    );
    lease.record.group.current_caller()?;
    lock(&lease)?;
    request_locked(&lease, document, page, selected, action, boot_seconds()?)
}

fn request_locked(
    lease: &Lease,
    document: Uuid,
    page: Uuid,
    selected: SelectedRefinement,
    action: SlotAction,
    now: u64,
) -> Result<Option<Request>> {
    let name = name(lease);
    let Some(pending) = read(lease, &name)? else {
        return Ok(None);
    };
    if pending.refinement(lease.record().activation(), now)? != Some(selected) {
        return Ok(None);
    }
    let Submission::RefineOnTablet {
        document: expected_document,
        page: expected_page,
        refinement,
    } = pending.submission
    else {
        anyhow::bail!("unexpected command in Writer slot");
    };
    if document != expected_document || page != expected_page {
        return Ok(None);
    }
    if action == SlotAction::Claim {
        discard_locked(lease, &name)?;
    }
    lease.current()?;
    Ok(Some(refinement))
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
mod tests {
    use super::*;

    #[test]
    fn writer_only_corner_consumes_only_its_live_request_and_named_page() {
        let fixture = super::super::tests::Fixture::new();
        let lease = Lease::publish_inner(&fixture.lease_dir(), fixture.record()).unwrap();
        lock(&lease).unwrap();
        let document = Uuid::new_v4();
        let page = Uuid::new_v4();
        let operation = Uuid::new_v4();
        let mut slot = Pending {
            submission: Submission::RefineOnTablet {
                document,
                page,
                refinement: Request::Refine {
                    conversation: Uuid::new_v4(),
                    operation,
                    draft: Uuid::new_v4(),
                    request: "Shorten the note".into(),
                },
            },
            expires: 120,
        };
        let path = lease.path.join(name(&lease));
        files::atomic_json(&path, &slot).unwrap();
        let activation = lease.record().activation();
        let selected = slot.refinement(activation, 118).unwrap().unwrap();
        assert!(request_locked(
            &lease,
            document,
            Uuid::new_v4(),
            selected,
            SlotAction::Claim,
            119
        )
        .unwrap()
        .is_none());
        assert!(
            request_locked(&lease, document, page, selected, SlotAction::Inspect, 119)
                .unwrap()
                .is_some()
        );
        assert!(path.is_file());
        assert!(
            request_locked(&lease, document, page, selected, SlotAction::Claim, 120)
                .unwrap()
                .is_none()
        );
        assert!(path.is_file());
        slot.expires = 240;
        files::atomic_json(&path, &slot).unwrap();
        assert!(slot.refinement(activation, 119).unwrap().is_none());
        assert!(slot.refinement(activation, 120).unwrap().is_none());
        assert!(
            request_locked(&lease, document, page, selected, SlotAction::Claim, 121)
                .unwrap()
                .is_none()
        );
        let fresh = slot.refinement(activation, 121).unwrap().unwrap();
        let result = request_locked(&lease, document, page, fresh, SlotAction::Claim, 122)
            .unwrap()
            .unwrap();
        assert_eq!(result.operation(), operation);
        assert!(
            request_locked(&lease, document, page, fresh, SlotAction::Claim, 122)
                .unwrap()
                .is_none()
        );
    }
}
