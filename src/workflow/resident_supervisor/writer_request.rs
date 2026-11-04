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
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    submission: Submission,
    expires: u64,
}

impl Submission {
    pub(crate) fn refinement(&self) -> &Request {
        let Self::RefineOnTablet { refinement, .. } = self;
        refinement
    }
}

fn boot_seconds() -> Result<u64> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
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

fn read(lease: &Lease, name: &str) -> Result<Option<Pending>> {
    let file = match open_at(&lease.directory, name, false, false) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.uid() == unsafe { libc::geteuid() }
            && meta.nlink() == 1 && meta.mode() & 0o7777 == 0o600
            && meta.len() <= LIMIT,
        "invalid Writer request slot"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 == meta.len(), "Writer request slot changed");
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub(crate) fn submit(submission: Submission) -> Result<()> {
    let Submission::RefineOnTablet { document, page, refinement } = &submission;
    ensure!(!document.is_nil() && !page.is_nil(), "empty Writer source");
    let Request::Refine { .. } = refinement else {
        anyhow::bail!("explicit Writer refinement required");
    };
    refinement.validate()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let pin = recovery::activation::InstalledReaderSelection::load(deadline)?
        .context("installed Writer requires resident selection")?
        .output(deadline)?
        .context("installed Writer output is unavailable")?;
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    lock(&lease)?;
    ensure!(pin.digest() == lease.record.recovery_profile_sha256, "Writer activation pin changed");
    lease.record.group.current_process()?;
    let name = name(&lease);
    let now = boot_seconds()?;
    ensure!(read(&lease, &name)?.is_none_or(|request| request.expires <= now),
        "a Writer refinement is already awaiting its page");
    files::atomic_json(&lease.path.join(name), &Pending {
        submission,
        expires: now + LIFETIME_SECS,
    })?;
    lease.current()
}

pub(crate) fn take(
    activation: uuid::Uuid,
    document: Uuid,
    page: Uuid,
) -> Result<Option<Request>> {
    let lease = Lease::reopen(Path::new(recovery::LEASE_DIRECTORY))?;
    ensure!(lease.record().activation() == activation, "Writer activation changed");
    lease.record.group.current_caller()?;
    lock(&lease)?;
    let name = name(&lease);
    let Some(pending) = read(&lease, &name)? else { return Ok(None) };
    let expired = boot_seconds()? >= pending.expires;
    let Submission::RefineOnTablet {
        document: expected_document, page: expected_page, refinement,
    } = pending.submission;
    if !expired && (document != expected_document || page != expected_page) {
        return Ok(None);
    }
    discard_locked(&lease, &name)?;
    if expired {
        return Ok(None);
    }
    lease.current()?;
    Ok(Some(refinement))
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
    ensure!(locked.record().activation() == lease.record().activation(), "Writer cleanup activation changed");
    discard_locked(&locked, &name(&locked))
}
