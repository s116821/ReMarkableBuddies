//! Linux service ownership evidence. No service commands or native activation.
//! Group observations require the qualified platform and service-state bracket
//! before a recovery helper may use them; absence alone grants no authority.
use anyhow::{bail, ensure, Context, Result};
use nix::libc;
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{self, File, Metadata},
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

const RECORD_LIMIT: u64 = 32 * 1024;
const LEASE_NAME: &str = "activation.json";

pub mod health;
pub mod recovery;
pub mod service;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
}
impl Identity {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
            mode: metadata.mode(),
        }
    }
    fn validate_kind(&self, kind: u32) -> Result<()> {
        ensure!(
            self.inode != 0 && self.device != 0,
            "missing filesystem identity"
        );
        ensure!(
            self.mode & libc::S_IFMT == kind && self.mode & !(libc::S_IFMT | 0o7777) == 0,
            "invalid identity file type"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
}
impl ProcessIdentity {
    fn current() -> Result<Self> {
        Self::read(std::process::id())
    }
    fn read(pid: u32) -> Result<Self> {
        ensure!(pid > 1, "invalid process PID");
        let stat = text(&PathBuf::from(format!("/proc/{pid}/stat")), 4096)?;
        ensure!(
            stat.split_whitespace().next() == Some(pid.to_string().as_str()),
            "process PID mismatch"
        );
        let state = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next());
        ensure!(
            !matches!(state, None | Some("Z" | "X" | "x")),
            "process is not live"
        );
        let start_ticks = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(19))
            .context("invalid process stat")?
            .parse()?;
        let result = Self { pid, start_ticks };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.pid > 1 && self.start_ticks != 0,
            "invalid process identity"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextIdentity {
    boot: Uuid,
    mount_namespace: Identity,
    cgroup_namespace: Identity,
    mount_record: String,
    mount: Identity,
    slice: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupBaseline {
    mount_path: PathBuf,
    unit: String,
    context: ContextIdentity,
    group: Identity,
    parent: ProcessIdentity,
}

/// An observation, deliberately not a stock-restoration authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupObservation {
    Populated,
    EmptyOriginal,
    OriginalAbsent,
}

struct GroupContext {
    identity: ContextIdentity,
    slice: File,
}

impl GroupBaseline {
    /// Capture only this process's actual membership. The integration caller must
    /// additionally prove that it is the admitted Buddy unit's current MainPID.
    pub fn capture(mount_path: &Path, unit: &str) -> Result<Self> {
        Self::capture_process(mount_path, unit, std::process::id())
    }
    fn capture_process(mount_path: &Path, unit: &str, pid: u32) -> Result<Self> {
        validate_unit(unit)?;
        let parent = ProcessIdentity::read(pid)?;
        let context = GroupContext::read(mount_path)?;
        for (name, expected) in [
            ("mnt", &context.identity.mount_namespace),
            ("cgroup", &context.identity.cgroup_namespace),
        ] {
            ensure!(
                Identity::of(&fs::metadata(format!("/proc/{pid}/ns/{name}"))?) == *expected,
                "process namespace mismatch"
            );
        }
        let membership_path = PathBuf::from(format!("/proc/{pid}/cgroup"));
        let membership = text(&membership_path, 16 * 1024)?;
        validate_membership(&membership, unit)?;
        let group = open_at(&context.slice, unit, true, false)?;
        ensure!(populated(&group)?, "caller group is not populated");
        let baseline = Self {
            mount_path: mount_path.to_owned(),
            unit: unit.to_owned(),
            context: context.identity,
            group: Identity::of(&group.metadata()?),
            parent,
        };
        baseline.validate()?;
        ensure!(
            baseline.observe()? == GroupObservation::Populated,
            "group changed during capture"
        );
        ensure!(
            baseline.parent == ProcessIdentity::read(pid)?,
            "caller changed during capture"
        );
        ensure!(
            text(&membership_path, 16 * 1024)? == membership,
            "caller membership changed during capture"
        );
        Ok(baseline)
    }

    pub fn parent(&self) -> &ProcessIdentity {
        &self.parent
    }

    pub fn unit(&self) -> &str {
        &self.unit
    }

    pub fn observe(&self) -> Result<GroupObservation> {
        self.observe_with(GroupContext::read)
    }

    fn current_caller(&self) -> Result<()> {
        ensure!(
            self.parent == ProcessIdentity::current()?,
            "lease caller mismatch"
        );
        self.current_process()
    }

    fn current_process(&self) -> Result<()> {
        let membership = PathBuf::from(format!("/proc/{}/cgroup", self.parent.pid));
        let check = || -> Result<()> {
            ensure!(
                ProcessIdentity::read(self.parent.pid)? == self.parent,
                "group process incarnation changed"
            );
            validate_membership(&text(&membership, 16 * 1024)?, &self.unit)?;
            for (name, expected) in [
                ("mnt", &self.context.mount_namespace),
                ("cgroup", &self.context.cgroup_namespace),
            ] {
                ensure!(
                    Identity::of(&fs::metadata(format!(
                        "/proc/{}/ns/{name}",
                        self.parent.pid
                    ))?) == *expected,
                    "group process namespace changed"
                );
            }
            Ok(())
        };
        check()?;
        ensure!(
            self.observe()? == GroupObservation::Populated,
            "caller no longer owns populated group"
        );
        check()
    }

    fn observe_with(
        &self,
        read: impl Fn(&Path) -> Result<GroupContext>,
    ) -> Result<GroupObservation> {
        self.validate()?;
        let before = read(&self.mount_path)?;
        ensure!(before.identity == self.context, "cgroup context changed");
        let result = match open_at(&before.slice, &self.unit, true, false) {
            Ok(group) => {
                ensure!(
                    Identity::of(&group.metadata()?) == self.group,
                    "original cgroup replaced"
                );
                if populated(&group)? {
                    GroupObservation::Populated
                } else {
                    GroupObservation::EmptyOriginal
                }
            }
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                GroupObservation::OriginalAbsent
            }
            Err(error) => return Err(error.into()),
        };
        let after = read(&self.mount_path)?;
        ensure!(
            after.identity == self.context,
            "cgroup context changed during lookup"
        );
        // Missing events/parents and replaced names are never deletion evidence.
        match open_at(&after.slice, &self.unit, true, false) {
            Ok(group) => {
                ensure!(
                    result != GroupObservation::OriginalAbsent
                        && Identity::of(&group.metadata()?) == self.group,
                    "cgroup name changed during observation"
                );
            }
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                ensure!(
                    result == GroupObservation::OriginalAbsent,
                    "cgroup disappeared during observation"
                );
            }
            Err(error) => return Err(error.into()),
        }
        Ok(result)
    }

    fn validate(&self) -> Result<()> {
        validate_absolute(&self.mount_path)?;
        validate_unit(&self.unit)?;
        self.parent.validate()?;
        self.group.validate_kind(libc::S_IFDIR)?;
        for identity in [
            &self.context.mount_namespace,
            &self.context.cgroup_namespace,
        ] {
            identity.validate_kind(libc::S_IFREG)?;
        }
        self.context.mount.validate_kind(libc::S_IFDIR)?;
        self.context.slice.validate_kind(libc::S_IFDIR)?;
        ensure!(
            self.group.device == self.context.mount.device
                && self.context.slice.device == self.context.mount.device
                && self.group.inode != self.context.slice.inode
                && self.group.inode != self.context.mount.inode
                && self.context.slice.inode != self.context.mount.inode,
            "cgroup filesystem relationship invalid"
        );
        ensure!(!self.context.boot.is_nil(), "empty boot identity");
        ensure!(
            self.context.mount_record.len() <= 4096,
            "mount record exceeds bound"
        );
        validate_mount_record(&self.context.mount_record, &self.mount_path)?;
        Ok(())
    }
}

