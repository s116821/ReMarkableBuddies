//! Finite Linux custody for one streamed isolated package.
use super::{Data, Operation, Report, Status, MAX_INPUT, MAX_OUTPUT};
use crate::{
    config::Config,
    storage::{files, Store, Uuid},
};
use anyhow::{ensure, Context, Result};
use nix::libc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, OsStr, OsString},
    fs::{self, File, Metadata},
    io::{self, Read, Seek, Write},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const PARENT: &str = ".remarkable-buddies-restore-stage-v1";
const OWNER: &str = "owner";
const OWNER_BYTES: &[u8] = b"remarkable-buddies-restore-stage-v1\n";
const DATA_DIR: &str = "data";
const OBJECTS: &str = "objects";
const BACKUP: &str = "backup.json";
const PORTABLE: &str = "portable-configuration.json";
const MARKER: &str = "isolated-backup.json";
const CONFIG_PATH: &str = "/home/root/.config/remarkable-buddies/config.json";
const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const INVENTORY_DOMAIN: &[u8] = b"remarkable-buddies-restore-stage-inventory-v1\0";
const CUSTODY_DOMAIN: &[u8] = b"remarkable-buddies-restore-stage-custody-v1\0";
const MAX_HEADER: usize = 4 * 1024;
const MAX_OBJECTS: u32 = 4096;
const MIB: u64 = 1024 * 1024;
const MAX_BACKUP: u64 = 8 * MIB;
const MAX_PORTABLE: u64 = 128 * 1024;
const MAX_MARKER: u64 = 16 * 1024;
const MAX_OBJECT: u64 = 256 * MIB;
const MAX_PAYLOAD: u64 = 512 * MIB;
const RESERVE: u64 = 256 * MIB;
const MAX_BUDGET_MS: u64 = 12_000;
const CLEANUP_BUDGET: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    stage_id: String,
    boot_id: String,
    marker_sha256: String,
    inventory_sha256: String,
    object_count: u32,
    payload_bytes: u64,
    budget_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    stage_id: String,
    boot_id: String,
    marker_sha256: String,
    inventory_sha256: String,
    root: PathBuf,
    custody_sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Settlement {
    NoOutput,
    Cleaned,
    Unconfirmed,
    Sealed,
    Verified,
    Released,
}

#[derive(Serialize)]
pub(super) struct StageData {
    settlement: Settlement,
    receipt: Option<Receipt>,
}

fn report(
    operation: Operation,
    status: Status,
    settlement: Settlement,
    receipt: Option<Receipt>,
) -> Report {
    let mut report = Report::new(operation, status);
    report.data = Some(Data::Stage(StageData {
        settlement,
        receipt,
    }));
    report
}

struct Input {
    fd: RawFd,
    start: Instant,
    deadline: Instant,
}

impl Input {
    fn new(fd: RawFd) -> Self {
        let start = Instant::now();
        Self {
            fd,
            start,
            deadline: start + Duration::from_millis(MAX_BUDGET_MS),
        }
    }

    fn line(&mut self) -> Result<Vec<u8>> {
        let mut line = Vec::new();
        while line.len() < MAX_HEADER {
            let mut byte = [0];
            self.read_exact(&mut byte)?;
            if byte[0] == b'\n' {
                return Ok(line);
            }
            line.push(byte[0]);
        }
        anyhow::bail!("stage control line exceeds bound")
    }
}

impl Read for Input {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let mut poll_fd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let wait_ms = (remaining.as_millis() + 1) as libc::c_int;
            // The CLI owns the single stdin reader. POLLHUP also permits reading
            // buffered bytes or observing EOF when the writer closes its pipe.
            let ready = unsafe { libc::poll(&mut poll_fd, 1, wait_ms) };
            if ready == 0 {
                continue;
            }
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if Instant::now() >= self.deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let count = unsafe { libc::read(self.fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count >= 0 {
                return Ok(count as usize);
            }
            let error = io::Error::last_os_error();
            if !matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                return Err(error);
            }
        }
    }
}

fn within(deadline: Instant) -> Result<()> {
    ensure!(Instant::now() < deadline, "stage deadline exceeded");
    Ok(())
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(value).ok()?;
    (id.hyphenated().to_string() == value).then_some(id)
}

fn original_id(stage: &str, boot: &str, marker: &str, inventory: &str) -> Result<()> {
    ensure!(
        canonical_uuid(stage).is_some_and(|id| id.get_version_num() == 4),
        "invalid stage ID"
    );
    ensure!(canonical_uuid(boot).is_some(), "invalid boot ID");
    ensure!(
        crate::storage::valid_digest(marker) && crate::storage::valid_digest(inventory),
        "invalid stage digest"
    );
    Ok(())
}

fn require_boot(boot: &str) -> Result<()> {
    let mut boot_bytes = String::new();
    File::open(BOOT_ID_PATH)?
        .take(40)
        .read_to_string(&mut boot_bytes)?;
    ensure!(
        boot_bytes.strip_suffix('\n') == Some(boot),
        "stage boot changed"
    );
    Ok(())
}

fn fd_result(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_at(parent: &File, name: &OsStr, flags: libc::c_int) -> io::Result<File> {
    let name = CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600 as libc::mode_t,
        )
    };
    fd_result(fd)?;
    // Successful openat returns an independently owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn mkdir_at(parent: &File, name: &OsStr) -> io::Result<()> {
    let name = CString::new(name.as_bytes())?;
    fd_result(unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) })
}

