#![cfg(target_os = "linux")]

use lopdf::{dictionary, Document, Object, Stream};
use nix::libc;
use remarkable_reader_buddy::{
    retrieval::{pdf, pdf_worker, Index, Retrieved},
    storage::{digest, Uuid},
};
use serde_json::json;
use std::{
    fs,
    io::{self, Read, Write},
    os::fd::{AsRawFd, RawFd},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(feature = "development-resident-bootstrap")]
use {
    image::{DynamicImage, ImageFormat, RgbaImage},
    remarkable_open_sdk::{
        native_adapter::{HistoricalCapture, ReaderFacts},
        PageKey, Uuid as SdkUuid,
    },
    remarkable_reader_buddy::{
        retrieval::pdf_worker::PendingIndex, workflow::development_original_pdf,
    },
    sha2::{Digest, Sha256},
    std::{
        io::Cursor,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
    },
};

const DOCUMENT: u128 = 1;
const REQUEST: u128 = 2;
const PAGES: u32 = 2;
const FIXTURE_WINDOW: Duration = Duration::from_secs(15);

fn fixture_pdf_bytes() -> Vec<u8> {
    let mut pdf = Document::with_version("1.5");
    let root = pdf.new_object_id();
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1",
        "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding"
    });
    let pages = [
        b"BT /F1 12 Tf (different first page) Tj ET".as_slice(),
        b"BT /F1 12 Tf (alpha acceleration) Tj ET",
    ]
    .into_iter()
    .map(|content| {
        let stream = pdf.add_object(Stream::new(dictionary! {}, content.to_vec()));
        Object::Reference(pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => root, "Contents" => stream,
            "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()]
        }))
    })
    .collect::<Vec<_>>();
    pdf.objects.insert(
        root,
        dictionary! {
            "Type" => "Pages", "Count" => i64::from(PAGES),
            "Kids" => pages,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } }
        }
        .into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    bytes
}

struct Worker {
    child: Child,
    deadline: Instant,
}

fn nonblocking(descriptor: RawFd) {
    // The fixture owns these pipe ends.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    assert!(flags >= 0, "get pipe flags failed");
    assert_eq!(
        unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0,
        "set nonblocking failed"
    );
}