impl GroupContext {
    fn read(path: &Path) -> Result<Self> {
        let mount = open_absolute_directory(path)?;
        let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
        ensure!(
            unsafe { libc::fstatfs(mount.as_raw_fd(), info.as_mut_ptr()) } == 0,
            "cgroup filesystem unavailable"
        );
        ensure!(
            unsafe { info.assume_init() }.f_type == libc::CGROUP2_SUPER_MAGIC,
            "not a cgroup2 filesystem"
        );
        let records = text(Path::new("/proc/self/mountinfo"), 256 * 1024)?;
        let expected = path.to_str().context("non-UTF8 mount")?;
        let candidates: Vec<_> = records
            .lines()
            .filter(|line| line.split_whitespace().nth(4) == Some(expected))
            .collect();
        ensure!(candidates.len() == 1, "missing or ambiguous cgroup mount");
        validate_mount_record(candidates[0], path)?;
        let slice = open_at(&mount, "system.slice", true, false)?;
        Ok(Self {
            identity: ContextIdentity {
                boot: Uuid::parse_str(
                    text(Path::new("/proc/sys/kernel/random/boot_id"), 64)?.trim(),
                )?,
                mount_namespace: Identity::of(&fs::metadata("/proc/self/ns/mnt")?),
                cgroup_namespace: Identity::of(&fs::metadata("/proc/self/ns/cgroup")?),
                mount_record: candidates[0].to_owned(),
                mount: Identity::of(&mount.metadata()?),
                slice: Identity::of(&slice.metadata()?),
            },
            slice,
        })
    }
}