fn remove_at(parent: &File, name: &OsStr, directory: bool) -> Result<()> {
    let name = CString::new(name.as_bytes())?;
    let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
    fd_result(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) })?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
}

impl Identity {
    fn from_meta(meta: &Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            mode: meta.mode(),
            uid: meta.uid(),
            gid: meta.gid(),
        }
    }

    fn same_inode(self, other: Self) -> bool {
        (self.dev, self.ino) == (other.dev, other.ino)
    }

    fn hash(self, digest: &mut Sha256) {
        digest.update(self.dev.to_be_bytes());
        digest.update(self.ino.to_be_bytes());
        digest.update(self.mode.to_be_bytes());
        digest.update(self.uid.to_be_bytes());
        digest.update(self.gid.to_be_bytes());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    identity: Identity,
    links: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl Stamp {
    fn read(file: &File, device: u64) -> Result<Self> {
        let meta = file.metadata()?;
        let identity = Identity::from_meta(&meta);
        ensure!(
            meta.is_file()
                && identity.dev == device
                && identity.uid == unsafe { libc::geteuid() }
                && (identity.mode & 0o7777) == 0o600
                && meta.nlink() == 1,
            "stage member is not a private single-link file"
        );
        Ok(Self {
            identity,
            links: meta.nlink(),
            size: meta.len(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
        })
    }

    fn hash(self, digest: &mut Sha256) {
        self.identity.hash(digest);
        digest.update(self.links.to_be_bytes());
        digest.update(self.size.to_be_bytes());
        digest.update(self.mtime.to_be_bytes());
        digest.update(self.mtime_nsec.to_be_bytes());
        digest.update(self.ctime.to_be_bytes());
        digest.update(self.ctime_nsec.to_be_bytes());
    }
}

struct Dir {
    name: OsString,
    file: File,
    identity: Identity,
}

impl Dir {
    fn new(name: OsString, file: File) -> Result<Self> {
        let meta = file.metadata()?;
        ensure!(meta.is_dir(), "expected stage directory");
        Ok(Self {
            name,
            file,
            identity: Identity::from_meta(&meta),
        })
    }

    fn open(parent: &File, name: &OsStr) -> Result<Self> {
        Self::new(name.to_owned(), open_at(parent, name, libc::O_DIRECTORY)?)
    }

    fn private(&self, device: u64) -> Result<()> {
        ensure!(
            self.identity.dev == device
                && self.identity.uid == unsafe { libc::geteuid() }
                && (self.identity.mode & 0o7777) == 0o700,
            "stage directory is not private"
        );
        Ok(())
    }

    fn verify(&self, parent: &File) -> Result<()> {
        ensure!(
            Self::open(parent, &self.name)?.identity == self.identity
                && Identity::from_meta(&self.file.metadata()?) == self.identity,
            "stage directory identity changed"
        );
        Ok(())
    }

    fn entries(&self, limit: usize, deadline: Instant) -> Result<Vec<OsString>> {
        struct Entries(*mut libc::DIR);
        impl Drop for Entries {
            fn drop(&mut self) {
                unsafe { libc::closedir(self.0) };
            }
        }
        // A separate open description starts at offset zero on every scan.
        let scan = open_at(&self.file, OsStr::new("."), libc::O_DIRECTORY)?;
        let entries = unsafe { libc::fdopendir(scan.as_raw_fd()) };
        if entries.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        // fdopendir now owns the descriptor.
        std::mem::forget(scan);
        let entries = Entries(entries);
        let mut names = Vec::new();
        loop {
            within(deadline)?;
            unsafe { *libc::__errno_location() = 0 };
            let entry = unsafe { libc::readdir(entries.0) };
            if entry.is_null() {
                if unsafe { *libc::__errno_location() } != 0 {
                    return Err(io::Error::last_os_error().into());
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                ensure!(names.len() < limit, "too many stage entries");
                names.push(OsString::from_vec(name.to_vec()));
            }
        }
        names.sort_unstable();
        Ok(names)
    }
}

struct Parent {
    path: PathBuf,
    anchor: File,
    ancestors: Vec<Dir>,
    parent: Dir,
    marker: Stamp,
}

impl Parent {
    fn open(config_path: Option<PathBuf>, create: bool, deadline: Instant) -> Result<Self> {
        within(deadline)?;
        let config_path = config_path.unwrap_or_else(|| CONFIG_PATH.into());
        let config = Config::load(Some(config_path.clone()))?;
        let data_parent = config.paths.data.parent().context("missing data parent")?;
        let path: PathBuf = data_parent.join(PARENT).components().collect();
        files::safe_path(&path)?;
        ensure!(
            serde_json::to_vec(&path)?.len() <= MAX_OUTPUT - 1024,
            "stage root cannot fit in a bounded receipt"
        );
        let protected = [
            config.paths.data.as_path(),
            config.paths.cache.as_path(),
            config.paths.credentials.as_path(),
            config_path.parent().context("missing config parent")?,
            Path::new("/home/root/.local/share/remarkable/xochitl"),
            Path::new("/home/root/.config/remarkable/xochitl.conf"),
        ];
        for root in protected {
            ensure!(
                !path.starts_with(root) && !root.starts_with(&path),
                "stage root overlaps a protected root"
            );
        }

        let anchor = File::open("/")?;
        let mut ancestors = Vec::<Dir>::new();
        for name in data_parent.iter().skip(1) {
            within(deadline)?;
            let base = ancestors.last().map_or(&anchor, |dir| &dir.file);
            ancestors.push(Dir::open(base, name)?);
        }
        let base = ancestors.last().context("missing stage ancestor")?;
        verify_chain(&anchor, &ancestors)?;
        for root in protected {
            within(deadline)?;
            if let Some(identity) = existing_identity(root)? {
                ensure!(
                    !ancestors
                        .iter()
                        .any(|dir| dir.identity.same_inode(identity)),
                    "stage anchor aliases a protected root"
                );
            }
        }
        let name = OsStr::new(PARENT);
        let created = if create {
            match mkdir_at(&base.file, name) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error.into()),
            }
        } else {
            false
        };
        let parent = Dir::open(&base.file, name)?;
        parent.private(base.identity.dev)?;
        // Check existing inode aliases as well as lexical overlap. This also
        // refuses a preexisting staging path mounted over a protected root.
        for root in protected {
            for ancestor in root.ancestors() {
                within(deadline)?;
                if let Some(identity) = existing_identity(ancestor)? {
                    ensure!(
                        !parent.identity.same_inode(identity),
                        "stage parent aliases a protected root"
                    );
                }
            }
        }
        verify_chain(&anchor, &ancestors)?;
        parent.verify(&base.file)?;
        if created {
            let mut marker = open_at(
                &parent.file,
                OsStr::new(OWNER),
                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
            )?;
            Stamp::read(&marker, parent.identity.dev)?;
            marker.write_all(OWNER_BYTES)?;
            marker.sync_all()?;
            parent.file.sync_all()?;
            base.file.sync_all()?;
        }
        let marker = read_owner_marker(&parent)?;
        let parent = Self {
            path,
            anchor,
            ancestors,
            parent,
            marker,
        };
        parent.verify(deadline)?;
        Ok(parent)
    }

    fn verify(&self, deadline: Instant) -> Result<()> {
        within(deadline)?;
        verify_chain(&self.anchor, &self.ancestors)?;
        self.parent.verify(
            &self
                .ancestors
                .last()
                .context("missing stage ancestor")?
                .file,
        )?;
        ensure!(
            read_owner_marker(&self.parent)? == self.marker,
            "stage owner marker changed"
        );
        within(deadline)
    }
}

fn existing_identity(path: &Path) -> Result<Option<Identity>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(Identity::from_meta(&meta))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn verify_chain(root: &File, ancestors: &[Dir]) -> Result<()> {
    let mut parent = root;
    for dir in ancestors {
        dir.verify(parent)?;
        parent = &dir.file;
    }
    Ok(())
}

fn read_owner_marker(parent: &Dir) -> Result<Stamp> {
    let owner = OsStr::new(OWNER);
    let observed = open_at(&parent.file, owner, libc::O_PATH)?;
    let stamp = Stamp::read(&observed, parent.identity.dev)?;
    let file = open_at(&parent.file, owner, libc::O_RDONLY)?;
    ensure!(
        Stamp::read(&file, parent.identity.dev)? == stamp,
        "stage parent owner marker changed"
    );
    let mut bytes = Vec::new();
    (&file)
        .take((OWNER_BYTES.len() + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes == OWNER_BYTES && Stamp::read(&file, parent.identity.dev)? == stamp,
        "stage parent owner marker is invalid"
    );
    Ok(stamp)
}

fn hash_name(digest: &mut Sha256, name: &[u8]) {
    digest.update((name.len() as u32).to_be_bytes());
    digest.update(name);
}

struct Node {
    dir: Dir,
    members: BTreeMap<OsString, Stamp>,
}

impl Node {
    fn open(parent: &Dir, name: &OsStr) -> Result<Self> {
        let dir = Dir::open(&parent.file, name)?;
        dir.private(parent.identity.dev)?;
        Ok(Self {
            dir,
            members: BTreeMap::new(),
        })
    }

    fn member_stamp(&self, name: &OsStr) -> Result<Stamp> {
        let member = open_at(&self.dir.file, name, libc::O_PATH)?;
        Stamp::read(&member, self.dir.identity.dev)
    }

    fn observe(&mut self, name: OsString, limit: u64) -> Result<()> {
        let stamp = self.member_stamp(&name)?;
        ensure!(stamp.size <= limit, "staged member exceeds its bound");
        self.members.insert(name, stamp);
        Ok(())
    }

    fn require_member(&self, name: &OsStr, stamp: Stamp, sealed: bool) -> Result<()> {
        let current = self.member_stamp(name)?;
        ensure!(
            current.identity == stamp.identity && (!sealed || current == stamp),
            "stage member changed"
        );
        Ok(())
    }

    fn verify(&self, child: Option<&Node>, sealed: bool, deadline: Instant) -> Result<()> {
        let size = self.members.len() + usize::from(child.is_some());
        let names = self.dir.entries(size, deadline)?;
        ensure!(names.len() == size, "missing stage member");
        for name in names {
            within(deadline)?;
            if child.is_some_and(|child| child.dir.name == name) {
                continue;
            }
            let stamp = self.members.get(&name).context("unexpected stage member")?;
            self.require_member(&name, *stamp, sealed)?;
        }
        Ok(())
    }

    fn remove_members(&self, sealed: bool, deadline: Instant) -> Result<()> {
        for (name, stamp) in &self.members {
            within(deadline)?;
            self.require_member(name, *stamp, sealed)?;
            remove_at(&self.dir.file, name, false)?;
        }
        Ok(())
    }

    fn hash(&self, digest: &mut Sha256) {
        hash_name(digest, self.dir.name.as_bytes());
        self.dir.identity.hash(digest);
        digest.update((self.members.len() as u32).to_be_bytes());
        for (name, stamp) in &self.members {
            hash_name(digest, name.as_bytes());
            stamp.hash(digest);
        }
    }
}

struct Tree {
    root: Node,
    data: Option<Node>,
    objects: Option<Node>,
}

impl Tree {
    fn open(parent: &Parent, name: &OsStr) -> Result<Self> {
        Ok(Self {
            root: Node::open(&parent.parent, name)?,
            data: None,
            objects: None,
        })
    }

    fn make_children(&mut self, deadline: Instant) -> Result<()> {
        within(deadline)?;
        mkdir_at(&self.root.dir.file, OsStr::new(DATA_DIR))?;
        let data = self
            .data
            .insert(Node::open(&self.root.dir, OsStr::new(DATA_DIR))?);
        within(deadline)?;
        mkdir_at(&data.dir.file, OsStr::new(OBJECTS))?;
        self.objects = Some(Node::open(&data.dir, OsStr::new(OBJECTS))?);
        Ok(())
    }

    fn observe(parent: &Parent, name: &OsStr, deadline: Instant) -> Result<Self> {
        let mut tree = Self::open(parent, name)?;
        tree.root.observe(PORTABLE.into(), MAX_PORTABLE)?;
        tree.root.observe(MARKER.into(), MAX_MARKER)?;
        let mut data = Node::open(&tree.root.dir, OsStr::new(DATA_DIR))?;
        data.observe(BACKUP.into(), MAX_BACKUP)?;
        let mut objects = Node::open(&data.dir, OsStr::new(OBJECTS))?;
        let mut total = data
            .members
            .values()
            .chain(tree.root.members.values())
            .map(|stamp| stamp.size)
            .sum::<u64>();
        for name in objects.dir.entries(MAX_OBJECTS as usize, deadline)? {
            ensure!(
                name.to_str().is_some_and(crate::storage::valid_digest),
                "invalid staged object name"
            );
            objects.observe(name.clone(), MAX_OBJECT)?;
            total += objects.members[&name].size;
            ensure!(total <= MAX_PAYLOAD, "stage exceeds aggregate bound");
        }
        tree.data = Some(data);
        tree.objects = Some(objects);
        tree.verify(parent, true, deadline)?;
        Ok(tree)
    }

    fn verify_links(&self, parent: &Parent, deadline: Instant) -> Result<()> {
        parent.verify(deadline)?;
        self.root.dir.verify(&parent.parent.file)?;
        if let Some(data) = &self.data {
            data.dir.verify(&self.root.dir.file)?;
            if let Some(objects) = &self.objects {
                objects.dir.verify(&data.dir.file)?;
            }
        }
        Ok(())
    }

    fn verify(&self, parent: &Parent, sealed: bool, deadline: Instant) -> Result<()> {
        self.verify_links(parent, deadline)?;
        self.root.verify(self.data.as_ref(), sealed, deadline)?;
        if let Some(data) = &self.data {
            data.verify(self.objects.as_ref(), sealed, deadline)?;
        }
        if let Some(objects) = &self.objects {
            objects.verify(None, sealed, deadline)?;
        }
        within(deadline)
    }

    fn remove(&self, parent: &Parent, sealed: bool, deadline: Instant) -> Result<()> {
        self.verify(parent, sealed, deadline)?;
        if let Some(data) = &self.data {
            if let Some(objects) = &self.objects {
                self.verify_links(parent, deadline)?;
                objects.remove_members(sealed, deadline)?;
                self.verify_links(parent, deadline)?;
                remove_at(&data.dir.file, &objects.dir.name, true)?;
            }
            parent.verify(deadline)?;
            self.root.dir.verify(&parent.parent.file)?;
            data.dir.verify(&self.root.dir.file)?;
            data.remove_members(sealed, deadline)?;
            parent.verify(deadline)?;
            self.root.dir.verify(&parent.parent.file)?;
            data.dir.verify(&self.root.dir.file)?;
            remove_at(&self.root.dir.file, &data.dir.name, true)?;
        }
        parent.verify(deadline)?;
        self.root.dir.verify(&parent.parent.file)?;
        self.root.remove_members(sealed, deadline)?;
        parent.verify(deadline)?;
        self.root.dir.verify(&parent.parent.file)?;
        remove_at(&parent.parent.file, &self.root.dir.name, true)?;
        parent.parent.file.sync_all()?;
        parent.verify(deadline)
    }

    fn custody(&self, parent: &Parent, receipt: &Receipt) -> String {
        let mut hash = Sha256::new();
        hash.update(CUSTODY_DOMAIN);
        for field in [
            &receipt.stage_id,
            &receipt.boot_id,
            &receipt.marker_sha256,
            &receipt.inventory_sha256,
        ] {
            hash_name(&mut hash, field.as_bytes());
        }
        hash_name(&mut hash, receipt.root.as_os_str().as_bytes());
        hash.update((parent.ancestors.len() as u32).to_be_bytes());
        for ancestor in &parent.ancestors {
            hash_name(&mut hash, ancestor.name.as_bytes());
            ancestor.identity.hash(&mut hash);
        }
        hash_name(&mut hash, parent.parent.name.as_bytes());
        parent.parent.identity.hash(&mut hash);
        parent.marker.hash(&mut hash);
        self.root.hash(&mut hash);
        for node in self.data.iter().chain(self.objects.iter()) {
            node.hash(&mut hash);
        }
        format!("{:x}", hash.finalize())
    }
}

fn enough_space(file: &File, remaining: u64, deadline: Instant) -> Result<()> {
    within(deadline)?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    fd_result(unsafe { libc::fstatvfs(file.as_raw_fd(), stat.as_mut_ptr()) })?;
    let stat = unsafe { stat.assume_init() };
    ensure!(
        reserve_available(stat.f_bavail.into(), stat.f_frsize.into(), remaining),
        "stage filesystem reserve unavailable"
    );
    within(deadline)
}

fn reserve_available(blocks: u128, block_size: u128, remaining: u64) -> bool {
    blocks
        .checked_mul(block_size)
        .is_some_and(|free| free >= u128::from(RESERVE) + u128::from(remaining))
}

fn header(input: &mut Input) -> Result<Header> {
    let header: Header = serde_json::from_slice(&input.line()?)?;
    original_id(
        &header.stage_id,
        &header.boot_id,
        &header.marker_sha256,
        &header.inventory_sha256,
    )?;
    ensure!(
        header.version == 1
            && header.object_count <= MAX_OBJECTS
            && header.payload_bytes <= MAX_PAYLOAD
            && (1..=MAX_BUDGET_MS).contains(&header.budget_ms),
        "invalid stage control bounds"
    );
    input.deadline = input.start + Duration::from_millis(header.budget_ms);
    within(input.deadline)?;
    Ok(header)
}

fn stage_name(id: &str) -> OsString {
    format!("restore-v1-{id}").into()
}

struct Member {
    len: u64,
    digest: [u8; 32],
}

struct Upload<'a, R> {
    input: &'a mut R,
    remaining: u64,
    inventory: Sha256,
    deadline: Instant,
    buffer: Vec<u8>,
}

impl<R: Read> Upload<'_, R> {
    fn descriptor(&mut self, min: u64, max: u64) -> Result<Member> {
        within(self.deadline)?;
        let mut descriptor = [0; 40];
        self.input.read_exact(&mut descriptor)?;
        let len = u64::from_be_bytes(descriptor[..8].try_into()?);
        ensure!(
            (min..=max).contains(&len) && len <= self.remaining,
            "invalid stage member length"
        );
        self.inventory.update(descriptor);
        Ok(Member {
            len,
            digest: descriptor[8..].try_into()?,
        })
    }

    fn write(&mut self, node: &mut Node, name: OsString, member: Member) -> Result<()> {
        enough_space(&node.dir.file, self.remaining, self.deadline)?;
        let mut file = open_at(
            &node.dir.file,
            &name,
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
        )?;
        let created = Stamp::read(&file, node.dir.identity.dev)?;
        node.members.insert(name.clone(), created);
        let mut left = member.len;
        let mut incoming = Sha256::new();
        while left > 0 {
            let chunk = (left.min(self.buffer.len() as u64)) as usize;
            self.input.read_exact(&mut self.buffer[..chunk])?;
            enough_space(&node.dir.file, self.remaining, self.deadline)?;
            file.write_all(&self.buffer[..chunk])?;
            incoming.update(&self.buffer[..chunk]);
            self.remaining -= chunk as u64;
            left -= chunk as u64;
        }
        ensure!(
            <[u8; 32]>::from(incoming.finalize()) == member.digest,
            "stage stream digest mismatch"
        );
        within(self.deadline)?;
        file.sync_all()?;
        let synced = Stamp::read(&file, node.dir.identity.dev)?;
        ensure!(
            synced.identity == created.identity && synced.size == member.len,
            "stage member length changed"
        );
        file.rewind()?;
        let mut readback = Sha256::new();
        let mut left = member.len;
        while left > 0 {
            within(self.deadline)?;
            let chunk = (left.min(self.buffer.len() as u64)) as usize;
            file.read_exact(&mut self.buffer[..chunk])?;
            readback.update(&self.buffer[..chunk]);
            left -= chunk as u64;
        }
        ensure!(
            <[u8; 32]>::from(readback.finalize()) == member.digest
                && Stamp::read(&file, node.dir.identity.dev)? == synced,
            "stage readback changed"
        );
        node.members.insert(name, synced);
        within(self.deadline)
    }
}

