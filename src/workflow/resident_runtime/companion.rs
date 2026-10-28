//! Owned vendor process. Service takeover/restoration remains the caller's job.
use super::*;
use crate::workflow::resident_supervisor::service::companion::{self as broker, BrokerOwner};
use std::os::unix::process::CommandExt;

/// An explicit launch selection, not a firmware capability claim. Installed
/// admission must independently pin both original services before using this.
pub struct Profile {
    executable: Artifact,
    size: u64,
    arguments: Vec<String>,
    #[cfg(test)]
    broker_file: Option<PathBuf>,
}
impl Profile {
    #[cfg(test)]
    pub(super) fn fixture(path: PathBuf, arguments: Vec<String>, broker_file: PathBuf) -> Self {
        let bytes = fs::read(&path).unwrap();
        Self {
            executable: Artifact {
                path,
                sha256: Sha256::digest(&bytes).into(),
            },
            size: bytes.len() as u64,
            arguments,
            broker_file: Some(broker_file),
        }
    }
    pub fn rm2_3_28_0_172() -> Self {
        Self {
            executable: Artifact {
                path: "/usr/bin/rm-sync".into(),
                sha256: [
                    0x1c, 0xaf, 0x64, 0x93, 0xcc, 0xe2, 0x13, 0x2a, 0xf8, 0x9a, 0x93, 0x77, 0x67,
                    0x9c, 0x19, 0x00, 0x5b, 0xef, 0x1c, 0x5b, 0x0e, 0x65, 0x8f, 0x70, 0x4f, 0x95,
                    0xb4, 0xfd, 0xbd, 0x07, 0x13, 0x8c,
                ],
            },
            size: 3_409_308,
            arguments: Vec::new(),
            #[cfg(test)]
            broker_file: None,
        }
    }
}

