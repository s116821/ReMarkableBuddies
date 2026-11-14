//! Owned, one-query PDF normalization in the installed Reader executable.
use super::{pdf, Index, IndexKey, Retrieved, SourceFormat};
use crate::storage::{digest, Uuid};
use anyhow::{bail, ensure, Context, Result};
use nix::libc;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

pub const ARGUMENT: &str = "--internal-reader-pdf-worker-v1";
pub(crate) const PDF_BYTES: usize = 16 * 1024 * 1024;
const READY_BYTES: usize = 9 * 1024 * 1024;
const HEADER_BYTES: usize = 1024;
const QUERY_BYTES: usize = 4096;
const PASSAGE_BYTES: usize = 16 * 1024;
const RESULT_BYTES: usize = PASSAGE_BYTES + HEADER_BYTES;
// Legal largest frames need fewer than 7,000 transfers on a 4 KiB pipe.
// Refuse if capacity, partial reads or readiness use up the remainder.
const IPC_CALLS: usize = 8192;
const CHUNK_BYTES: usize = 32 * 1024;
const VERSION: u8 = 1;
const INPUT: u8 = 1;
const READY: u8 = 2;
const QUERY: u8 = 3;
const OUTPUT: u8 = 4;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    request: Uuid,
    document: Uuid,
    bytes: usize,
    sha256: String,
    pages: u32,
    extractor_revision: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    request: Uuid,
    key: IndexKey,
    pages: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    request: Uuid,
    key: IndexKey,
    max_passages: usize,
    max_bytes: usize,
    neighbors: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    request: Uuid,
    result: Retrieved,
}

fn packet<T: Serialize>(header: &T, bytes: &[u8], cap: usize) -> Result<Vec<u8>> {
    let header = serde_json::to_vec(header)?;
    ensure!(
        header.len() <= HEADER_BYTES,
        "PDF worker header exceeds limit"
    );
    let length = 4 + header.len() + bytes.len();
    ensure!(length <= cap, "PDF worker payload exceeds limit");
    let mut result = Vec::with_capacity(length);
    result.extend_from_slice(&u32::try_from(header.len())?.to_le_bytes());
    result.extend_from_slice(&header);
    result.extend_from_slice(bytes);
    Ok(result)
}

fn unpack<T: DeserializeOwned>(packet: &[u8]) -> Result<(T, &[u8])> {
    let header = packet.get(..4).context("PDF worker header absent")?;
    let length = u32::from_le_bytes(header.try_into()?) as usize;
    ensure!(length <= HEADER_BYTES, "PDF worker header exceeds limit");
    let header = packet
        .get(4..4 + length)
        .context("PDF worker header truncated")?;
    Ok((serde_json::from_slice(header)?, &packet[4 + length..]))
}

fn frame(kind: u8, length: usize) -> Result<[u8; 6]> {
    let mut header = [VERSION, kind, 0, 0, 0, 0];
    header[2..].copy_from_slice(&u32::try_from(length)?.to_le_bytes());
    Ok(header)
}

fn length(header: [u8; 6], kind: u8, cap: usize) -> Result<usize> {
    ensure!(
        header[..2] == [VERSION, kind],
        "Unexpected PDF worker frame"
    );
    let size = u32::from_le_bytes(header[2..].try_into()?) as usize;
    ensure!(size > 0 && size <= cap, "PDF worker frame exceeds limit");
    Ok(size)
}

fn read_frame(input: &mut impl Read, kind: u8, cap: usize) -> Result<Vec<u8>> {
    let mut header = [0; 6];
    input.read_exact(&mut header)?;
    let mut payload = vec![0; length(header, kind, cap)?];
    input.read_exact(&mut payload)?;
    Ok(payload)
}

fn write_frame(output: &mut impl Write, kind: u8, payload: &[u8]) -> Result<()> {
    output.write_all(&frame(kind, payload.len())?)?;
    output.write_all(payload)?;
    output.flush()?;
    Ok(())
}

fn query_valid(query: &Query, question: &str) -> bool {
    !question.trim().is_empty()
        && question.len() <= 2048
        && !question.contains('\0')
        && (1..=16).contains(&query.max_passages)
        && (512..=PASSAGE_BYTES).contains(&query.max_bytes)
}