fn receive_body(
    tree: &mut Tree,
    parent: &Parent,
    request: &Header,
    input: &mut impl Read,
    deadline: Instant,
) -> Result<Receipt> {
    tree.make_children(deadline)?;
    let data = tree.data.as_mut().context("missing stage data")?;
    let objects = tree.objects.as_mut().context("missing stage objects")?;
    let mut upload = Upload {
        input,
        remaining: request.payload_bytes,
        inventory: Sha256::new(),
        deadline,
        buffer: vec![0; 64 * 1024],
    };
    upload.inventory.update(INVENTORY_DOMAIN);
    upload.inventory.update(request.object_count.to_be_bytes());
    let backup = upload.descriptor(1, MAX_BACKUP)?;
    upload.write(data, BACKUP.into(), backup)?;
    let mut previous = None;
    for _ in 0..request.object_count {
        let object = upload.descriptor(0, MAX_OBJECT)?;
        ensure!(
            previous.is_none_or(|previous| previous < object.digest),
            "stage objects out of order"
        );
        previous = Some(object.digest);
        upload.write(objects, hex_digest(&object.digest).into(), object)?;
    }
    let portable = upload.descriptor(1, MAX_PORTABLE)?;
    upload.write(&mut tree.root, PORTABLE.into(), portable)?;
    let marker = upload.descriptor(1, MAX_MARKER)?;
    ensure!(
        hex_digest(&marker.digest) == request.marker_sha256,
        "stage marker differs from original"
    );
    upload.write(&mut tree.root, MARKER.into(), marker)?;
    within(deadline)?;
    ensure!(
        upload.remaining == 0
            && format!("{:x}", upload.inventory.finalize()) == request.inventory_sha256,
        "stage inventory differs from original"
    );
    let mut extra = [0];
    ensure!(upload.input.read(&mut extra)? == 0, "trailing stage bytes");
    for node in [&*objects, &*data, &tree.root] {
        within(deadline)?;
        node.dir.file.sync_all()?;
    }
    parent.parent.file.sync_all()?;
    tree.verify(parent, true, deadline)?;
    let root = parent.path.join(&tree.root.dir.name);
    Store::inspect_isolated_package(&root, Some(&request.marker_sha256))?;
    tree.verify(parent, true, deadline)?;
    let mut receipt = Receipt {
        version: 1,
        stage_id: request.stage_id.clone(),
        boot_id: request.boot_id.clone(),
        marker_sha256: request.marker_sha256.clone(),
        inventory_sha256: request.inventory_sha256.clone(),
        root,
        custody_sha256: String::new(),
    };
    receipt.custody_sha256 = tree.custody(parent, &receipt);
    within(deadline)?;
    Ok(receipt)
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(64);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("String writes are infallible");
    }
    result
}

