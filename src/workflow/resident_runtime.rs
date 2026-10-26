//! Explicit Linux child ownership for the resident SDK. Not installed activation.
//! The caller still owns approved profile selection, stock-service takeover and
//! independent stock recovery. Protocol Ready confers no Reader/write capability.
use super::resident_client::{self, ConnectionFailure, ResidentConnection};
use crate::storage::Store;
use anyhow::{ensure, Context, Result};
use nix::libc;
use remarkable_open_sdk::{
    native_adapter::{ActivationAdmission, ConnectionIdentity, LostRequest, RuntimeIdentity},
    OperationId, Uuid,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(10);
const CLEANUP: Duration = Duration::from_secs(2);

/// Expected bytes from independently reviewed delivery, never peer assertions.
pub struct Artifact {
    pub path: PathBuf,
    pub sha256: [u8; 32],
}
pub struct Launch {
    pub profile: Artifact,
    pub executable: Artifact,
    pub adapter: Artifact,
    /// Fresh absolute path below an existing caller-owned mode0700 directory.
    pub directory: PathBuf,
    /// Explicit approved ordinary values; all ambient environment is discarded.
    pub environment: BTreeMap<String, String>,
    /// Approved argv[1..], preserving exact boundaries without a shell.
    pub arguments: Vec<String>,
    pub operation_budget_ms: u32,
}

#[derive(Debug)]
pub struct Shutdown {
    pub pid: u32,
    pub start_ticks: Option<u64>,
    pub lost: LostRequest,
    pub reaped: bool,
    pub error: Option<String>,
}
#[derive(Debug)]
pub struct StartupFailure {
    pub directory: PathBuf,
    pub error: anyhow::Error,
    pub shutdown: Option<Box<Shutdown>>,
}
impl std::fmt::Display for StartupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "resident startup failed: {:#}; evidence: {}",
            self.error,
            self.directory.display()
        )
    }
}
impl std::error::Error for StartupFailure {}

struct PinnedFile {
    file: File,
    path: PathBuf,
    metadata: Metadata,
}
impl PinnedFile {
    fn current(&self) -> Result<()> {
        ensure!(
            same_file(&self.metadata, &self.file.metadata()?)
                && same_file(&self.metadata, &fs::symlink_metadata(&self.path)?),
            "resident file changed"
        );
        Ok(())
    }
}
struct Prepared {
    directory: PinnedFile,
    artifacts: Vec<PinnedFile>,
    admission: PinnedFile,
    admission_bytes: [u8; 168],
    identity: RuntimeIdentity,
    uid: u32,
    command: Command,
}