fn close_extra_descriptors() -> Result<()> {
    // This runs at worker entry before starting threads. Drop the directory
    // iterator before closing the descriptors sampled from it.
    let descriptors = fs::read_dir("/proc/self/fd")?
        .take(4097)
        .map(|entry| -> Result<i32> {
            let entry = entry?;
            Ok(entry
                .file_name()
                .to_str()
                .context("Invalid descriptor name")?
                .parse()?)
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        descriptors.len() <= 4096,
        "PDF worker descriptor bound exhausted"
    );
    for descriptor in descriptors.into_iter().filter(|&fd| fd > 2) {
        // EBADF is expected for the directory descriptor already closed above.
        unsafe { libc::close(descriptor) };
    }
    Ok(())
}

fn admit_worker_streams() -> Result<()> {
    for descriptor in [libc::STDIN_FILENO, libc::STDOUT_FILENO] {
        // fstat does not open or consume an inherited stream.
        let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(descriptor, status.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("Inspect PDF worker stream");
        }
        let status = unsafe { status.assume_init() };
        ensure!(
            status.st_mode & libc::S_IFMT == libc::S_IFIFO,
            "PDF worker requires parent pipes"
        );
    }
    Ok(())
}

/// Dispatch the exact private invocation before ordinary application startup.
pub fn dispatch() -> Option<Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(ARGUMENT)) {
        return None;
    }
    Some(if args.next().is_some() {
        Err(anyhow::anyhow!("Invalid internal PDF worker invocation"))
    } else {
        run().map_err(|_| anyhow::anyhow!("Internal PDF worker refused"))
    })
}

fn run() -> Result<()> {
    close_extra_descriptors()?;
    admit_worker_streams()?;
    for (resource, soft, hard) in [
        (libc::RLIMIT_AS, 256 * 1024 * 1024, 256 * 1024 * 1024),
        (libc::RLIMIT_CPU, 20, 21),
        (libc::RLIMIT_CORE, 0, 0),
    ] {
        let value = libc::rlimit {
            rlim_cur: soft,
            rlim_max: hard,
        };
        // The private worker lowers only its own limits.
        if unsafe { libc::setrlimit(resource, &value) } != 0 {
            return Err(io::Error::last_os_error()).context("Set PDF worker resource limit");
        }
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let original = read_frame(&mut input, INPUT, PDF_BYTES + HEADER_BYTES + 4)?;
    let (request, bytes): (Input, _) = unpack(&original)?;
    ensure!(
        !request.request.is_nil()
            && !request.document.is_nil()
            && (1..=PDF_BYTES).contains(&request.bytes)
            && request.bytes == bytes.len()
            && request.sha256 == digest(bytes)
            && (1..=256).contains(&request.pages)
            && request.extractor_revision == pdf::EXTRACTOR_REVISION,
        "PDF worker original input changed"
    );
    let normalized = pdf::normalize(request.document, bytes)?;
    ensure!(
        normalized.document.units.len() == request.pages as usize
            && normalized.document.source_sha256 == request.sha256,
        "PDF worker original page count or digest changed"
    );
    drop(original);
    let units = serde_json::to_vec(&normalized.document.units)?;
    let index = Index::build(normalized.document)?;
    let ready = Ready {
        request: request.request,
        key: index.key().clone(),
        pages: request.pages,
    };
    write_frame(&mut output, READY, &packet(&ready, &units, READY_BYTES)?)?;
    drop(units);

    let query_frame = read_frame(&mut input, QUERY, QUERY_BYTES)?;
    let (query, question): (Query, _) = unpack(&query_frame)?;
    let question = std::str::from_utf8(question)?;
    ensure!(
        query.request == ready.request && query.key == ready.key && query_valid(&query, question),
        "PDF worker query does not match admitted input"
    );
    ensure!(
        input.read(&mut [0])? == 0,
        "PDF worker received extra input"
    );
    let result = index.query(
        &query.key,
        question,
        query.max_passages,
        query.max_bytes,
        query.neighbors,
    )?;
    let response = serde_json::to_vec(&Output {
        request: query.request,
        result,
    })?;
    ensure!(
        response.len() <= RESULT_BYTES,
        "PDF worker result exceeds limit"
    );
    write_frame(&mut output, OUTPUT, &response)
}

struct OwnedChild {
    child: Child,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    calls_left: usize,
    polls_left: usize,
}

struct Progress<'a> {
    check: &'a mut dyn FnMut() -> Result<()>,
    next: Instant,
}

impl<'a> Progress<'a> {
    fn new(check: &'a mut dyn FnMut() -> Result<()>) -> Self {
        Self {
            check,
            next: Instant::now(),
        }
    }

    fn poll(&mut self) -> Result<()> {
        if Instant::now() >= self.next {
            self.force()?;
        }
        Ok(())
    }

    fn force(&mut self) -> Result<()> {
        (self.check)()?;
        self.next = Instant::now() + crate::workflow::indicator::CADENCE;
        Ok(())
    }
}