pub(super) struct Companion {
    executable: PinnedFile,
    expected_hash: [u8; 32],
    command: Command,
    cmdline: Vec<u8>,
    child: Option<Child>,
    start_ticks: Option<u64>,
    owner: Option<BrokerOwner>,
    failed: bool,
    #[cfg(test)]
    broker_file: Option<PathBuf>,
}
impl Companion {
    #[cfg(test)]
    pub(super) fn pid(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }
    pub(super) fn prepare(profile: Profile, directory: &Path, deadline: Instant) -> Result<Self> {
        before(deadline)?;
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "vendor companion requires root"
        );
        let executable = pin(profile.executable.path, false)?;
        let metadata = &executable.metadata;
        ensure!(
            metadata.is_file()
                && metadata.uid() == 0
                && metadata.gid() == 0
                && metadata.mode() & 0o7777 == 0o755
                && metadata.nlink() == 1
                && metadata.size() == profile.size
                && profile.size > 0
                && profile.size <= 16 * 1024 * 1024,
            "companion executable metadata refused"
        );
        let mut cmdline = executable.path.as_os_str().as_encoded_bytes().to_vec();
        cmdline.push(0);
        for argument in &profile.arguments {
            ensure!(!argument.contains('\0'), "companion argument contains NUL");
            cmdline.extend_from_slice(argument.as_bytes());
            cmdline.push(0);
        }
        ensure!(
            cmdline.len() <= 4096,
            "companion command line exceeds bound"
        );
        let mut command = Command::new(format!("/proc/self/fd/{}", executable.file.as_raw_fd()));
        command
            .arg0(&executable.path)
            .args(&profile.arguments)
            .current_dir("/")
            .env_clear()
            .env("HOME", "/home/root")
            .env("USER", "root")
            .env("LANG", "en_US.UTF-8")
            .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin")
            .stdin(Stdio::null())
            .stdout(create_private(&directory.join("companion-stdout.log"))?)
            .stderr(create_private(&directory.join("companion-stderr.log"))?);
        // Only async-signal-safe libc calls in the child before exec. The parent
        // retains its own umask/priority; no service/notification state is inherited.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o022);
                if libc::setpriority(libc::PRIO_PROCESS, 0, 19) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut prepared = Self {
            executable,
            expected_hash: profile.executable.sha256,
            command,
            cmdline,
            child: None,
            start_ticks: None,
            owner: None,
            failed: false,
            #[cfg(test)]
            broker_file: profile.broker_file,
        };
        prepared.hash_current(deadline)?;
        Ok(prepared)
    }
    fn file_current(&self) -> Result<()> {
        self.executable.current()?;
        let expected = &self.executable.metadata;
        let actual = self.executable.file.metadata()?;
        ensure!(
            actual.gid() == expected.gid()
                && actual.nlink() == expected.nlink()
                && actual.ctime() == expected.ctime()
                && actual.ctime_nsec() == expected.ctime_nsec()
                && fs::canonicalize(&self.executable.path)? == self.executable.path,
            "companion executable identity changed"
        );
        Ok(())
    }
    fn hash_current(&mut self, deadline: Instant) -> Result<()> {
        self.file_current()?;
        self.executable.file.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut total = 0;
        let mut buffer = [0u8; 65536];
        loop {
            before(deadline)?;
            let count = self.executable.file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            ensure!(
                total <= self.executable.metadata.size(),
                "companion bytes grew"
            );
            hash.update(&buffer[..count]);
        }
        ensure!(
            total == self.executable.metadata.size()
                && <[u8; 32]>::from(hash.finalize()) == self.expected_hash,
            "companion executable digest changed"
        );
        self.file_current()?;
        before(deadline)
    }
    pub(super) fn spawn(&mut self, deadline: Instant) -> Result<()> {
        ensure!(self.child.is_none(), "companion already spawned");
        self.hash_current(deadline)?;
        // Bind exec to the retained inode, not a mutable path. Root-owned file
        // changes remain integrity failures; this is not an atomic byte snapshot.
        self.child = Some(
            self.command
                .spawn()
                .context("spawn owned vendor companion")?,
        );
        self.start_ticks = Some(process_start(self.child.as_ref().unwrap().id())?);
        self.current()
    }
    pub(super) fn current(&mut self) -> Result<()> {
        ensure!(
            !self.failed,
            "companion session integrity previously failed"
        );
        let result = self.current_inner();
        if result.is_err() {
            self.failed = true;
        }
        result.map_err(integrity)
    }
    fn current_inner(&mut self) -> Result<()> {
        self.file_current()?;
        let child = self.child.as_mut().context("companion not running")?;
        ensure!(child.try_wait()?.is_none(), "owned companion exited");
        let pid = child.id();
        ensure!(
            Some(process_start(pid)?) == self.start_ticks
                && same_inode(
                    &self.executable.metadata,
                    &fs::metadata(format!("/proc/{pid}/exe"))?
                )
                && fs::read_link(format!("/proc/{pid}/exe"))? == self.executable.path,
            "companion process incarnation/executable changed"
        );
        let mut cmdline = Vec::new();
        File::open(format!("/proc/{pid}/cmdline"))?
            .take(4097)
            .read_to_end(&mut cmdline)?;
        ensure!(cmdline == self.cmdline, "companion command line changed");
        Ok(())
    }
    pub(super) fn observe(
        &mut self,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<bool> {
        let result = self.observe_inner(deadline, progress);
        result.map_err(|error| {
            if resident_request::is_request_retirement(&error) {
                error
            } else {
                self.failed = true;
                integrity(error)
            }
        })
    }
    fn observe_inner(
        &mut self,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<bool> {
        #[cfg(test)]
        let fixture = self.broker_file.clone();
        let mut checked = || {
            before(deadline)?;
            progress()?;
            self.current()?;
            before(deadline)
        };
        checked()?;
        // Test broker evidence is explicitly local; production always queries the
        // fixed system broker and never supplies a service activation operation.
        #[cfg(test)]
        let observed = if let Some(path) = fixture {
            checked()?;
            match fs::read_to_string(path) {
                Ok(value) => Some(BrokerOwner {
                    broker_id: "0".repeat(32),
                    unique_name: ":1.42".into(),
                    pid: value.trim().parse()?,
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            }
        } else {
            broker::observe_owner(deadline, &mut checked)?
        };
        #[cfg(not(test))]
        let observed = broker::observe_owner(deadline, &mut checked)?;
        checked()?;
        let Some(observed) = observed else {
            ensure!(self.owner.is_none(), "companion lost broker ownership");
            #[cfg(test)]
            if let Some(path) = &self.broker_file {
                // Private fixture witness: startup actually reached a successful
                // missing-owner observation, rather than expiring in handshake.
                fs::write(path.with_extension("missing-observed"), b"missing-owner")?;
            }
            return Ok(false);
        };
        ensure!(
            observed.pid == self.child.as_ref().context("companion child absent")?.id(),
            "foreign companion broker owner"
        );
        if let Some(expected) = &self.owner {
            ensure!(expected == &observed, "companion broker identity changed");
        } else {
            self.owner = Some(observed);
        }
        Ok(true)
    }
    pub(super) fn take_child(&mut self) -> Option<(Child, Option<u64>)> {
        self.child.take().map(|child| (child, self.start_ticks))
    }
}
impl Drop for Companion {
    fn drop(&mut self) {
        if let Some((child, start)) = self.take_child() {
            let report = finish_stop(begin_stop(child, start), Instant::now() + CLEANUP);
            if !report.reaped || report.error.is_some() {
                log::error!("Companion recovery required: {report:?}");
            }
        }
    }
}