/// Retain for the entire session, not one conversation. Dropping stops only this
/// direct child; it does not restore stock or settle any pending native effect.
pub struct ResidentRuntime {
    child: Option<Child>,
    connection: Option<ResidentConnection>,
    prepared: Prepared,
    store: Arc<Store>,
    retired_loss: LostRequest,
}
impl ResidentRuntime {
    pub fn launch(
        store: Arc<Store>,
        input: Launch,
        deadline: Instant,
    ) -> std::result::Result<Self, StartupFailure> {
        Self::launch_inner(store, input, deadline, |_| {})
    }
    fn launch_inner(
        store: Arc<Store>,
        input: Launch,
        deadline: Instant,
        decorate: impl FnOnce(&mut Command),
    ) -> std::result::Result<Self, StartupFailure> {
        Self::launch_checked(store, input, deadline, decorate, |_| Ok(()))
    }
    /// Installed admission is repeated after potentially lengthy artifact
    /// preparation, before any child exists, then through socket/Ready waits.
    /// It cannot replace the fixed deadline or disable owned-child cleanup.
    pub(super) fn launch_admitted(
        store: Arc<Store>,
        input: Launch,
        deadline: Instant,
        admit: impl FnMut(Instant) -> Result<()>,
    ) -> std::result::Result<Self, StartupFailure> {
        Self::launch_checked(store, input, deadline, |_| {}, admit)
    }
    fn launch_checked(
        store: Arc<Store>,
        input: Launch,
        deadline: Instant,
        decorate: impl FnOnce(&mut Command),
        mut admit: impl FnMut(Instant) -> Result<()>,
    ) -> std::result::Result<Self, StartupFailure> {
        let path = input.directory.clone();
        let prepare = || -> Result<Prepared> {
            // Invalid retained effects must refuse before starting another process.
            resident_client::attempts(&store)?;
            Prepared::new(input, deadline)
        };
        let mut prepared = prepare().map_err(|error| StartupFailure {
            directory: path.clone(),
            error,
            shutdown: None,
        })?;
        decorate(&mut prepared.command); // Only tests customize the private fixture launch.
        let spawned = (|| -> Result<Child> {
            prepared.current(false)?;
            before(deadline)?;
            admit(deadline)?;
            prepared.current(false)?;
            before(deadline)?;
            prepared.command.spawn().context("spawn resident child")
        })();
        let child = spawned.map_err(|error| StartupFailure {
            directory: path.clone(),
            error,
            shutdown: None,
        })?;
        let mut runtime = Self {
            child: Some(child),
            connection: None,
            prepared,
            store: store.clone(),
            retired_loss: LostRequest::NoPendingMutation,
        };
        match runtime.connect(store, deadline, &mut admit) {
            Ok(()) => Ok(runtime),
            Err(error) => {
                let shutdown = runtime.stop(Instant::now() + CLEANUP);
                Err(StartupFailure {
                    directory: path,
                    error,
                    shutdown: Some(Box::new(shutdown)),
                })
            }
        }
    }
    fn connect(
        &mut self,
        store: Arc<Store>,
        deadline: Instant,
        admit: &mut dyn FnMut(Instant) -> Result<()>,
    ) -> Result<()> {
        let pid = self.child.as_ref().expect("owned child").id();
        self.prepared.identity.pid = pid;
        self.prepared.identity.start_ticks = process_start(pid)?;
        loop {
            before(deadline)?;
            admit(deadline)?;
            before(deadline)?;
            ensure!(
                self.child.as_mut().unwrap().try_wait()?.is_none(),
                "resident exited before Ready"
            );
            self.prepared.current(false)?;
            match fs::symlink_metadata(self.prepared.directory.path.join("sdk.sock")) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
                }
                Err(error) => return Err(error.into()),
            }
        }
        self.check_child()?;
        let scope = ConnectionIdentity {
            runtime: self.prepared.identity.clone(),
            connection: nonce(),
            server_nonce: None,
        };
        let connection = ResidentConnection::connect_with_progress(
            store,
            self.prepared.directory.file.try_clone()?,
            self.prepared.uid,
            scope,
            OperationId(nonce()),
            deadline,
            &mut || {
                admit(deadline)?;
                self.prepared.current(false)?;
                self.check_child()?;
                before(deadline)
            },
        )?;
        self.connection = Some(connection);
        admit(deadline)?;
        self.prepared.current(true)?;
        self.check_child()?;
        before(deadline)
    }
    fn check_child(&mut self) -> Result<()> {
        let child = self.child.as_mut().context("resident already shut down")?;
        ensure!(child.try_wait()?.is_none(), "resident child exited");
        let actual = fs::metadata(format!("/proc/{}/exe", child.id()))?;
        let expected = &self.prepared.artifacts[1].metadata;
        ensure!(
            same_inode(expected, &actual)
                && process_start(child.id())? == self.prepared.identity.start_ticks,
            "resident child executable/incarnation changed"
        );
        Ok(())
    }
    pub fn connection(&mut self) -> Result<&mut ResidentConnection> {
        self.check_child()?;
        self.prepared.current(true)?;
        self.connection
            .as_mut()
            .context("resident connection absent")
    }
    pub fn identity(&self) -> &RuntimeIdentity {
        &self.prepared.identity
    }
    /// Close only the client socket, retaining original effect uncertainty and
    /// child ownership. This is not native cancellation or permission to replay.
    pub fn retire_connection(&mut self) -> LostRequest {
        if let Some(mut connection) = self.connection.take() {
            let lost = connection.close();
            if self.retired_loss == LostRequest::NoPendingMutation {
                self.retired_loss = lost;
            }
        }
        self.retired_loss.clone()
    }
    /// Explicitly reconnect to this same verified child. The installed owner
    /// must supply lease/group/health admission on each callback. No retry or
    /// child restart follows failure, and unresolved effects stay read-only.
    pub fn reconnect(
        &mut self,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.retire_connection();
        before(deadline)?;
        ensure!(
            deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(30),
            "resident reconnect bound exceeds 30 seconds"
        );
        self.prepared.current(true)?;
        self.check_child()?;
        let scope = ConnectionIdentity {
            runtime: self.prepared.identity.clone(),
            connection: nonce(),
            server_nonce: None,
        };
        let result = ResidentConnection::connect_with_progress(
            self.store.clone(),
            self.prepared.directory.file.try_clone()?,
            self.prepared.uid,
            scope,
            OperationId(nonce()),
            deadline,
            &mut || {
                progress()?;
                self.prepared.current(true)?;
                self.check_child()?;
                before(deadline)
            },
        );
        let mut connection =
            result.map_err(|error| match error.downcast::<ConnectionFailure>() {
                Ok(mut failure) => {
                    if self.retired_loss != LostRequest::NoPendingMutation {
                        failure.lost = self.retired_loss.clone();
                    }
                    anyhow::Error::new(failure)
                }
                Err(error) => error,
            })?;
        progress()?;
        self.prepared.current(true)?;
        self.check_child()?;
        before(deadline)?;
        connection.retain_loss(self.retired_loss.clone());
        self.connection = Some(connection);
        Ok(())
    }
    pub fn evidence_directory(&self) -> &Path {
        &self.prepared.directory.path
    }
    pub fn shutdown(mut self, deadline: Instant) -> Shutdown {
        self.stop(deadline)
    }
    fn stop(&mut self, deadline: Instant) -> Shutdown {
        let deadline = deadline.min(Instant::now() + CLEANUP);
        let lost = self.retire_connection();
        let mut report = Shutdown {
            pid: self.prepared.identity.pid,
            start_ticks: (self.prepared.identity.start_ticks != 0)
                .then_some(self.prepared.identity.start_ticks),
            lost,
            reaped: false,
            error: None,
        };
        if let Some(mut child) = self.child.take() {
            report.pid = child.id();
            let stopped = (|| -> Result<bool> {
                if child.try_wait()?.is_some() {
                    return Ok(true);
                }
                child.kill().context("stop owned resident child")?;
                loop {
                    if child.try_wait()?.is_some() {
                        return Ok(true);
                    }
                    if Instant::now() >= deadline {
                        return Ok(false);
                    }
                    thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
                }
            })();
            match stopped {
                Ok(reaped) => report.reaped = reaped,
                Err(error) => report.error = Some(format!("{error:#}")),
            }
        }
        if !report.reaped && report.error.is_none() {
            report.error = Some("owned child was not reaped before deadline".into());
        }
        // Process diagnostics only; native request evidence remains in Store.
        let record = format!("{report:?}\n");
        if let Err(error) = (|| -> Result<()> {
            // Identity loss may mean the original name now denotes another root.
            // Retain diagnostics through our owned directory descriptor instead.
            let path = PathBuf::from(format!(
                "/proc/self/fd/{}/shutdown.txt",
                self.prepared.directory.file.as_raw_fd()
            ));
            let mut file = create_private(&path)?;
            file.write_all(record.as_bytes())?;
            file.sync_all()?;
            self.prepared.directory.file.sync_all()?;
            Ok(())
        })() {
            log::warn!("Resident shutdown evidence retention failed: {error:#}");
            report.error = Some(match report.error.take() {
                Some(prior) => format!("{prior}; shutdown evidence retention failed: {error:#}"),
                None => format!("shutdown evidence retention failed: {error:#}"),
            });
        }
        report
    }
}
impl Drop for ResidentRuntime {
    fn drop(&mut self) {
        if self.child.is_some() {
            let report = self.stop(Instant::now() + CLEANUP);
            if !report.reaped {
                log::error!("Resident recovery required: {report:?}");
            }
        }
    }
}