impl OwnedChild {
    fn spawn(executable: &Path) -> Result<Self> {
        let parent = std::process::id();
        let mut command = Command::new(executable);
        command
            .arg(ARGUMENT)
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Async-signal-safe calls only between fork and exec. The parent ID
        // check closes the race where the parent exits before prctl.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let child = command.spawn().context("Start owned PDF worker")?;
        let mut worker = Self {
            child,
            input: None,
            output: None,
            calls_left: IPC_CALLS,
            polls_left: IPC_CALLS * 2,
        };
        worker.input = worker.child.stdin.take();
        worker.output = worker.child.stdout.take();
        for fd in [
            worker
                .input
                .as_ref()
                .context("PDF worker input absent")?
                .as_raw_fd(),
            worker
                .output
                .as_ref()
                .context("PDF worker output absent")?
                .as_raw_fd(),
        ] {
            // The parent owns these ends of the pipes exclusively.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error()).context("Configure PDF worker pipe");
            }
        }
        Ok(worker)
    }

    fn write(
        &mut self,
        kind: u8,
        payload: &[u8],
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let input = self.input.as_mut().context("PDF worker input closed")?;
        let header = frame(kind, payload.len())?;
        for mut bytes in [header.as_slice(), payload] {
            while !bytes.is_empty() {
                poll(
                    input.as_raw_fd(),
                    libc::POLLOUT,
                    &mut self.polls_left,
                    deadline,
                    progress,
                )?;
                charge(&mut self.calls_left)?;
                match input.write(&bytes[..bytes.len().min(CHUNK_BYTES)]) {
                    Ok(0) => bail!("PDF worker pipe stopped writing"),
                    Ok(count) => bytes = &bytes[count..],
                    Err(error) if retry(&error) => {}
                    Err(error) => return Err(error).context("Write PDF worker frame"),
                }
            }
        }
        Ok(())
    }

    fn read_some(
        &mut self,
        bytes: &mut [u8],
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<usize> {
        let output = self.output.as_mut().context("PDF worker output closed")?;
        let limit = bytes.len().min(CHUNK_BYTES);
        loop {
            poll(
                output.as_raw_fd(),
                libc::POLLIN,
                &mut self.polls_left,
                deadline,
                progress,
            )?;
            charge(&mut self.calls_left)?;
            match output.read(&mut bytes[..limit]) {
                Ok(count) => return Ok(count),
                Err(error) if retry(&error) => {}
                Err(error) => return Err(error).context("Read PDF worker frame"),
            }
        }
    }

    fn read_exact(
        &mut self,
        mut bytes: &mut [u8],
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            let count = self.read_some(bytes, deadline, progress)?;
            ensure!(count > 0, "PDF worker frame truncated");
            bytes = &mut bytes[count..];
        }
        Ok(())
    }

    fn read(
        &mut self,
        kind: u8,
        cap: usize,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Vec<u8>> {
        let mut header = [0; 6];
        self.read_exact(&mut header, deadline, progress)?;
        let mut payload = vec![0; length(header, kind, cap)?];
        self.read_exact(&mut payload, deadline, progress)?;
        Ok(payload)
    }

    fn read_ready(
        &mut self,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<(Ready, String)> {
        let mut frame_header = [0; 6];
        self.read_exact(&mut frame_header, deadline, progress)?;
        let size = length(frame_header, READY, READY_BYTES)?;
        ensure!(size > 4, "PDF worker ready header absent");
        let mut header_size = [0; 4];
        self.read_exact(&mut header_size, deadline, progress)?;
        let header_size = u32::from_le_bytes(header_size) as usize;
        ensure!(
            header_size <= HEADER_BYTES && 4 + header_size < size,
            "PDF worker ready payload has no units or invalid header"
        );
        let mut header = vec![0; header_size];
        self.read_exact(&mut header, deadline, progress)?;
        let mut remaining = size - 4 - header_size;
        let mut hasher = Sha256::new();
        let mut chunk = [0; CHUNK_BYTES];
        while remaining > 0 {
            let limit = remaining.min(CHUNK_BYTES);
            let count = self.read_some(&mut chunk[..limit], deadline, progress)?;
            ensure!(count > 0, "PDF worker ready payload truncated");
            hasher.update(&chunk[..count]);
            remaining -= count;
        }
        Ok((
            serde_json::from_slice(&header)?,
            format!("{:x}", hasher.finalize()),
        ))
    }

    fn finish(
        &mut self,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        ensure!(
            self.read_some(&mut [0], deadline, progress)? == 0,
            "PDF worker returned extra bytes"
        );
        loop {
            check(deadline, progress)?;
            if let Some(status) = self.child.try_wait()? {
                ensure!(status.success(), "PDF worker exited unsuccessfully");
                return Ok(());
            }
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.input.take();
        self.output.take();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => worker_identity_lost(error),
            }
        }
        let _ = self.child.kill();
        // Retain custody until the OS reaps the child, even on cancellation
        // or unwind. There is no background waiter or next request.
        loop {
            match self.child.wait() {
                Ok(_) => return,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => worker_identity_lost(error),
            }
        }
    }
}