/// Facts sampled/verified by the service integration before publishing a lease.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockBaseline {
    pub process: ProcessIdentity,
    pub invocation: String,
    pub executable_sha256: [u8; 32],
    pub unit_sha256: [u8; 32],
    pub dropin_sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    version: u32,
    activation: Uuid,
    group: GroupBaseline,
    buddy_invocation: String,
    buddy_start_micros: u64,
    stock: StockBaseline,
    profile_sha256: [u8; 32],
    adapter_sha256: [u8; 32],
    recovery_profile_sha256: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    companion: Option<String>,
}
impl LeaseRecord {
    pub fn new(
        group: GroupBaseline,
        buddy_invocation: String,
        buddy_start_micros: u64,
        stock: StockBaseline,
        profile_sha256: [u8; 32],
        adapter_sha256: [u8; 32],
        recovery_profile_sha256: [u8; 32],
    ) -> Result<Self> {
        let record = Self {
            version: 2,
            activation: Uuid::new_v4(),
            group,
            buddy_invocation,
            buddy_start_micros,
            stock,
            profile_sha256,
            adapter_sha256,
            recovery_profile_sha256,
            companion: None,
        };
        record.validate()?;
        Ok(record)
    }
    fn with_companion(mut self, baseline: String) -> Result<Self> {
        self.version = 3;
        self.companion = Some(baseline);
        self.validate()?;
        Ok(self)
    }
    pub fn group(&self) -> &GroupBaseline {
        &self.group
    }
    pub fn stock(&self) -> &StockBaseline {
        &self.stock
    }
    /// Must be verified against Buddy's actual service invocation before takeover.
    pub fn buddy_invocation(&self) -> &str {
        &self.buddy_invocation
    }
    /// Exact ExecMainStartTimestampMonotonic sample, distinct from proc start ticks.
    /// Must be verified with MainPID and InvocationID in the admission bracket.
    pub fn buddy_start_micros(&self) -> u64 {
        self.buddy_start_micros
    }
    pub fn activation(&self) -> Uuid {
        self.activation
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            ((self.version == 2 && self.companion.is_none())
                || (self.version == 3 && self.companion.is_some()))
                && !self.activation.is_nil(),
            "unsupported or empty lease"
        );
        self.group.validate()?;
        if let Some(record) = &self.companion {
            let original = service::baseline::ServiceBaseline::decode(record.as_bytes())?;
            ensure!(
                original.group().unit == "rm-sync.service"
                    && original.group().group != self.group.group
                    && original.group().mount_path == self.group.mount_path
                    && original.group().context == self.group.context,
                "companion lease context mismatch"
            );
        }
        self.stock.process.validate()?;
        validate_invocation(&self.buddy_invocation)?;
        ensure!(
            self.buddy_start_micros != 0,
            "missing Buddy service start time"
        );
        validate_invocation(&self.stock.invocation)?;
        for digest in [
            self.profile_sha256,
            self.adapter_sha256,
            self.recovery_profile_sha256,
            self.stock.executable_sha256,
            self.stock.unit_sha256,
            self.stock.dropin_sha256,
        ] {
            ensure!(digest != [0; 32], "empty artifact digest");
        }
        Ok(())
    }
}