impl Prepared {
    fn new(input: Launch, deadline: Instant) -> Result<Self> {
        before(deadline)?;
        ensure!(
            deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(60),
            "resident startup bound exceeds 60 seconds"
        );
        let environment = environment(&input.environment)?;
        ensure!(
            input.arguments.len() <= 16,
            "resident argument count exceeds bound"
        );
        let argument_bytes = input.arguments.iter().try_fold(0usize, |total, argument| {
            ensure!(!argument.contains('\0'), "resident argument contains NUL");
            total
                .checked_add(argument.len())
                .context("resident argument byte count overflow")
        })?;
        ensure!(
            argument_bytes <= 16 * 1024,
            "resident arguments exceed bound"
        );
        ensure!(
            (1..=30_000).contains(&input.operation_budget_ms),
            "resident operation bound invalid"
        );
        ensure!(
            input.directory.is_absolute()
                && input.directory.as_os_str().as_encoded_bytes().len() + 9 < 108,
            "resident directory must be absolute and fit Unix socket path"
        );
        let directory_bytes = input.directory.as_os_str().as_encoded_bytes();
        ensure!(
            !directory_bytes.ends_with(b"/")
                && !directory_bytes
                    .iter()
                    .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b':' | b'$')),
            "resident directory cannot contain preload separators, loader tokens or a trailing slash"
        );
        ensure!(
            input.directory.components().all(|part| matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
            "resident directory contains traversal"
        );
        let parent = input.directory.parent().context("resident parent absent")?;
        let uid = unsafe { libc::geteuid() };
        let parent_metadata = fs::symlink_metadata(parent)?;
        ensure!(
            parent_metadata.is_dir()
                && parent_metadata.uid() == uid
                && parent_metadata.mode() & 0o7777 == 0o700
                && fs::canonicalize(parent)? == parent,
            "resident parent is not owned canonical mode0700 directory"
        );
        DirBuilder::new().mode(0o700).create(&input.directory)?;
        let directory = pin(input.directory.clone(), true)?;
        ensure!(
            directory.metadata.uid() == uid && directory.metadata.mode() & 0o7777 == 0o700,
            "resident directory permissions changed"
        );
        let artifacts = vec![
            copy_artifact(
                &input.profile,
                &input.directory.join("profile.json"),
                64 * 1024,
                0o400,
                uid,
                deadline,
            )?,
            copy_artifact(
                &input.executable,
                &input.directory.join("reader"),
                64 * 1024 * 1024,
                0o500,
                uid,
                deadline,
            )?,
            copy_artifact(
                &input.adapter,
                &input.directory.join("adapter.so"),
                32 * 1024 * 1024,
                0o400,
                uid,
                deadline,
            )?,
        ];
        let boot = Uuid::parse(read_text("/proc/sys/kernel/random/boot_id")?.trim())
            .context("invalid boot UUID")?;
        before(deadline)?;
        let now = boot_ms()?;
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(60_000) as u64;
        let activation = nonce();
        let admission_bytes = ActivationAdmission {
            profile_sha256: input.profile.sha256,
            executable_sha256: input.executable.sha256,
            adapter_sha256: input.adapter.sha256,
            boot,
            activation,
            supervisor_uid: uid,
            supervisor_pid: std::process::id(),
            supervisor_start_ticks: process_start(std::process::id())?,
            expires_boot_ms: now
                .checked_add(remaining)
                .context("boot deadline overflow")?,
            operation_budget_ms: input.operation_budget_ms,
        }
        .encode(now)
        .map_err(|error| anyhow::anyhow!("admission refused: {error:?}"))?;
        let mut admission = create_private(&input.directory.join("admission.bin"))?;
        admission.write_all(&admission_bytes)?;
        admission.sync_all()?;
        let admission = PinnedFile {
            metadata: admission.metadata()?,
            file: admission,
            path: input.directory.join("admission.bin"),
        };
        let stdout = create_private(&input.directory.join("stdout.log"))?;
        let stderr = create_private(&input.directory.join("stderr.log"))?;
        directory.file.sync_all()?;
        File::open(parent)?.sync_all()?;
        let mut command = Command::new(&artifacts[1].path);
        command
            .args(&input.arguments)
            .current_dir("/")
            .env_clear()
            .envs(environment)
            .env("LD_PRELOAD", &artifacts[2].path)
            .env("LD_BIND_NOW", "1")
            .env("RSDK_ACTIVATION_DIRECTORY", &input.directory)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        let identity = RuntimeIdentity {
            profile_sha256: input.profile.sha256,
            executable_sha256: input.executable.sha256,
            adapter_sha256: input.adapter.sha256,
            boot,
            pid: 0,
            start_ticks: 0,
            activation,
        };
        Ok(Self {
            directory,
            artifacts,
            admission,
            admission_bytes,
            identity,
            uid,
            command,
        })
    }
    fn current(&mut self, consumed: bool) -> Result<()> {
        // Directory timestamps legitimately change when the child creates files.
        let actual = self.directory.file.metadata()?;
        let named = fs::symlink_metadata(&self.directory.path)?;
        ensure!(
            same_inode(&self.directory.metadata, &actual)
                && same_inode(&actual, &named)
                && actual.mode() == self.directory.metadata.mode()
                && actual.uid() == self.uid
                && actual.nlink() != 0,
            "resident directory changed"
        );
        for artifact in &self.artifacts {
            artifact.current()?;
        }
        self.admission.current()?;
        if consumed {
            let claimed = fs::symlink_metadata(self.directory.path.join("admission.consumed"))?;
            let original = self.admission.file.metadata()?;
            ensure!(
                same_file(&original, &claimed) && original.nlink() == 2 && claimed.nlink() == 2,
                "resident admission consumption mismatch"
            );
            self.admission.file.seek(SeekFrom::Start(0))?;
            let mut actual = Vec::new();
            (&mut self.admission.file)
                .take(169)
                .read_to_end(&mut actual)?;
            ensure!(
                actual == self.admission_bytes,
                "resident admission bytes changed"
            );
        }
        Ok(())
    }
}