fn worker_identity_lost(error: io::Error) -> ! {
    log::error!("Unable to reap owned PDF worker: {error}");
    // The child has a parent-death signal. A lost wait identity prevents
    // reusing this runtime or dispatching a signal to a stale PID.
    std::process::abort();
}

fn retry(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

fn charge(remaining: &mut usize) -> Result<()> {
    *remaining = remaining
        .checked_sub(1)
        .context("PDF worker I/O limit exhausted")?;
    Ok(())
}

fn check(deadline: Instant, progress: &mut dyn FnMut() -> Result<()>) -> Result<()> {
    ensure!(Instant::now() < deadline, "PDF worker deadline expired");
    progress()?;
    ensure!(Instant::now() < deadline, "PDF worker deadline expired");
    Ok(())
}

fn poll(
    fd: i32,
    events: i16,
    polls_left: &mut usize,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    loop {
        check(deadline, progress)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = remaining.as_millis().clamp(1, 25) as i32;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        charge(polls_left)?;
        // The caller retains this descriptor throughout the poll.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready > 0 {
            check(deadline, progress)?;
            return Ok(());
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error).context("Poll PDF worker pipe");
            }
        }
    }
}

/// One PDF worker. Its accepted input and index are fixed until the one query.
pub struct PendingIndex {
    worker: OwnedChild,
    request: Uuid,
    key: IndexKey,
    pages: u32,
    expires: Instant,
}

impl PendingIndex {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        executable: &Path,
        document: Uuid,
        pdf_bytes: &[u8],
        original_sha256: String,
        pages: u32,
        ready_by: Instant,
        expires: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            !document.is_nil()
                && (1..=PDF_BYTES).contains(&pdf_bytes.len())
                && (1..=256).contains(&pages)
                && crate::storage::valid_digest(&original_sha256)
                && ready_by <= expires,
            "Invalid admitted PDF worker input"
        );
        let mut progress = Progress::new(progress);
        check(ready_by, &mut || progress.poll())?;
        let request = Uuid::new_v4();
        let original = Input {
            request,
            document,
            bytes: pdf_bytes.len(),
            sha256: original_sha256.clone(),
            pages,
            extractor_revision: pdf::EXTRACTOR_REVISION.into(),
        };
        let mut worker = OwnedChild::spawn(executable)?;
        worker.write(
            INPUT,
            &packet(&original, pdf_bytes, PDF_BYTES + HEADER_BYTES + 4)?,
            ready_by,
            &mut || progress.poll(),
        )?;
        let (ready, normalized_sha256) = worker.read_ready(ready_by, &mut || progress.poll())?;
        let key = IndexKey {
            document,
            format: SourceFormat::Pdf,
            source_sha256: original_sha256,
            extractor_revision: pdf::EXTRACTOR_REVISION.into(),
            normalized_sha256,
        };
        ensure!(
            ready.request == request && ready.pages == pages && ready.key == key,
            "PDF worker returned a different index identity"
        );
        check(ready_by, &mut || progress.force())?;
        Ok(Self {
            worker,
            request,
            key,
            pages,
            expires,
        })
    }

    pub fn key(&self) -> &IndexKey {
        &self.key
    }

    #[allow(clippy::too_many_arguments)]
    pub fn query(
        mut self,
        expected: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Retrieved> {
        let deadline = deadline.min(self.expires);
        let mut progress = Progress::new(progress);
        ensure!(
            expected == &self.key && question.len() <= 2048,
            "Invalid admitted PDF query"
        );
        let query = Query {
            request: self.request,
            key: self.key.clone(),
            max_passages,
            max_bytes,
            neighbors,
        };
        ensure!(query_valid(&query, question), "Invalid admitted PDF query");
        let payload = packet(&query, question.as_bytes(), QUERY_BYTES)?;
        self.worker
            .write(QUERY, &payload, deadline, &mut || progress.poll())?;
        self.worker.input.take();
        let bytes = self.worker.read(
            OUTPUT,
            RESULT_BYTES.min(max_bytes + HEADER_BYTES),
            deadline,
            &mut || progress.poll(),
        )?;
        let output: Output = serde_json::from_slice(&bytes)?;
        ensure!(
            output.request == self.request
                && output.result.source == self.key
                && output.result.passages.len() <= max_passages
                && output.result.passages.iter().all(|passage| {
                    passage.unit < self.pages as usize
                        && passage.page == Some(passage.unit as u32 + 1)
                        && passage.section.is_none()
                })
                && serde_json::to_vec(&output.result)?.len() <= max_bytes,
            "PDF worker returned invalid passage provenance"
        );
        self.worker.finish(deadline, &mut || progress.poll())?;
        check(deadline, &mut || progress.force())?;
        Ok(output.result)
    }
}