/// Immutable boot/session ownership, not a native effect journal. Presence is a
/// latch: no overwrite, repair, automatic deletion or second activation API.
pub struct Lease {
    directory: File,
    path: PathBuf,
    identity: Identity,
    file: File,
    file_metadata: Metadata,
    record: LeaseRecord,
    bytes: Vec<u8>,
}
impl Lease {
    /// Only returns after exclusive publication and both synchronization steps.
    /// Errors after creation deliberately leave the occupied record untouched.
    pub fn publish(path: &Path, record: LeaseRecord) -> Result<Self> {
        Self::publish_checked(path, record, GroupBaseline::current_caller)
    }
    fn publish_checked(
        path: &Path,
        record: LeaseRecord,
        mut check: impl FnMut(&GroupBaseline) -> Result<()>,
    ) -> Result<Self> {
        record.validate()?;
        check(&record.group)?;
        let lease = Self::publish_inner(path, record)?;
        // A late mismatch leaves the synchronized lease occupied. The caller
        // receives no activation authority, and must not stop stock.
        check(&lease.record.group)?;
        lease.current()?;
        Ok(lease)
    }
    fn publish_inner(path: &Path, record: LeaseRecord) -> Result<Self> {
        Self::publish_using(path, record, |file, bytes| {
            file.write_all(bytes)?;
            Ok(())
        })
    }
    fn publish_using(
        path: &Path,
        record: LeaseRecord,
        write: impl FnOnce(&mut File, &[u8]) -> Result<()>,
    ) -> Result<Self> {
        record.validate()?;
        let bytes = serde_json::to_vec(&record)?;
        ensure!(bytes.len() as u64 <= RECORD_LIMIT, "lease exceeds bound");
        let directory = private_directory(path)?;
        let identity = Identity::of(&directory.metadata()?);
        let mut file = open_at(&directory, LEASE_NAME, false, true)?;
        write(&mut file, &bytes)?;
        file.sync_all()?;
        directory.sync_all()?;
        let file_metadata = file.metadata()?;
        let lease = Self {
            directory,
            path: path.to_owned(),
            identity,
            file,
            file_metadata,
            record,
            bytes,
        };
        lease.current()?;
        Ok(lease)
    }
    /// Repeated reads permit recovery retries, including after helper failure.
    /// Reading this record can never authorize a new native activation.
    pub fn reopen(path: &Path) -> Result<Self> {
        Self::reopen_if_present(path)?.context("activation record absent")
    }
    // Only the exact initial final-component ENOENT may become None. Once an
    // occupied record was observed, every later failure remains an error.
    fn reopen_if_present(path: &Path) -> Result<Option<Self>> {
        let directory = private_directory(path)?;
        let identity = Identity::of(&directory.metadata()?);
        let file = match open_at(&directory, LEASE_NAME, false, false) {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                absent_record(path, &directory, &identity)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let file_metadata = file.metadata()?;
        let bytes = read_record(&directory)?;
        let record: LeaseRecord = serde_json::from_slice(&bytes)?;
        record.validate()?;
        ensure!(
            record.group.context.boot
                == Uuid::parse_str(text(Path::new("/proc/sys/kernel/random/boot_id"), 64)?.trim())?,
            "lease belongs to a different boot"
        );
        let lease = Self {
            directory,
            path: path.to_owned(),
            identity,
            file,
            file_metadata,
            record,
            bytes,
        };
        lease.current()?;
        Ok(Some(lease))
    }
    pub fn record(&self) -> &LeaseRecord {
        &self.record
    }
    pub fn current(&self) -> Result<()> {
        ensure!(
            same_file(&self.file_metadata, &self.file.metadata()?)
                && same_file(
                    &self.file_metadata,
                    &open_at(&self.directory, LEASE_NAME, false, false)?.metadata()?
                ),
            "lease inode changed"
        );
        ensure!(
            Identity::of(&self.directory.metadata()?) == self.identity
                && Identity::of(&private_directory(&self.path)?.metadata()?) == self.identity,
            "lease directory changed"
        );
        ensure!(
            read_record(&self.directory)? == self.bytes,
            "lease bytes changed"
        );
        ensure!(
            same_file(&self.file_metadata, &self.file.metadata()?),
            "lease changed during read"
        );
        ensure!(
            Identity::of(&private_directory(&self.path)?.metadata()?) == self.identity,
            "lease directory changed during read"
        );
        Ok(())
    }
}

fn absent_record(path: &Path, directory: &File, identity: &Identity) -> Result<()> {
    let current = || -> Result<()> {
        ensure!(
            Identity::of(&directory.metadata()?) == *identity
                && Identity::of(&private_directory(path)?.metadata()?) == *identity,
            "lease directory changed during absence check"
        );
        Ok(())
    };
    current()?;
    match open_at(directory, LEASE_NAME, false, false) {
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
        Err(error) => return Err(error.into()),
        Ok(_) => bail!("activation record appeared during absence check"),
    }
    current()
}

fn read_record(directory: &File) -> Result<Vec<u8>> {
    let mut file = open_at(directory, LEASE_NAME, false, false)?;
    let before = file.metadata()?;
    ensure!(
        before.is_file()
            && before.nlink() == 1
            && before.uid() == unsafe { libc::geteuid() }
            && before.mode() & 0o7777 == 0o600
            && before.len() > 0
            && before.len() <= RECORD_LIMIT,
        "lease file metadata refused"
    );
    let mut bytes = Vec::new();
    (&mut file).take(RECORD_LIMIT + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() as u64 == before.len()
            && same_file(&before, &after)
            && same_file(
                &after,
                &open_at(directory, LEASE_NAME, false, false)?.metadata()?
            ),
        "lease file changed while reading"
    );
    Ok(bytes)
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    Identity::of(a) == Identity::of(b)
        && a.len() == b.len()
        && a.nlink() == b.nlink()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
fn private_directory(path: &Path) -> Result<File> {
    let file = open_absolute_directory(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o7777 == 0o700
            && meta.nlink() != 0,
        "lease directory must be owned mode0700"
    );
    Ok(file)
}
fn open_absolute_directory(path: &Path) -> Result<File> {
    validate_absolute(path)?;
    let mut directory = File::open("/")?;
    for component in path.components().skip(1) {
        let Component::Normal(name) = component else {
            bail!("invalid directory component")
        };
        directory = open_at(
            &directory,
            name.to_str().context("non-UTF8 component")?,
            true,
            false,
        )?;
    }
    Ok(directory)
}
fn open_at(directory: &File, name: &str, is_dir: bool, create: bool) -> std::io::Result<File> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid relative name",
        ));
    }
    let name = CString::new(name)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in name"))?;
    let flags = libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if is_dir { libc::O_DIRECTORY } else { 0 }
        | if create {
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY
        };
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn validate_absolute(path: &Path) -> Result<()> {
    let value = path.to_str().context("non-UTF8 path")?;
    ensure!(
        path.is_absolute()
            && value.len() <= 4096
            && value != "/"
            && !value
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b == b'\\' || b == 0)
            && value
                .split('/')
                .skip(1)
                .all(|c| !c.is_empty() && c != "." && c != ".."),
        "invalid absolute path"
    );
    Ok(())
}
fn validate_invocation(invocation: &str) -> Result<()> {
    ensure!(
        invocation.len() == 32
            && invocation
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && invocation.bytes().any(|b| b != b'0'),
        "invalid service invocation"
    );
    Ok(())
}
fn validate_membership(input: &str, unit: &str) -> Result<()> {
    let groups: Vec<_> = input
        .lines()
        .filter_map(|s| s.strip_prefix("0::"))
        .collect();
    ensure!(
        groups == [format!("/system.slice/{unit}")],
        "caller group mismatch"
    );
    Ok(())
}
fn validate_unit(unit: &str) -> Result<()> {
    ensure!(
        unit.len() <= 200
            && unit.ends_with(".service")
            && unit.len() > ".service".len()
            && unit[..unit.len() - 8]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid service unit"
    );
    Ok(())
}
fn validate_mount_record(record: &str, path: &Path) -> Result<()> {
    ensure!(
        !record.contains('\n') && !record.contains('\r'),
        "multiline mount record"
    );
    let (before, after) = record.split_once(" - ").context("invalid mount record")?;
    let fields: Vec<_> = before.split_whitespace().collect();
    let tail: Vec<_> = after.split_whitespace().collect();
    ensure!(
        fields.len() >= 6
            && fields[0].parse::<u64>().is_ok()
            && fields[1].parse::<u64>().is_ok()
            && fields[3] == "/"
            && Some(fields[4]) == path.to_str()
            && tail.len() == 3
            && tail[0] == "cgroup2",
        "unsupported cgroup mount"
    );
    Ok(())
}
fn populated(directory: &File) -> Result<bool> {
    let file = open_at(directory, "cgroup.events", false, false)?;
    ensure!(
        file.metadata()?.is_file(),
        "cgroup events is not a regular file"
    );
    let mut input = String::new();
    file.take(4097).read_to_string(&mut input)?;
    ensure!(input.len() <= 4096, "cgroup events exceeds bound");
    let mut population = None;
    let mut frozen = None;
    for line in input.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(
            fields.len() == 2 && ["0", "1"].contains(&fields[1]),
            "invalid cgroup event"
        );
        let target = match fields[0] {
            "populated" => &mut population,
            "frozen" => &mut frozen,
            _ => bail!("unknown cgroup event"),
        };
        ensure!(
            target.replace(fields[1] == "1").is_none(),
            "duplicate cgroup event"
        );
    }
    population.context("missing cgroup population")
}
fn text(path: &Path, bound: u64) -> Result<String> {
    let mut result = String::new();
    File::open(path)?
        .take(bound + 1)
        .read_to_string(&mut result)?;
    ensure!(
        result.len() as u64 <= bound,
        "process metadata exceeds bound"
    );
    Ok(result)
}

#[cfg(test)]
mod tests;