fn before(deadline: Instant) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "resident startup deadline expired"
    );
    Ok(())
}
fn nonce() -> Uuid {
    Uuid::parse(&crate::storage::Uuid::new_v4().to_string()).expect("OS UUID")
}
fn same_inode(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    // Link-count/ctime change when the admission is legitimately consumed.
    same_inode(a, b)
        && a.mode() == b.mode()
        && a.uid() == b.uid()
        && a.size() == b.size()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
}
fn create_private(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}
fn pin(path: PathBuf, directory: bool) -> Result<PinnedFile> {
    let flags = libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(&path)?;
    Ok(PinnedFile {
        metadata: file.metadata()?,
        file,
        path,
    })
}
fn copy_artifact(
    input: &Artifact,
    path: &Path,
    bound: u64,
    mode: u32,
    uid: u32,
    deadline: Instant,
) -> Result<PinnedFile> {
    ensure!(
        input.sha256 != [0; 32] && input.path.is_absolute(),
        "resident artifact expectation invalid"
    );
    let mut source = pin(input.path.clone(), false)?;
    let info = &source.metadata;
    ensure!(
        info.is_file()
            && (info.uid() == uid || info.uid() == 0)
            && info.mode() & 0o22 == 0
            && info.size() > 0
            && info.size() <= bound,
        "resident artifact metadata refused"
    );
    let mut output = create_private(path)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        before(deadline)?;
        let count = source.file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= bound, "resident artifact exceeds bound");
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    ensure!(
        total == info.size() && <[u8; 32]>::from(hash.finalize()) == input.sha256,
        "resident artifact bytes mismatch"
    );
    source.current()?;
    output.set_permissions(fs::Permissions::from_mode(mode))?;
    output.sync_all()?;
    // Linux refuses executing an inode held open for writing (ETXTBSY).
    drop(output);
    pin(path.to_owned(), false)
}
fn environment(input: &BTreeMap<String, String>) -> Result<&BTreeMap<String, String>> {
    for (key, value) in input {
        ensure!(
            [
                "HOME",
                "USER",
                "LOGNAME",
                "PATH",
                "LANG",
                "LC_ALL",
                "TZ",
                "XDG_RUNTIME_DIR",
                "QT_QPA_PLATFORM"
            ]
            .contains(&key.as_str())
                && !value.contains('\0')
                && value.len() <= 4096,
            "resident environment variable refused"
        );
        if key == "QT_QPA_PLATFORM" {
            ensure!(
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "resident platform must be a simple identifier"
            );
        }
    }
    Ok(input)
}
fn read_text(path: &str) -> Result<String> {
    let mut value = String::new();
    File::open(path)?.take(4097).read_to_string(&mut value)?;
    ensure!(
        value.len() <= 4096,
        "resident process metadata exceeds bound"
    );
    Ok(value)
}
fn process_start(pid: u32) -> Result<u64> {
    let stat = read_text(&format!("/proc/{pid}/stat"))?;
    let value: u64 = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .context("invalid process stat")?
        .parse()?;
    ensure!(value != 0, "empty process incarnation");
    Ok(value)
}
fn boot_ms() -> Result<u64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    ensure!(
        unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } == 0
            && value.tv_sec >= 0
            && (0..1_000_000_000).contains(&value.tv_nsec),
        "boot clock unavailable"
    );
    (value.tv_sec as u64)
        .checked_mul(1000)
        .and_then(|v| v.checked_add(value.tv_nsec as u64 / 1_000_000))
        .context("boot clock overflow")
}

#[cfg(test)]
mod tests;