fn receive(
    config: Option<PathBuf>,
    request: Header,
    input: &mut impl Read,
    deadline: Instant,
) -> Report {
    let operation = Operation::ReceiveIsolatedStage;
    let no_output = || report(operation, Status::Unavailable, Settlement::NoOutput, None);
    let Ok(parent) = Parent::open(config, true, deadline) else {
        return no_output();
    };
    if enough_space(&parent.parent.file, request.payload_bytes, deadline)
        .and_then(|()| parent.verify(deadline))
        .is_err()
    {
        return no_output();
    }
    let name = stage_name(&request.stage_id);
    match mkdir_at(&parent.parent.file, &name) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return no_output(),
        Err(_) => return report(operation, Status::Failed, Settlement::Unconfirmed, None),
    }
    let Ok(mut tree) = Tree::open(&parent, &name) else {
        return report(operation, Status::Failed, Settlement::Unconfirmed, None);
    };
    match receive_body(&mut tree, &parent, &request, input, deadline) {
        Ok(receipt) => report(operation, Status::Ok, Settlement::Sealed, Some(receipt)),
        Err(_) => {
            // Cleanup consumes the original attempt's remaining time, up to a short cap.
            let cleanup_deadline = deadline.min(Instant::now() + CLEANUP_BUDGET);
            let settlement = if tree.remove(&parent, false, cleanup_deadline).is_ok() {
                Settlement::Cleaned
            } else {
                Settlement::Unconfirmed
            };
            report(operation, Status::Failed, settlement, None)
        }
    }
}