fn bounded<T>(deadline: Instant, mut attempt: impl FnMut() -> io::Result<T>) -> T {
    loop {
        assert!(
            Instant::now() < deadline,
            "PDF worker fixture deadline expired"
        );
        match attempt() {
            Ok(value) => {
                assert!(
                    Instant::now() < deadline,
                    "PDF worker fixture deadline expired"
                );
                return value;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(
                    Duration::from_millis(2)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => panic!("PDF worker fixture I/O: {error}"),
        }
    }
}

impl Worker {
    fn start(deadline: Instant) -> Self {
        Self::spawn(deadline, None)
    }

    fn spawn(deadline: Instant, extra_argument: Option<&str>) -> Self {
        assert!(
            Instant::now() < deadline,
            "PDF worker fixture deadline expired"
        );
        let child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .arg(pdf_worker::ARGUMENT)
            .args(extra_argument)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if extra_argument.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .spawn()
            .unwrap();
        let worker = Self { child, deadline };
        nonblocking(worker.child.stdin.as_ref().unwrap().as_raw_fd());
        nonblocking(worker.child.stdout.as_ref().unwrap().as_raw_fd());
        if let Some(stderr) = &worker.child.stderr {
            nonblocking(stderr.as_raw_fd());
        }
        worker
    }

    fn write(&mut self, mut bytes: &[u8]) {
        let input = self.child.stdin.as_mut().unwrap();
        while !bytes.is_empty() {
            let count = bounded(self.deadline, || input.write(bytes));
            assert!(count > 0, "PDF worker input stopped");
            bytes = &bytes[count..];
        }
    }

    fn read_exact(&mut self, mut bytes: &mut [u8]) {
        let output = self.child.stdout.as_mut().unwrap();
        while !bytes.is_empty() {
            let count = bounded(self.deadline, || output.read(bytes));
            assert!(count > 0, "PDF worker output truncated");
            bytes = &mut bytes[count..];
        }
    }

    fn send(&mut self, kind: u8, payload: &[u8]) {
        let mut header = [1, kind, 0, 0, 0, 0];
        header[2..].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        self.write(&header);
        self.write(payload);
    }

    fn receive(&mut self, kind: u8, cap: usize) -> Vec<u8> {
        let mut header = [0; 6];
        self.read_exact(&mut header);
        assert_eq!(header[..2], [1, kind]);
        let length = u32::from_le_bytes(header[2..].try_into().unwrap()) as usize;
        assert!(length > 0 && length <= cap);
        let mut payload = vec![0; length];
        self.read_exact(&mut payload);
        payload
    }

    fn close_input(&mut self) {
        self.child.stdin.take();
    }

    fn finish(&mut self, success: bool) {
        self.close_input();
        let output = self.child.stdout.as_mut().unwrap();
        assert_eq!(bounded(self.deadline, || output.read(&mut [0])), 0);
        let status = bounded(self.deadline, || {
            self.child
                .try_wait()?
                .ok_or(io::ErrorKind::WouldBlock.into())
        });
        assert_eq!(status.success(), success);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.close_input();
        self.child.stdout.take();
        self.child.stderr.take();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    let _ = self.child.kill();
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
        while let Err(error) = self.child.wait() {
            if error.kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
    }
}

fn input(bytes: &[u8], sha256: &str) -> Vec<u8> {
    let header = serde_json::to_vec(&json!({
        "request": Uuid::from_u128(REQUEST),
        "document": Uuid::from_u128(DOCUMENT),
        "bytes": bytes.len(),
        "sha256": sha256,
        "pages": PAGES,
        "extractor_revision": pdf::EXTRACTOR_REVISION,
    }))
    .unwrap();
    packet(&header, bytes)
}

fn packet(header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut packet = (header.len() as u32).to_le_bytes().to_vec();
    packet.extend_from_slice(header);
    packet.extend_from_slice(body);
    packet
}

#[test]
fn selected_pdf_worker_binary_normalizes_queries_and_reaches_exact_eof() {
    let deadline = Instant::now() + FIXTURE_WINDOW;
    let bytes = fixture_pdf_bytes();
    let document = Uuid::from_u128(DOCUMENT);
    let normalized = pdf::normalize(document, bytes.as_slice()).unwrap();
    let expected = Index::build(normalized.document.clone()).unwrap();
    let mut worker = Worker::start(deadline);
    worker.send(1, &input(&bytes, &digest(&bytes)));
    let ready = worker.receive(2, 9 * 1024 * 1024);
    let limits = fs::read_to_string(format!("/proc/{}/limits", worker.child.id())).unwrap();
    for (name, soft, hard) in [
        ("Max cpu time", "20", "21"),
        ("Max address space", "268435456", "268435456"),
        ("Max core file size", "0", "0"),
    ] {
        let line = limits
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap();
        let mut limits = line.split_whitespace();
        assert_eq!(limits.next(), Some(soft));
        assert_eq!(limits.next(), Some(hard));
    }
    let header_len = u32::from_le_bytes(ready[..4].try_into().unwrap()) as usize;
    let header: serde_json::Value = serde_json::from_slice(&ready[4..4 + header_len]).unwrap();
    assert_eq!(header["request"], json!(Uuid::from_u128(REQUEST)));
    assert_eq!(header["pages"], PAGES);
    assert_eq!(header["key"], json!(expected.key()));
    assert_eq!(
        &ready[4 + header_len..],
        serde_json::to_vec(&normalized.document.units).unwrap()
    );
    worker.send(
        3,
        &packet(
            &serde_json::to_vec(&json!({
                "request": Uuid::from_u128(REQUEST), "key": expected.key(),
                "max_passages": 2, "max_bytes": 4096, "neighbors": false,
            }))
            .unwrap(),
            b"acceleration",
        ),
    );
    worker.close_input();
    let output = worker.receive(4, 17 * 1024);
    let output: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(output["request"], json!(Uuid::from_u128(REQUEST)));
    let retrieved: Retrieved = serde_json::from_value(output["result"].clone()).unwrap();
    assert_eq!(&retrieved.source, expected.key());
    assert_eq!(retrieved.passages.len(), 1);
    assert_eq!(retrieved.passages[0].page, Some(2));
    assert!(retrieved.passages[0].text.contains("alpha acceleration"));
    worker.finish(true);
}

#[test]
fn selected_pdf_worker_binary_refuses_wrong_digest_and_extra_query_bytes() {
    let deadline = Instant::now() + FIXTURE_WINDOW;
    let bytes = fixture_pdf_bytes();
    let mut worker = Worker::start(deadline);
    worker.send(1, &input(&bytes, &"0".repeat(64)));
    worker.finish(false);

    let mut worker = Worker::start(deadline);
    worker.send(1, &input(&bytes, &digest(&bytes)));
    let ready = worker.receive(2, 9 * 1024 * 1024);
    let header_len = u32::from_le_bytes(ready[..4].try_into().unwrap()) as usize;
    let header: serde_json::Value = serde_json::from_slice(&ready[4..4 + header_len]).unwrap();
    worker.send(
        3,
        &packet(
            &serde_json::to_vec(&json!({
                "request": Uuid::from_u128(REQUEST), "key": header["key"],
                "max_passages": 2, "max_bytes": 4096, "neighbors": false,
            }))
            .unwrap(),
            b"acceleration",
        ),
    );
    worker.write(&[0]);
    worker.finish(false);
}

#[test]
fn selected_pdf_worker_binary_refuses_extra_arguments_and_short_input() {
    let deadline = Instant::now() + FIXTURE_WINDOW;
    let mut worker = Worker::spawn(deadline, Some("--not-worker-input"));
    worker.finish(false);
    let mut stderr = [0; 256];
    let mut used = 0;
    let stream = worker.child.stderr.as_mut().unwrap();
    loop {
        assert!(
            used < stderr.len(),
            "unexpected PDF worker diagnostic length"
        );
        let count = bounded(deadline, || stream.read(&mut stderr[used..]));
        if count == 0 {
            break;
        }
        used += count;
    }
    assert!(
        String::from_utf8_lossy(&stderr[..used]).contains("Invalid internal PDF worker invocation")
    );

    let mut worker = Worker::start(deadline);
    worker.write(&[1, 1, 1]);
    worker.finish(false);
}

#[cfg(feature = "development-resident-bootstrap")]
struct OwnedRoot(PathBuf);

#[cfg(feature = "development-resident-bootstrap")]
impl Drop for OwnedRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(feature = "development-resident-bootstrap")]
struct OriginalPdfFixture {
    root: OwnedRoot,
    bytes: Vec<u8>,
    document: Uuid,
    facts: ReaderFacts,
    capture: HistoricalCapture,
}

#[cfg(feature = "development-resident-bootstrap")]
fn fixture_capture(facts: &ReaderFacts) -> HistoricalCapture {
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(2, 2))
        .write_to(&mut png, ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let hash = Sha256::digest(&png).into();
    HistoricalCapture::new(facts.clone(), facts.clone(), [2, 2], 1.0, png, hash).unwrap()
}

#[cfg(feature = "development-resident-bootstrap")]
impl OriginalPdfFixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("buddy-original-pdf-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let root = OwnedRoot(path);
        let bytes = fixture_pdf_bytes();
        let document = Uuid::from_u128(DOCUMENT);
        let id = |value| SdkUuid::parse(&Uuid::from_u128(value).to_string()).unwrap();
        let facts = ReaderFacts::new(
            PageKey {
                document: id(DOCUMENT),
                page: id(4),
            },
            vec![id(3), id(4)],
        )
        .unwrap();
        fs::write(
            root.0.join(format!("{document}.content")),
            serde_json::to_vec(&json!({
                "fileType": "pdf",
                "cPages": { "original": { "value": PAGES }, "pages": [
                    { "id": Uuid::from_u128(3), "idx": { "value": "a" }, "redir": { "value": 0 } },
                    { "id": Uuid::from_u128(4), "idx": { "value": "b" }, "redir": { "value": 1 } },
                ] },
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(root.0.join(format!("{document}.pdf")), &bytes).unwrap();
        let capture = fixture_capture(&facts);
        Self {
            root,
            bytes,
            document,
            facts,
            capture,
        }
    }

    fn index(&self, executable: &Path, deadline: Instant) -> anyhow::Result<PendingIndex> {
        Ok(development_original_pdf(
            &self.root.0,
            executable,
            self.document,
            &self.facts,
            &self.capture,
            deadline,
            &mut || Ok(()),
        )?
        .index)
    }
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
fn actual_original_pdf_producer_joins_binary_index_and_keeps_source_pin() {
    let OriginalPdfFixture {
        root,
        bytes,
        document,
        facts,
        capture,
    } = OriginalPdfFixture::new();
    let executable = Path::new(env!("CARGO_BIN_EXE_reader-buddy"));
    let deadline = Instant::now() + FIXTURE_WINDOW;
    let foreign = ReaderFacts::new(
        PageKey {
            document: SdkUuid::parse(&Uuid::from_u128(99).to_string()).unwrap(),
            page: facts.page().page,
        },
        facts.order().to_vec(),
    )
    .unwrap();
    let foreign_capture = fixture_capture(&foreign);
    let refused = development_original_pdf(
        &root.0,
        executable,
        document,
        &facts,
        &foreign_capture,
        deadline,
        &mut || Ok(()),
    );
    assert_eq!(
        refused.err().unwrap().to_string(),
        "Original PDF capture correspondence changed"
    );
    let mut index = development_original_pdf(
        &root.0,
        executable,
        document,
        &facts,
        &capture,
        deadline,
        &mut || Ok(()),
    )
    .unwrap();
    let expected =
        Index::build(pdf::normalize(document, bytes.as_slice()).unwrap().document).unwrap();
    assert_eq!(index.pin.original_page(), 2);
    assert_eq!(index.index.key(), expected.key());
    let passages = index
        .index
        .query(
            expected.key(),
            "acceleration",
            2,
            4096,
            false,
            deadline,
            &mut || Ok(()),
        )
        .unwrap()
        .passages;
    assert_eq!(passages.len(), 1);
    assert_eq!(passages[0].page, Some(2));
    assert!(passages[0].text.contains("alpha acceleration"));
    index.pin.check(deadline).unwrap();

    let index = development_original_pdf(
        &root.0,
        executable,
        document,
        &facts,
        &capture,
        deadline,
        &mut || Ok(()),
    )
    .unwrap();
    let found = index
        .index
        .query(
            expected.key(),
            "absentword",
            2,
            4096,
            false,
            deadline,
            &mut || Ok(()),
        )
        .unwrap();
    assert!(found.passages.is_empty());

    let index = development_original_pdf(
        &root.0,
        executable,
        document,
        &facts,
        &capture,
        deadline,
        &mut || Ok(()),
    )
    .unwrap();
    let mut called = false;
    let result = index.index.query(
        expected.key(),
        "acceleration",
        2,
        4096,
        false,
        deadline,
        &mut || {
            called = true;
            anyhow::bail!("fixture canceled")
        },
    );
    assert!(called);
    assert!(result.is_err());
}

#[cfg(feature = "development-resident-bootstrap")]
fn owned_worker_pid() -> libc::pid_t {
    // The producer spawns on this test thread. Other fixtures can run in parallel.
    let children = fs::read_to_string("/proc/thread-self/children").unwrap();
    let mut children = children.split_whitespace();
    let pid = children.next().expect("owned worker").parse().unwrap();
    assert!(children.next().is_none(), "unexpected additional worker");
    pid
}

#[cfg(feature = "development-resident-bootstrap")]
fn observe_worker(pid: libc::pid_t, events: libc::c_int) -> io::Result<libc::pid_t> {
    // WNOWAIT observes stopped/exited children without reaping them.
    let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid.try_into().unwrap(),
            status.as_mut_ptr(),
            events | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // With WNOHANG, a zero PID means that no matching event is ready.
    Ok(unsafe { status.assume_init().si_pid() })
}

#[cfg(feature = "development-resident-bootstrap")]
fn assert_worker_reaped(pid: libc::pid_t) {
    assert_eq!(
        observe_worker(pid, libc::WEXITED)
            .expect_err("worker still belongs to the test process")
            .raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
fn actual_original_pdf_cancellation_reaps_stalled_query_before_next_request() {
    let fixture = OriginalPdfFixture::new();
    let executable = Path::new(env!("CARGO_BIN_EXE_reader-buddy"));
    let deadline = Instant::now() + FIXTURE_WINDOW;
    let index = fixture.index(executable, deadline).unwrap();
    let key = index.key().clone();
    let pid = owned_worker_pid();
    // Freeze the real worker after Ready. While stopped it cannot consume the
    // one query or exit on EOF. The parent still writes into the actual pipe.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    bounded(deadline, || match observe_worker(pid, libc::WSTOPPED)? {
        0 => Err(io::ErrorKind::WouldBlock.into()),
        observed => {
            assert_eq!(observed, pid);
            Ok(())
        }
    });
    let input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(format!("/proc/{pid}/fd/0"))
        .unwrap();
    let result = index.query(&key, "acceleration", 2, 4096, false, deadline, &mut || {
        let mut descriptor = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut descriptor, 1, 0) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let submitted = libc::POLLIN | libc::POLLHUP;
        if descriptor.revents & submitted == submitted {
            anyhow::bail!("fixture canceled after query submission and input EOF");
        }
        Ok(())
    });
    assert_eq!(
        result.unwrap_err().to_string(),
        "fixture canceled after query submission and input EOF"
    );
    assert_worker_reaped(pid);

    let deadline = Instant::now() + FIXTURE_WINDOW;
    let next = fixture.index(executable, deadline).unwrap();
    let pid = owned_worker_pid();
    let result = next
        .query(&key, "acceleration", 2, 4096, false, deadline, &mut || {
            Ok(())
        })
        .unwrap();
    assert_eq!(result.source, key);
    assert_eq!(result.passages.len(), 1);
    assert_eq!(result.passages[0].page, Some(2));
    assert_worker_reaped(pid);
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
fn actual_original_pdf_parent_refuses_invalid_worker_output_and_reaps() {
    let fixture = OriginalPdfFixture::new();
    let units = pdf::normalize(fixture.document, fixture.bytes.as_slice())
        .unwrap()
        .document
        .units;
    let units = serde_json::to_string(&units).unwrap();
    for (fault, expected) in [
        ("key", "PDF worker returned invalid passage provenance"),
        ("frame", "Unexpected PDF worker frame"),
        ("truncated", "PDF worker frame truncated"),
        ("abrupt", "PDF worker frame truncated"),
        ("extra", "PDF worker returned extra bytes"),
        ("exit", "PDF worker exited unsuccessfully"),
    ] {
        let executable = fixture.root.0.join(format!("worker-{fault}"));
        let script = format!(
            "#!/usr/bin/python3\nFAULT = {fault:?}\nUNITS = {units:?}\n{}",
            include_str!("fixtures/selected_reader_pdf_worker_fault.py")
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let deadline = Instant::now() + FIXTURE_WINDOW;
        let index = fixture.index(&executable, deadline).unwrap();
        let key = index.key().clone();
        let pid = owned_worker_pid();
        let result = index.query(&key, "acceleration", 2, 4096, false, deadline, &mut || {
            Ok(())
        });
        assert_eq!(result.unwrap_err().to_string(), expected, "{fault}");
        assert_worker_reaped(pid);
    }
}
