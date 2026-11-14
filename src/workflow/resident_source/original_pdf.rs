//! Read-only original PDF custody around an admitted resident capture.
use crate::{
    device::native_page::original_pdf_mapping,
    retrieval::pdf_worker::{PendingIndex, PDF_BYTES},
    storage::Uuid,
};
use anyhow::{bail, ensure, Context, Result};
use nix::libc;
use remarkable_open_sdk::native_adapter::{HistoricalCapture, ReaderFacts};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr},
    fs::{File, Metadata, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    time::Instant,
};

const INSTALLED_ROOT: &str = "/home/root/.local/share/remarkable/xochitl";
const INSTALLED_EXECUTABLE: &str = "/proc/self/exe";
const TABLE_BYTES: usize = 1024 * 1024;
const CHUNK_BYTES: usize = 32 * 1024;
const SOURCE_READS: usize = 2 * (PDF_BYTES / CHUNK_BYTES + TABLE_BYTES / CHUNK_BYTES + 2);
const SOURCE_BYTES: usize = 2 * (PDF_BYTES + TABLE_BYTES);
const METADATA_SAMPLE_WORK: usize = 64;
const METADATA_SAMPLES: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}
impl From<&Metadata> for Identity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    identity: Identity,
    size: u64,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl From<&Metadata> for Stamp {
    fn from(metadata: &Metadata) -> Self {
        Self {
            identity: Identity::from(metadata),
            size: metadata.len(),
            links: metadata.nlink(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn open_at(parent: &File, name: &OsStr, flags: i32) -> Result<File> {
    let name = CString::new(name.as_bytes())?;
    // The name is a single normal path component. All descriptors are read-only
    // and close-on-exec. Nonblocking avoids waiting on a replaced FIFO.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("Open original PDF input");
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

struct Directory {
    path: PathBuf,
    handles: Vec<File>,
    identities: Vec<Identity>,
    uid: u32,
    gid: Option<u32>,
    allow_fixture_temp: bool,
}

impl Directory {
    fn open(
        path: &Path,
        (uid, gid): (u32, Option<u32>),
        allow_fixture_temp: bool,
        work: &mut usize,
    ) -> Result<Self> {
        ensure!(
            path.is_absolute() && path.components().count() <= 16,
            "Invalid original PDF directory"
        );
        let mut handles = Vec::new();
        let mut identities = Vec::new();
        for part in path.components() {
            charge(work, 2)?;
            let file = match part {
                Component::RootDir if handles.is_empty() => File::open("/")?,
                Component::Normal(name) => open_at(
                    handles.last().context("Original PDF root absent")?,
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?,
                _ => bail!("Invalid original PDF directory component"),
            };
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_dir()
                    && (metadata.uid() == 0 || metadata.uid() == uid)
                    && (metadata.mode() & 0o022 == 0
                        || allow_fixture_temp && metadata.mode() & 0o1000 != 0),
                "Original PDF directory is outside trusted ownership"
            );
            identities.push(Identity::from(&metadata));
            handles.push(file);
        }
        let identity = identities.last().context("Original PDF directory absent")?;
        ensure!(
            identity.uid == uid
                && gid.is_none_or(|expected| identity.gid == expected)
                && identity.mode & 0o022 == 0,
            "Original PDF directory owner changed"
        );
        Ok(Self {
            path: path.into(),
            handles,
            identities,
            uid,
            gid,
            allow_fixture_temp,
        })
    }

    fn current(&self, work: &mut usize) -> Result<Self> {
        let candidate = Self::open(
            &self.path,
            (self.uid, self.gid),
            self.allow_fixture_temp,
            work,
        )?;
        ensure!(
            candidate.identities == self.identities,
            "Original PDF directory changed"
        );
        Ok(candidate)
    }

    fn leaf(&self) -> &File {
        self.handles
            .last()
            .expect("validated original PDF directory")
    }
}

struct Asset {
    file: File,
    name: String,
    stamp: Stamp,
}

impl Asset {
    fn open(directory: &Directory, name: String, max: usize, work: &mut usize) -> Result<Self> {
        charge(work, 4)?;
        let entry = open_at(directory.leaf(), OsStr::new(&name), libc::O_PATH)?;
        let metadata = entry.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == directory.uid
                && directory.gid.is_none_or(|gid| metadata.gid() == gid)
                && metadata.nlink() == 1
                && metadata.mode() & 0o022 == 0
                && (1..=max as u64).contains(&metadata.len()),
            "Original PDF input is outside trusted file bounds"
        );
        // Reopen the already validated regular inode, rather than opening
        // the name again and risking a substituted device or FIFO.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(format!("/proc/self/fd/{}", entry.as_raw_fd()))?;
        let stamp = Stamp::from(&metadata);
        ensure!(
            Stamp::from(&file.metadata()?) == stamp,
            "Original PDF file changed"
        );
        Ok(Self { file, name, stamp })
    }

    fn current(&self, live: &Directory, work: &mut usize) -> Result<()> {
        charge(work, 3)?;
        let named = open_at(live.leaf(), OsStr::new(&self.name), libc::O_PATH)?;
        ensure!(
            Stamp::from(&self.file.metadata()?) == self.stamp
                && Stamp::from(&named.metadata()?) == self.stamp,
            "Original PDF file changed"
        );
        Ok(())
    }

    fn read(
        &self,
        mut retain: Option<&mut Vec<u8>>,
        reads: &mut usize,
        source_bytes: &mut usize,
        progress: &mut SourceProgress<'_>,
    ) -> Result<String> {
        let size = usize::try_from(self.stamp.size)?;
        charge(source_bytes, size)?;
        let mut chunk = [0; CHUNK_BYTES];
        let mut hash = Sha256::new();
        let mut offset = 0;
        while offset < size {
            progress.poll()?;
            charge(reads, 1)?;
            let length = (size - offset).min(CHUNK_BYTES);
            let count = self.file.read_at(&mut chunk[..length], offset as u64)?;
            ensure!(count > 0, "Original PDF input was truncated");
            hash.update(&chunk[..count]);
            if let Some(bytes) = retain.as_mut() {
                bytes.extend_from_slice(&chunk[..count]);
            }
            offset += count;
        }
        progress.poll()?;
        charge(reads, 1)?;
        ensure!(
            self.file.read_at(&mut [0], size as u64)? == 0,
            "Original PDF input grew"
        );
        progress.poll()?;
        Ok(format!("{:x}", hash.finalize()))
    }
}

fn charge(remaining: &mut usize, amount: usize) -> Result<()> {
    *remaining = remaining
        .checked_sub(amount)
        .context("Original PDF allowance exhausted")?;
    Ok(())
}

struct SourceProgress<'a> {
    deadline: Instant,
    next: Instant,
    callback: &'a mut dyn FnMut() -> Result<()>,
}

impl<'a> SourceProgress<'a> {
    fn new(deadline: Instant, callback: &'a mut dyn FnMut() -> Result<()>) -> Self {
        Self {
            deadline,
            next: Instant::now(),
            callback,
        }
    }

    fn check_time(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.deadline,
            "Original PDF admission expired"
        );
        Ok(())
    }

    fn poll(&mut self) -> Result<()> {
        self.check_time()?;
        if Instant::now() >= self.next {
            self.boundary()?;
        }
        Ok(())
    }

    fn boundary(&mut self) -> Result<()> {
        self.check_time()?;
        (self.callback)()?;
        self.next = Instant::now() + crate::workflow::indicator::CADENCE;
        self.check_time()
    }
}

/// The request-local proof is a sampled file identity, with no native lease.
pub struct OriginalPdfPin {
    directory: Directory,
    table: Asset,
    pdf: Asset,
    samples_left: usize,
    original_page: u32,
    expires: Instant,
}

impl OriginalPdfPin {
    pub fn original_page(&self) -> u32 {
        self.original_page
    }

    /// Repeat at the preflight and ordered admission boundaries. This opens
    /// and stats the same names, without reading or parsing source content.
    pub fn check(&mut self, deadline: Instant) -> Result<()> {
        let deadline = deadline.min(self.expires);
        let mut remaining = std::mem::take(&mut self.samples_left);
        ensure!(Instant::now() < deadline, "Original PDF pin expired");
        charge(&mut remaining, 1)?;
        let mut work = METADATA_SAMPLE_WORK;
        let current = self.directory.current(&mut work)?;
        self.table.current(&current, &mut work)?;
        self.pdf.current(&current, &mut work)?;
        ensure!(Instant::now() < deadline, "Original PDF pin expired");
        self.samples_left = remaining;
        Ok(())
    }
}

pub struct ObservedOriginalPdf {
    observed: ReaderFacts,
    pin: OriginalPdfPin,
    table_digest: String,
    pdf_digest: String,
    bytes: Vec<u8>,
    pages: u32,
    reads_left: usize,
    source_bytes_left: usize,
    acquisition_deadline: Instant,
}

impl ObservedOriginalPdf {
    /// Call with the installed Reader's observed facts and already admitted
    /// document identity, before sending the matching CaptureSource request.
    pub fn observe(
        observed: &ReaderFacts,
        admitted_document: Uuid,
        acquisition_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        Self::observe_at(
            Path::new(INSTALLED_ROOT),
            (0, Some(0)),
            false,
            observed,
            admitted_document,
            acquisition_deadline,
            progress,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_at(
        root: &Path,
        owner: (u32, Option<u32>),
        allow_fixture_temp: bool,
        observed: &ReaderFacts,
        admitted_document: Uuid,
        acquisition_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            !admitted_document.is_nil()
                && observed.page().document.as_bytes() == admitted_document.as_bytes(),
            "Original PDF does not match admitted document"
        );
        let mut progress = SourceProgress::new(acquisition_deadline, progress);
        progress.boundary()?;
        // One initial metadata sample includes holding and validating both names.
        let mut work = METADATA_SAMPLE_WORK;
        let directory = Directory::open(root, owner, allow_fixture_temp, &mut work)?;
        let table = Asset::open(
            &directory,
            format!("{admitted_document}.content"),
            TABLE_BYTES,
            &mut work,
        )?;
        let pdf = Asset::open(
            &directory,
            format!("{admitted_document}.pdf"),
            PDF_BYTES,
            &mut work,
        )?;
        let mut pin = OriginalPdfPin {
            directory,
            table,
            pdf,
            samples_left: METADATA_SAMPLES - 1,
            original_page: 0,
            expires: acquisition_deadline,
        };
        let mut reads_left = SOURCE_READS;
        let mut source_bytes_left = SOURCE_BYTES;
        let mut table = Vec::with_capacity(pin.table.stamp.size.try_into()?);
        let table_digest = pin.table.read(
            Some(&mut table),
            &mut reads_left,
            &mut source_bytes_left,
            &mut progress,
        )?;
        let mut bytes = Vec::with_capacity(pin.pdf.stamp.size.try_into()?);
        let pdf_digest = pin.pdf.read(
            Some(&mut bytes),
            &mut reads_left,
            &mut source_bytes_left,
            &mut progress,
        )?;
        pin.check(acquisition_deadline)?;
        let (pages, redirect) = original_pdf_mapping(&table, observed)?;
        pin.original_page = redirect + 1;
        progress.boundary()?;
        Ok(Self {
            observed: observed.clone(),
            pin,
            table_digest,
            pdf_digest,
            bytes,
            pages,
            reads_left,
            source_bytes_left,
            acquisition_deadline,
        })
    }

    /// Consume the exact captured facts returned by the already checked
    /// resident Session, before Prepare. All source reads and worker waits
    /// occur outside the selected admission gates.
    pub fn captured(
        self,
        capture: &HistoricalCapture,
        request_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<CapturedOriginalPdf> {
        self.captured_with_executable(
            capture,
            Path::new(INSTALLED_EXECUTABLE),
            request_deadline,
            progress,
        )
    }

    fn captured_with_executable(
        self,
        capture: &HistoricalCapture,
        executable: &Path,
        request_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<CapturedOriginalPdf> {
        self.recheck_capture(capture.before(), capture.after(), progress)?
            .start_index(executable, request_deadline, progress)
    }

    fn recheck_capture(
        mut self,
        before: &ReaderFacts,
        after: &ReaderFacts,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            before == &self.observed && after == &self.observed,
            "Original PDF capture correspondence changed"
        );
        let mut progress = SourceProgress::new(self.acquisition_deadline, progress);
        progress.boundary()?;
        self.pin.check(self.acquisition_deadline)?;
        let table_digest = self.pin.table.read(
            None,
            &mut self.reads_left,
            &mut self.source_bytes_left,
            &mut progress,
        )?;
        let pdf_digest = self.pin.pdf.read(
            None,
            &mut self.reads_left,
            &mut self.source_bytes_left,
            &mut progress,
        )?;
        self.pin.check(self.acquisition_deadline)?;
        ensure!(
            table_digest == self.table_digest && pdf_digest == self.pdf_digest,
            "Original PDF bytes changed around capture"
        );
        progress.boundary()?;
        Ok(self)
    }

    fn start_index(
        mut self,
        executable: &Path,
        request_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<CapturedOriginalPdf> {
        let document = Uuid::from_bytes(*self.observed.page().document.as_bytes());
        let index = PendingIndex::start(
            executable,
            document,
            &self.bytes,
            self.pdf_digest,
            self.pages,
            self.acquisition_deadline,
            request_deadline,
            progress,
        )?;
        self.pin.expires = request_deadline;
        Ok(CapturedOriginalPdf {
            pin: self.pin,
            index,
        })
    }
}

pub struct CapturedOriginalPdf {
    pub pin: OriginalPdfPin,
    pub index: PendingIndex,
}

/// Hosted fixture entry for the actual producer and worker. Installed Reader
/// always uses the fixed Xochitl root and the running executable.
#[cfg(feature = "development-resident-bootstrap")]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn development_original_pdf(
    root: &Path,
    executable: &Path,
    document: Uuid,
    observed: &ReaderFacts,
    capture: &HistoricalCapture,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<CapturedOriginalPdf> {
    // SAFETY: geteuid does not take pointers or modify process state.
    let uid = unsafe { libc::geteuid() };
    ObservedOriginalPdf::observe_at(
        root,
        (uid, None),
        true,
        observed,
        document,
        deadline,
        progress,
    )?
    .captured_with_executable(capture, executable, deadline, progress)
}

#[cfg(test)]
mod tests;