fn reconcile(
    operation: Operation,
    config: Option<PathBuf>,
    receipt: Receipt,
    deadline: Instant,
) -> Result<Report> {
    let parent = Parent::open(config, false, deadline)?;
    let name = stage_name(&receipt.stage_id);
    ensure!(
        receipt.root == parent.path.join(&name),
        "stage root differs from original"
    );
    let tree = Tree::observe(&parent, &name, deadline)?;
    ensure!(
        tree.custody(&parent, &receipt) == receipt.custody_sha256,
        "stage custody differs from original"
    );
    within(deadline)?;
    if operation == Operation::ReleaseIsolatedStage {
        tree.remove(&parent, true, deadline)?;
        Ok(report(
            operation,
            Status::Ok,
            Settlement::Released,
            Some(receipt),
        ))
    } else {
        Ok(report(
            operation,
            Status::Ok,
            Settlement::Verified,
            Some(receipt),
        ))
    }
}

pub(super) fn run(operation: Operation, config: Option<PathBuf>, fd: RawFd) -> Report {
    let invalid = || report(operation, Status::InvalidInput, Settlement::NoOutput, None);
    let mut input = Input::new(fd);
    if operation == Operation::ReceiveIsolatedStage {
        let Ok(request) = header(&mut input) else {
            return invalid();
        };
        if require_boot(&request.boot_id).is_err() {
            return invalid();
        }
        let deadline = input.deadline;
        return receive(config, request, &mut input, deadline);
    }
    let mut bytes = Vec::new();
    if (&mut input)
        .take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_INPUT
    {
        return invalid();
    }
    let Ok(receipt) = serde_json::from_slice::<Receipt>(&bytes) else {
        return invalid();
    };
    if receipt.version != 1
        || original_id(
            &receipt.stage_id,
            &receipt.boot_id,
            &receipt.marker_sha256,
            &receipt.inventory_sha256,
        )
        .is_err()
        || !crate::storage::valid_digest(&receipt.custody_sha256)
        || require_boot(&receipt.boot_id).is_err()
    {
        return invalid();
    }
    reconcile(operation, config, receipt, input.deadline).unwrap_or_else(|_| {
        report(
            operation,
            Status::Unavailable,
            Settlement::Unconfirmed,
            None,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_maintenance::tests::isolated_package;
    use serde_json::Value;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn command_deadline() -> Instant {
        Instant::now() + Duration::from_millis(MAX_BUDGET_MS)
    }

    fn saved_config(root: &Path) -> PathBuf {
        let directory = root.join("target-config");
        fs::create_dir(&directory).unwrap();
        let config = directory.join("config.json");
        fs::write(&config, fs::read(root.join("config.json")).unwrap()).unwrap();
        config
    }

    fn producer_frame(package: &Path, marker: String, bad_backup: bool) -> (Header, Vec<u8>) {
        let data = package.join(DATA_DIR);
        let mut objects = fs::read_dir(data.join(OBJECTS))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        objects.sort_unstable();
        let object_count = objects.len() as u32;
        let files = [data.join(BACKUP)]
            .into_iter()
            .chain(objects)
            .chain([package.join(PORTABLE), package.join(MARKER)]);
        let mut inventory = Sha256::new();
        inventory.update(INVENTORY_DOMAIN);
        inventory.update(object_count.to_be_bytes());
        let mut payload_bytes = 0;
        let mut frame = Vec::new();
        for path in files {
            let mut bytes = fs::read(&path).unwrap();
            if bad_backup && path.ends_with(BACKUP) {
                bytes[0] ^= 1;
            }
            let len = (bytes.len() as u64).to_be_bytes();
            let digest = Sha256::digest(&bytes);
            inventory.update(len);
            inventory.update(digest);
            frame.extend(len);
            frame.extend(digest);
            frame.extend(&bytes);
            payload_bytes += bytes.len() as u64;
        }
        (
            Header {
                version: 1,
                stage_id: Uuid::new_v4().to_string(),
                boot_id: fs::read_to_string(BOOT_ID_PATH)
                    .unwrap()
                    .trim_end()
                    .to_owned(),
                marker_sha256: marker,
                inventory_sha256: format!("{:x}", inventory.finalize()),
                object_count,
                payload_bytes,
                budget_ms: MAX_BUDGET_MS,
            },
            frame,
        )
    }

    fn json(report: Report) -> Value {
        let mut bytes = Vec::new();
        report.write_to(&mut bytes).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
        serde_json::from_slice(&bytes).unwrap()
    }

    fn sealed(config: &Path, package: &Path, marker: &str) -> Receipt {
        let (request, frame) = producer_frame(package, marker.to_owned(), false);
        let report = json(receive(
            Some(config.to_owned()),
            request,
            &mut frame.as_slice(),
            command_deadline(),
        ));
        assert_eq!(report["status"], "ok");
        assert_eq!(report["activation_performed"], false);
        assert_eq!(report["data"]["settlement"], "sealed");
        serde_json::from_value(report["data"]["receipt"].clone()).unwrap()
    }

    #[test]
    fn actual_producer_receive_check_and_release() {
        let (fixture, package, marker, expected) = isolated_package(false);
        let config = saved_config(&fixture.root);
        let before = fixture.inventory();
        let receipt = sealed(&config, &package, &marker);
        assert_eq!(receipt.marker_sha256, marker);
        assert_eq!(
            receipt.root.parent(),
            Some(fixture.root.join(PARENT).as_path())
        );
        let observed = Store::inspect_isolated_package(&receipt.root, Some(&marker)).unwrap();
        assert_eq!(serde_json::to_value(observed).unwrap(), expected);
        let check = |receipt: Receipt| {
            reconcile(
                Operation::CheckIsolatedStage,
                Some(config.clone()),
                receipt,
                command_deadline(),
            )
        };
        let other = sealed(&config, &package, &marker);
        assert_eq!(
            json(check(receipt.clone()).unwrap())["data"]["settlement"],
            "verified"
        );
        let mut wrong = receipt.clone();
        wrong.inventory_sha256 = "0".repeat(64);
        assert!(check(wrong).is_err());
        let (mut repeated, frame) = producer_frame(&package, marker, false);
        repeated.stage_id.clone_from(&receipt.stage_id);
        let result = json(receive(
            Some(config.clone()),
            repeated,
            &mut frame.as_slice(),
            command_deadline(),
        ));
        assert_eq!(result["data"]["settlement"], "no-output");
        assert!(check(receipt.clone()).is_ok());
        let release = |receipt: Receipt| {
            reconcile(
                Operation::ReleaseIsolatedStage,
                Some(config.clone()),
                receipt,
                command_deadline(),
            )
        };
        assert_eq!(
            json(release(receipt.clone()).unwrap())["data"]["settlement"],
            "released"
        );
        assert!(!receipt.root.exists());
        assert!(release(receipt).is_err());
        assert!(release(other).is_ok());
        assert_eq!(fixture.inventory(), before);
    }

    #[test]
    fn failed_receives_remove_proven_partial_output() {
        let (fixture, package, marker, _) = isolated_package(false);
        let config = saved_config(&fixture.root);
        for fault in [
            "short-descriptor",
            "short-payload",
            "extra",
            "member-hash",
            "inventory-hash",
            "marker-hash",
            "oracle",
        ] {
            let (mut request, mut frame) =
                producer_frame(&package, marker.clone(), fault == "oracle");
            match fault {
                "short-descriptor" => frame.truncate(17),
                "short-payload" => {
                    frame.pop();
                }
                "extra" => frame.push(0),
                "member-hash" => frame[40] ^= 1,
                "inventory-hash" => request.inventory_sha256 = "0".repeat(64),
                "marker-hash" => request.marker_sha256 = "0".repeat(64),
                "oracle" => (),
                _ => unreachable!(),
            }
            let root = fixture
                .root
                .join(PARENT)
                .join(stage_name(&request.stage_id));
            let report = json(receive(
                Some(config.clone()),
                request,
                &mut frame.as_slice(),
                command_deadline(),
            ));
            assert_eq!(report["status"], "failed", "{fault}");
            assert_eq!(report["data"]["settlement"], "cleaned", "{fault}");
            assert!(report["data"]["receipt"].is_null(), "{fault}");
            assert!(!root.exists(), "{fault}");
        }
        assert_eq!(fs::read_dir(fixture.root.join(PARENT)).unwrap().count(), 1);
    }

    #[test]
    fn reconciliation_refuses_replaced_parent_or_member_and_extra_or_linked_members() {
        let (fixture, package, marker, _) = isolated_package(false);
        let config = saved_config(&fixture.root);
        let release = |receipt| {
            reconcile(
                Operation::ReleaseIsolatedStage,
                Some(config.clone()),
                receipt,
                command_deadline(),
            )
        };
        for fault in [
            "extra",
            "hardlink",
            "symlink",
            "same-bytes-replacement",
            "parent",
        ] {
            let receipt = sealed(&config, &package, &marker);
            let member = receipt.root.join(DATA_DIR).join(BACKUP);
            let outside = fixture.root.join(format!("outside-{fault}"));
            match fault {
                "extra" => fs::write(receipt.root.join("extra"), b"unrelated").unwrap(),
                "hardlink" => fs::hard_link(&member, &outside).unwrap(),
                "symlink" => {
                    fs::rename(&member, &outside).unwrap();
                    symlink(&outside, &member).unwrap();
                }
                "same-bytes-replacement" => {
                    fs::rename(&member, &outside).unwrap();
                    fs::copy(&outside, &member).unwrap();
                }
                "parent" => {
                    fs::rename(fixture.root.join(PARENT), &outside).unwrap();
                    fs::create_dir(fixture.root.join(PARENT)).unwrap();
                    fs::set_permissions(
                        fixture.root.join(PARENT),
                        fs::Permissions::from_mode(0o700),
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(release(receipt.clone()).is_err(), "{fault}");
            let retained = if fault == "parent" {
                outside.join(stage_name(&receipt.stage_id))
            } else {
                receipt.root
            };
            assert!(retained.join(MARKER).is_file(), "{fault}");
            assert!(retained.join(DATA_DIR).join(BACKUP).exists(), "{fault}");
        }
    }

    #[test]
    fn refuses_unmarked_parent_and_preserves_unexpected_partial_members() {
        let (fixture, package, marker, _) = isolated_package(false);
        let config = saved_config(&fixture.root);
        let path = fixture.root.join(PARENT);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(path.join("unrelated"), b"owned elsewhere").unwrap();
        let (request, frame) = producer_frame(&package, marker, false);
        let report = json(receive(
            Some(config.clone()),
            request,
            &mut frame.as_slice(),
            command_deadline(),
        ));
        assert_eq!(report["data"]["settlement"], "no-output");
        assert_eq!(
            fs::read(path.join("unrelated")).unwrap(),
            b"owned elsewhere"
        );
        assert!(!path.join(OWNER).exists());
        fs::rename(&path, fixture.root.join("unmarked")).unwrap();

        let deadline = command_deadline();
        let parent = Parent::open(Some(config), true, deadline).unwrap();
        let name = stage_name(&Uuid::new_v4().to_string());
        mkdir_at(&parent.parent.file, &name).unwrap();
        let mut tree = Tree::open(&parent, &name).unwrap();
        tree.make_children(deadline).unwrap();
        let data = parent.path.join(name).join(DATA_DIR);
        fs::write(data.join("unrelated"), b"owned elsewhere").unwrap();
        assert!(tree.remove(&parent, false, deadline).is_err());
        assert_eq!(
            fs::read(data.join("unrelated")).unwrap(),
            b"owned elsewhere"
        );
        assert!(data.join(OBJECTS).is_dir());
    }

    #[test]
    fn reserve_and_deadline_are_bounded() {
        assert!(reserve_available(u128::from(RESERVE) + 65_536, 1, 65_536));
        assert!(!reserve_available(u128::from(RESERVE) + 65_535, 1, 65_536));
        assert!(!reserve_available(u128::MAX, 2, MAX_PAYLOAD));
        let (fixture, package, marker, _) = isolated_package(false);
        let config = saved_config(&fixture.root);
        let (request, frame) = producer_frame(&package, marker, false);
        let report = json(receive(
            Some(config),
            request,
            &mut frame.as_slice(),
            Instant::now(),
        ));
        assert_eq!(report["data"]["settlement"], "no-output");
        assert!(!fixture.root.join(PARENT).exists());
    }
}
