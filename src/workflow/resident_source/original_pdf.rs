//! Read-only original publication custody around an admitted resident capture.
use crate::{
    device::native_page::{original_pdf_mapping, OriginalBookPage},
    retrieval::pdf_worker::{PendingIndex, Publication, PDF_BYTES},
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
pub(super) const INSTALLED_EXECUTABLE: &str = "/proc/self/exe";
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
        return Err(io::Error::last_os_error()).context("Open original document input");
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
            "Invalid original document directory"
        );
        let mut handles = Vec::new();
        let mut identities = Vec::new();
        for part in path.components() {
            charge(work, 2)?;
            let file = match part {
                Component::RootDir if handles.is_empty() => File::open("/")?,
                Component::Normal(name) => open_at(
                    handles.last().context("Original document root absent")?,
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?,
                _ => bail!("Invalid original document directory component"),
            };
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_dir()
                    && (metadata.uid() == 0 || metadata.uid() == uid)
                    && (metadata.mode() & 0o022 == 0
                        || allow_fixture_temp && metadata.mode() & 0o1000 != 0),
                "Original document directory is outside trusted ownership"
            );
            identities.push(Identity::from(&metadata));
            handles.push(file);
        }
        let identity = identities
            .last()
            .context("Original document directory absent")?;
        ensure!(
            identity.uid == uid
                && gid.is_none_or(|expected| identity.gid == expected)
                && identity.mode & 0o022 == 0,
            "Original document directory owner changed"
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
            "Original document directory changed"
        );
        Ok(candidate)
    }

    fn leaf(&self) -> &File {
        self.handles
            .last()
            .expect("validated original document directory")
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
            "Original document input is outside trusted file bounds"
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
            "Original document file changed"
        );
        Ok(Self { file, name, stamp })
    }

    fn current(&self, live: &Directory, work: &mut usize) -> Result<()> {
        charge(work, 3)?;
        let named = open_at(live.leaf(), OsStr::new(&self.name), libc::O_PATH)?;
        ensure!(
            Stamp::from(&self.file.metadata()?) == self.stamp
                && Stamp::from(&named.metadata()?) == self.stamp,
            "Original document file changed"
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
            ensure!(count > 0, "Original document input was truncated");
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
            "Original document input grew"
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
pub(in crate::workflow) struct OriginalFilePin {
    directory: Directory,
    table: Asset,
    original: Asset,
    samples_left: usize,
    expires: Instant,
}

/// A selected PDF capture also proves its one-based page in the original PDF.
pub struct OriginalPdfPin {
    file: OriginalFilePin,
    original_page: u32,
}

impl OriginalPdfPin {
    pub fn original_page(&self) -> u32 {
        self.original_page
    }

    pub fn check(&mut self, deadline: Instant) -> Result<()> {
        self.file.check(deadline)
    }
}

impl OriginalFilePin {
    /// Repeat at the preflight and ordered admission boundaries. This opens
    /// and stats the same names, without reading or parsing source content.
    pub fn check(&mut self, deadline: Instant) -> Result<()> {
        let deadline = deadline.min(self.expires);
        let mut remaining = std::mem::take(&mut self.samples_left);
        ensure!(Instant::now() < deadline, "Original document pin expired");
        charge(&mut remaining, 1)?;
        let mut work = METADATA_SAMPLE_WORK;
        let current = self.directory.current(&mut work)?;
        self.table.current(&current, &mut work)?;
        self.original.current(&current, &mut work)?;
        ensure!(Instant::now() < deadline, "Original document pin expired");
        self.samples_left = remaining;
        Ok(())
    }
}

/// Holds the named assets' identities across CaptureSource without reading
/// either file. The original can be absent when the current page is a note.
#[cfg(feature = "installed-resident-output")]
pub(in crate::workflow) struct OrdinaryBookSource {
    directory: Directory,
    table: Asset,
    pdf: Option<Result<Asset>>,
    epub: Option<Result<Asset>>,
}

#[cfg(feature = "installed-resident-output")]
impl OrdinaryBookSource {
    pub(super) fn pin(facts: &ReaderFacts, pdf: bool, epub: bool) -> Result<Self> {
        Self::pin_at(
            Path::new(INSTALLED_ROOT),
            (0, Some(0)),
            false,
            facts,
            pdf,
            epub,
        )
    }

    #[cfg(test)]
    pub(super) fn pin_fixture(
        root: &Path,
        facts: &ReaderFacts,
        pdf: bool,
        epub: bool,
    ) -> Result<Self> {
        Self::pin_at(
            root,
            (unsafe { libc::geteuid() }, None),
            true,
            facts,
            pdf,
            epub,
        )
    }

    fn pin_at(
        root: &Path,
        owner: (u32, Option<u32>),
        allow_fixture_temp: bool,
        facts: &ReaderFacts,
        pdf_enabled: bool,
        epub_enabled: bool,
    ) -> Result<Self> {
        let document = facts.page().document;
        let mut work = METADATA_SAMPLE_WORK;
        let directory = Directory::open(root, owner, allow_fixture_temp, &mut work)?;
        let table = Asset::open(
            &directory,
            format!("{document}.content"),
            TABLE_BYTES,
            &mut work,
        )?;
        let mut asset = |extension| {
            Asset::open(
                &directory,
                format!("{document}.{extension}"),
                PDF_BYTES,
                &mut work,
            )
        };
        let pdf = pdf_enabled.then(|| asset("pdf"));
        let epub = epub_enabled.then(|| asset("epub"));
        Ok(Self {
            directory,
            table,
            pdf,
            epub,
        })
    }

    pub(super) fn read(
        self,
        facts: &ReaderFacts,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Option<ObservedOriginalBook>> {
        let mut progress = SourceProgress::new(deadline, progress);
        progress.boundary()?;
        let mut work = METADATA_SAMPLE_WORK;
        let current = self.directory.current(&mut work)?;
        self.table.current(&current, &mut work)?;
        let pdf_enabled = self.pdf.is_some();
        let epub_enabled = self.epub.is_some();
        ObservedOriginalBook::read_mapped(
            self.directory,
            self.table,
            |_, page, work| {
                let asset = match page {
                    OriginalBookPage::Pdf { .. } => self.pdf,
                    OriginalBookPage::Epub => self.epub,
                }
                .context("Original document format disabled")??;
                asset.current(&current, work)?;
                Ok(asset)
            },
            facts,
            progress,
            work,
            |raw, facts| {
                crate::device::native_page::ordinary_book_mapping(
                    raw,
                    facts,
                    pdf_enabled,
                    epub_enabled,
                )
            },
        )
    }
}

/// One bounded normalized input. The file identities contain no open
/// descriptors, page authority, or live worker.
#[cfg(feature = "installed-resident-output")]
pub(in crate::workflow) struct OrdinaryBookCache {
    directory: Vec<Identity>,
    original: Stamp,
    units: crate::retrieval::pdf_worker::CachedNormalization,
}

pub struct ObservedOriginalBook {
    observed: ReaderFacts,
    pin: OriginalFilePin,
    table_digest: String,
    original_digest: String,
    bytes: Vec<u8>,
    page: OriginalBookPage,
    reads_left: usize,
    source_bytes_left: usize,
    acquisition_deadline: Instant,
}

pub type ObservedOriginalPdf = ObservedOriginalBook;

impl ObservedOriginalBook {
    pub(in crate::workflow) fn publication(&self) -> Publication {
        match self.page {
            OriginalBookPage::Pdf { pages, .. } => Publication::Pdf { pages },
            OriginalBookPage::Epub => Publication::Epub,
        }
    }

    fn pdf_page(&self) -> Result<(u32, u32)> {
        match self.page {
            OriginalBookPage::Pdf { pages, redirect } => Ok((pages, redirect + 1)),
            OriginalBookPage::Epub => bail!("Selected PDF requires an original PDF page"),
        }
    }
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
    pub(super) fn observe_at(
        root: &Path,
        owner: (u32, Option<u32>),
        allow_fixture_temp: bool,
        observed: &ReaderFacts,
        admitted_document: Uuid,
        acquisition_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        Self::observe_mapped(
            root,
            owner,
            allow_fixture_temp,
            observed,
            admitted_document,
            acquisition_deadline,
            progress,
            |raw, facts| {
                original_pdf_mapping(raw, facts)
                    .map(|(pages, redirect)| Some(OriginalBookPage::Pdf { pages, redirect }))
            },
        )?
        .context("Original PDF mapping unavailable")
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_mapped(
        root: &Path,
        owner: (u32, Option<u32>),
        allow_fixture_temp: bool,
        observed: &ReaderFacts,
        admitted_document: Uuid,
        acquisition_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
        mapping: impl FnOnce(&[u8], &ReaderFacts) -> Result<Option<OriginalBookPage>>,
    ) -> Result<Option<Self>> {
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
        Self::read_mapped(
            directory,
            table,
            |directory, _, work| {
                Asset::open(
                    directory,
                    format!("{admitted_document}.pdf"),
                    PDF_BYTES,
                    work,
                )
            },
            observed,
            progress,
            work,
            mapping,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn read_mapped(
        directory: Directory,
        table: Asset,
        open_original: impl FnOnce(&Directory, OriginalBookPage, &mut usize) -> Result<Asset>,
        observed: &ReaderFacts,
        mut progress: SourceProgress<'_>,
        mut work: usize,
        mapping: impl FnOnce(&[u8], &ReaderFacts) -> Result<Option<OriginalBookPage>>,
    ) -> Result<Option<Self>> {
        let acquisition_deadline = progress.deadline;
        let mut reads_left = SOURCE_READS;
        let mut source_bytes_left = SOURCE_BYTES;
        let mut raw_table = Vec::with_capacity(table.stamp.size.try_into()?);
        let table_digest = table.read(
            Some(&mut raw_table),
            &mut reads_left,
            &mut source_bytes_left,
            &mut progress,
        )?;
        let Some(page) = mapping(&raw_table, observed)? else {
            let mut work = METADATA_SAMPLE_WORK;
            let current = directory.current(&mut work)?;
            table.current(&current, &mut work)?;
            progress.boundary()?;
            return Ok(None);
        };
        let original = open_original(&directory, page, &mut work)?;
        let mut pin = OriginalFilePin {
            directory,
            table,
            original,
            samples_left: METADATA_SAMPLES - 1,
            expires: acquisition_deadline,
        };
        let mut bytes = Vec::with_capacity(pin.original.stamp.size.try_into()?);
        let original_digest = pin.original.read(
            Some(&mut bytes),
            &mut reads_left,
            &mut source_bytes_left,
            &mut progress,
        )?;
        pin.check(acquisition_deadline)?;
        progress.boundary()?;
        Ok(Some(Self {
            observed: observed.clone(),
            pin,
            table_digest,
            original_digest,
            bytes,
            page,
            reads_left,
            source_bytes_left,
            acquisition_deadline,
        }))
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

    /// The resident owner has already checked both HistoricalCapture samples,
    /// the Observe/Capture operation identities and the runtime correspondence
    /// through ResidentSourceCapture::from_session.
    pub(in crate::workflow::resident_source) fn captured_by_owner(
        self,
        verified: &ReaderFacts,
        request_deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<CapturedOriginalPdf> {
        self.recheck_verified(verified, progress)?.start_index(
            Path::new(INSTALLED_EXECUTABLE),
            request_deadline,
            progress,
        )
    }

    #[cfg(feature = "installed-resident-output")]
    pub(super) fn complete_ordinary(
        self,
        verified: &ReaderFacts,
        executable: &Path,
        deadline: Instant,
        retained: Option<OrdinaryBookCache>,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<(CapturedOriginalBook, Option<OrdinaryBookCache>)> {
        use crate::retrieval::pdf_worker::OriginalInput;

        // Each request checks its own table, including pages inserted since the
        // last answer. Only the original book's identity carries across requests.
        let mut source = self.recheck_verified(verified, progress)?;
        let units = retained
            .filter(|cache| {
                cache.directory == source.pin.directory.identities
                    && cache.original == source.pin.original.stamp
            })
            .map(|cache| cache.units);
        let document = Uuid::from_bytes(*verified.page().document.as_bytes());
        let (index, units) = PendingIndex::start_ordinary(
            executable,
            OriginalInput {
                document,
                bytes: &source.bytes,
                publication: source.publication(),
                sha256: source.original_digest,
            },
            (source.acquisition_deadline, deadline),
            units,
            progress,
        )?;
        let cache = units.map(|units| OrdinaryBookCache {
            directory: source.pin.directory.identities.clone(),
            original: source.pin.original.stamp.clone(),
            units,
        });
        source.pin.expires = deadline;
        Ok((
            CapturedOriginalBook {
                pin: source.pin,
                index,
            },
            cache,
        ))
    }

    #[cfg(test)]
    pub(super) fn verify_fixture_capture(
        self,
        verified: &ReaderFacts,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<u32> {
        Ok(self.recheck_verified(verified, progress)?.pdf_page()?.1)
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
        self,
        before: &ReaderFacts,
        after: &ReaderFacts,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            before == after,
            "Original PDF capture correspondence changed"
        );
        self.recheck_verified(before, progress)
    }

    fn recheck_verified(
        mut self,
        verified: &ReaderFacts,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            verified == &self.observed,
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
        let original_digest = self.pin.original.read(
            None,
            &mut self.reads_left,
            &mut self.source_bytes_left,
            &mut progress,
        )?;
        self.pin.check(self.acquisition_deadline)?;
        ensure!(
            table_digest == self.table_digest && original_digest == self.original_digest,
            "Original document bytes changed around capture"
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
        let (pages, original_page) = self.pdf_page()?;
        let index = PendingIndex::start(
            executable,
            document,
            &self.bytes,
            self.original_digest,
            pages,
            self.acquisition_deadline,
            request_deadline,
            progress,
        )?;
        self.pin.expires = request_deadline;
        Ok(CapturedOriginalPdf {
            pin: OriginalPdfPin {
                file: self.pin,
                original_page,
            },
            index,
        })
    }
}

pub struct CapturedOriginalPdf {
    pub pin: OriginalPdfPin,
    pub index: PendingIndex,
}

#[cfg(feature = "installed-resident-output")]
pub(in crate::workflow) struct CapturedOriginalBook {
    pub pin: OriginalFilePin,
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

#[cfg(all(test, feature = "installed-resident-output"))]
pub(crate) use tests::assert_ordinary_pdf_reuse;
#[cfg(test)]
mod tests;
