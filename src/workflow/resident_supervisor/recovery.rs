//! Installed, bounded stock restoration. A release-approved recovery profile is
//! required; none is inferred from live state or shipped before qualification.
use super::{
    open_at, same_file as metadata_equal, service, text, validate_absolute, validate_invocation,
    GroupBaseline, Lease, LeaseRecord, ProcessIdentity,
};
use anyhow::{bail, ensure, Context, Result};
use nix::libc;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub mod activation;

pub const LEASE_DIRECTORY: &str = "/run/reader-buddy-resident";
const PROFILE: &str = "/home/root/.local/lib/reader-buddy/resident-recovery.json";
const APP: &str = "/home/root/.local/lib/reader-buddy/reader-buddy";
const BUDDY: &str = "reader-buddy.service";
const HELPER: &str = "reader-buddy-stock-recovery.service";
const STOCK: &str = "xochitl.service";
const BUDDY_UNIT: &str = "/etc/systemd/system/reader-buddy.service";
const HELPER_UNIT: &str = "/etc/systemd/system/reader-buddy-stock-recovery.service";
const STOCK_EXE: &str = "/usr/bin/xochitl";
const STOCK_UNIT: &str = "/usr/lib/systemd/system/xochitl.service";
const STOCK_DROPIN: &str =
    "/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf";
const MOUNT: &str = "/sys/fs/cgroup/unified";
const MAX_BYTES: u64 = 128 * 1024 * 1024;
const RECOVERY_SECONDS: u64 = 120;

// The only permitted missing directory is the fixed final component below the
// protected /run parent. Missing/replaced/untrusted ancestors never mean absent.
fn installed_directory_identity() -> Result<Option<super::Identity>> {
    let parent = trusted_file(Path::new("/run"))?;
    let identity = super::Identity::of(&parent.metadata()?);
    ensure!(
        parent.metadata()?.is_dir() && identity.uid == 0 && identity.mode & 0o22 == 0,
        "untrusted runtime directory"
    );
    let observe = || -> Result<Option<super::Identity>> {
        match open_at(&parent, "reader-buddy-resident", true, false) {
            Ok(directory) => {
                let metadata = directory.metadata()?;
                ensure!(
                    metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o700,
                    "untrusted installed lease directory"
                );
                Ok(Some(super::Identity::of(&metadata)))
            }
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(error) => Err(error.into()),
        }
    };
    let present = observe()?;
    ensure!(
        super::Identity::of(&trusted_file(Path::new("/run"))?.metadata()?) == identity
            && observe()? == present,
        "runtime directory changed"
    );
    Ok(present)
}

/// Ordinary Linux startup must not bypass an occupied installed activation latch.
/// Absence permits only ordinary startup, not stock stop or native activation.
pub fn refuse_occupied_activation() -> Result<()> {
    if let Some(identity) = installed_directory_identity()? {
        require_unoccupied(Path::new(LEASE_DIRECTORY))?;
        ensure!(
            installed_directory_identity()? == Some(identity),
            "installed lease directory changed"
        );
    }
    Ok(())
}

pub(super) fn require_unoccupied(path: &Path) -> Result<()> {
    ensure!(
        Lease::reopen_if_present(path)
            .context("installed activation latch refuses ordinary startup")?
            .is_none(),
        "resident activation already consumed this boot; ordinary startup refused"
    );
    Ok(())
}

/// The same fixed occupied path is used by installed activation and recovery.
/// The installer creates it once per boot; service stop must never remove it.
/// A failed publication leaves the directory occupied and cannot authorize stop.
pub fn publish_installed(record: LeaseRecord) -> Result<Lease> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "installed activation requires root"
    );
    ensure!(
        record.group.unit == BUDDY && record.group.mount_path == Path::new(MOUNT),
        "installed activation context mismatch"
    );
    // Profile admission/stock/service bracketing must precede this call. Publishing
    // alone still grants neither stock-stop nor native-launch authority.
    Lease::publish(Path::new(LEASE_DIRECTORY), record)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    firmware: String,
    kernel: String,
    native_profile_sha256: [u8; 32],
    adapter_sha256: [u8; 32],
    app_sha256: [u8; 32],
    buddy_unit_sha256: [u8; 32],
    helper_unit_sha256: [u8; 32],
    stock_executable_sha256: [u8; 32],
    stock_unit_sha256: [u8; 32],
    stock_dropin_sha256: [u8; 32],
    #[serde(deserialize_with = "policy_fields")]
    buddy_policy: BTreeMap<String, String>,
    #[serde(deserialize_with = "policy_fields")]
    helper_policy: BTreeMap<String, String>,
    #[serde(deserialize_with = "policy_fields")]
    stock_policy: BTreeMap<String, String>,
}

fn policy_fields<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct Fields;
    impl<'de> serde::de::Visitor<'de> for Fields {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("unique bounded service policy fields")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut fields = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if !service::POLICY.contains(&key.as_str())
                    || value.len() > 4096
                    || value.bytes().any(|b| b.is_ascii_control())
                    || fields.insert(key, value).is_some()
                {
                    return Err(serde::de::Error::custom(
                        "invalid or duplicate recovery policy field",
                    ));
                }
            }
            if fields.len() != service::POLICY.len() {
                return Err(serde::de::Error::custom("missing recovery policy field"));
            }
            Ok(fields)
        }
    }
    deserializer.deserialize_map(Fields)
}

pub(super) struct Pinned {
    path: PathBuf,
    file: File,
    metadata: Metadata,
}
impl Pinned {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let file = trusted_file(path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == 0
                && metadata.mode() & 0o22 == 0
                && metadata.nlink() == 1,
            "recovery artifact must be a protected root-owned regular file"
        );
        Ok(Self {
            path: path.to_owned(),
            file,
            metadata,
        })
    }
    pub(super) fn current(&self) -> Result<()> {
        ensure!(
            metadata_equal(&self.metadata, &self.file.metadata()?)
                && metadata_equal(&self.metadata, &trusted_file(&self.path)?.metadata()?),
            "recovery artifact changed"
        );
        Ok(())
    }
    pub(super) fn bytes(&mut self, limit: u64, deadline: Instant) -> Result<Vec<u8>> {
        let bytes = bounded_bytes(&mut self.file, limit, deadline)?;
        ensure!(
            bytes.len() as u64 == self.metadata.len(),
            "recovery artifact length changed"
        );
        self.current()?;
        check_deadline(deadline)?;
        Ok(bytes)
    }
    pub(super) fn verified(path: &str, digest: [u8; 32], deadline: Instant) -> Result<Self> {
        ensure!(digest != [0; 32], "missing recovery artifact digest");
        let mut pinned = Self::open(Path::new(path))?;
        ensure!(
            hash_reader(&mut pinned.file, MAX_BYTES, deadline)? == digest,
            "recovery artifact digest mismatch: {path}"
        );
        pinned.current()?;
        Ok(pinned)
    }
}

pub(super) fn trusted_file(path: &Path) -> Result<File> {
    validate_absolute(path)?;
    let mut directory = File::open("/")?;
    let components: Vec<_> = path.components().skip(1).collect();
    for (index, component) in components.iter().enumerate() {
        let meta = directory.metadata()?;
        ensure!(
            meta.uid() == 0 && meta.mode() & 0o22 == 0,
            "untrusted recovery artifact parent"
        );
        let Component::Normal(name) = component else {
            bail!("invalid artifact path")
        };
        directory = open_at(
            &directory,
            name.to_str().context("non-UTF8 artifact path")?,
            index + 1 != components.len(),
            false,
        )?;
    }
    Ok(directory)
}
fn bounded_bytes(file: &mut impl Read, limit: u64, deadline: Instant) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 65536];
    loop {
        check_deadline(deadline)?;
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        ensure!(
            bytes.len() as u64 + count as u64 <= limit,
            "recovery input exceeds bound"
        );
        bytes.extend_from_slice(&chunk[..count]);
    }
    check_deadline(deadline)?;
    Ok(bytes)
}
fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn hash_reader(file: &mut impl Read, limit: u64, deadline: Instant) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut chunk = [0; 65536];
    let mut total = 0u64;
    loop {
        check_deadline(deadline)?;
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= limit, "recovery executable exceeds bound");
        hash.update(&chunk[..count]);
    }
    check_deadline(deadline)?;
    Ok(hash.finalize().into())
}
fn check_deadline(deadline: Instant) -> Result<()> {
    crate::workflow::resident_request::before(deadline, "stock recovery deadline expired")
}

struct Installation {
    profile: Profile,
    artifacts: Vec<Pinned>,
    digest: [u8; 32],
}
impl Installation {
    fn load(deadline: Instant) -> Result<Self> {
        let mut file = Pinned::open(Path::new(PROFILE))?;
        let bytes = file.bytes(32 * 1024, deadline)?;
        let digest = sha256(&bytes);
        let profile: Profile = serde_json::from_slice(&bytes)?;
        profile.validate_definition()?;
        ensure!(
            firmware_version(&text(Path::new("/etc/os-release"), 16 * 1024)?)? == profile.firmware
                && text(Path::new("/proc/sys/kernel/osrelease"), 256)?.trim() == profile.kernel,
            "recovery platform mismatch"
        );
        let mut artifacts = vec![file];
        for (path, hash) in [
            (APP, profile.app_sha256),
            (BUDDY_UNIT, profile.buddy_unit_sha256),
            (HELPER_UNIT, profile.helper_unit_sha256),
            (STOCK_EXE, profile.stock_executable_sha256),
            (STOCK_UNIT, profile.stock_unit_sha256),
            (STOCK_DROPIN, profile.stock_dropin_sha256),
        ] {
            artifacts.push(Pinned::verified(path, hash, deadline)?);
        }
        let installation = Self {
            profile,
            artifacts,
            digest,
        };
        installation.current(deadline)?;
        Ok(installation)
    }
    fn current(&self, deadline: Instant) -> Result<()> {
        check_deadline(deadline)?;
        for artifact in &self.artifacts {
            artifact.current()?;
        }
        check_deadline(deadline)
    }
}

struct Installed {
    profile: Profile,
    artifacts: Vec<Pinned>,
    helper_group: GroupBaseline,
    helper_invocation: String,
    helper_start: u64,
}
impl Installed {
    fn load(lease: &Lease, deadline: Instant) -> Result<Self> {
        let record = lease.record();
        ensure!(
            record.group.unit == BUDDY && record.group.mount_path == Path::new(MOUNT),
            "lease is not installed Buddy activation"
        );
        let installation = Installation::load(deadline)?;
        ensure!(
            installation.digest == record.recovery_profile_sha256,
            "original recovery profile mismatch"
        );
        installation.profile.validate(record)?;
        let Installation {
            profile, artifacts, ..
        } = installation;
        let before = service::observe(HELPER, deadline)?;
        let helper_group = GroupBaseline::capture(Path::new(MOUNT), HELPER)?;
        let after = service::observe(HELPER, deadline)?;
        ensure!(before == after, "helper changed during admission");
        helper_state(&before, helper_group.parent(), &profile.helper_policy)?;
        verify_process(
            helper_group.parent(),
            APP,
            profile.app_sha256,
            &[APP, "--resident-stock-recovery"],
            deadline,
        )?;
        let result = Self {
            profile,
            artifacts,
            helper_group,
            helper_invocation: before.invocation().to_owned(),
            helper_start: before.start_micros(),
        };
        result.current(lease, deadline)?;
        Ok(result)
    }
    fn current(&self, lease: &Lease, deadline: Instant) -> Result<()> {
        check_deadline(deadline)?;
        lease.current()?;
        for file in &self.artifacts {
            file.current()?;
        }
        let before = service::observe(HELPER, deadline)?;
        helper_state(
            &before,
            self.helper_group.parent(),
            &self.profile.helper_policy,
        )?;
        ensure!(
            before.invocation() == self.helper_invocation
                && before.start_micros() == self.helper_start,
            "helper invocation changed"
        );
        self.helper_group.current_caller()?;
        let after = service::observe(HELPER, deadline)?;
        ensure!(before == after, "helper changed during verification");
        check_deadline(deadline)
    }
    fn admit(&self, lease: &Lease, deadline: Instant) -> Result<()> {
        self.current(lease, deadline)?;
        let observation = service::observe_recovery(lease, deadline)?;
        match_policy(&observation.policy, &self.profile.buddy_policy)?;
        check_deadline(deadline)
    }
}

impl Profile {
    fn validate(&self, record: &LeaseRecord) -> Result<()> {
        self.validate_definition()?;
        ensure!(
            self.native_profile_sha256 == record.profile_sha256
                && self.adapter_sha256 == record.adapter_sha256
                && self.stock_executable_sha256 == record.stock.executable_sha256
                && self.stock_unit_sha256 == record.stock.unit_sha256
                && self.stock_dropin_sha256 == record.stock.dropin_sha256,
            "recovery profile differs from admitted activation"
        );
        Ok(())
    }
    fn validate_definition(&self) -> Result<()> {
        for digest in [
            self.native_profile_sha256,
            self.adapter_sha256,
            self.app_sha256,
            self.buddy_unit_sha256,
            self.helper_unit_sha256,
            self.stock_executable_sha256,
            self.stock_unit_sha256,
            self.stock_dropin_sha256,
        ] {
            ensure!(digest != [0; 32], "missing installed artifact digest");
        }
        ensure!(
            self.version == 1
                && self.firmware == "3.28.0.172"
                && self.kernel == "5.4.70-v1.6.3-rm11x",
            "unsupported recovery profile"
        );
        for (policy, fragment, dropins) in [
            (&self.buddy_policy, BUDDY_UNIT, ""),
            (&self.helper_policy, HELPER_UNIT, ""),
            (&self.stock_policy, STOCK_UNIT, STOCK_DROPIN),
        ] {
            ensure!(
                policy.len() == service::POLICY.len()
                    && service::POLICY.iter().all(|key| policy.contains_key(*key)),
                "incomplete recovery policy"
            );
            require(policy, "FragmentPath", fragment)?;
            require(policy, "DropInPaths", dropins)?;
            require(policy, "NeedDaemonReload", "no")?;
            require(policy, "KillMode", "control-group")?;
            require(policy, "Delegate", "no")?;
            require(policy, "SendSIGKILL", "yes")?;
            require(policy, "RestartForceExitStatus", "")?;
        }
        for policy in [&self.buddy_policy, &self.helper_policy] {
            require(policy, "Restart", "no")?;
            require(policy, "DefaultDependencies", "yes")?;
            ensure!(
                tokens(&policy["Conflicts"])?.contains(&"shutdown.target"),
                "missing shutdown conflict"
            );
        }
        require(&self.buddy_policy, "Type", "notify")?;
        require(&self.buddy_policy, "OnSuccess", HELPER)?;
        require(&self.buddy_policy, "OnFailure", HELPER)?;
        require(&self.buddy_policy, "WatchdogUSec", "2min")?;
        require(&self.helper_policy, "WatchdogUSec", "0")?;
        require(&self.helper_policy, "TimeoutStartUSec", "2min 10s")?;
        require(&self.stock_policy, "TimeoutStopUSec", "1min 30s")?;
        require(&self.helper_policy, "Type", "oneshot")?;
        require(&self.helper_policy, "OnSuccess", "")?;
        require(&self.helper_policy, "OnFailure", "")?;
        for key in ["Conflicts", "After"] {
            ensure!(
                tokens(&self.helper_policy[key])?.contains(&BUDDY),
                "helper ordering absent"
            );
        }
        Ok(())
    }
}
fn require(policy: &BTreeMap<String, String>, key: &str, value: &str) -> Result<()> {
    ensure!(
        policy.get(key).map(String::as_str) == Some(value),
        "recovery policy mismatch: {key}"
    );
    Ok(())
}
fn firmware_version(input: &str) -> Result<&str> {
    let mut values = input
        .lines()
        .filter_map(|line| line.strip_prefix("IMG_VERSION="));
    let raw = values.next().context("missing firmware image version")?;
    ensure!(values.next().is_none(), "duplicate firmware image version");
    let value = raw
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(raw);
    ensure!(
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit() || b == b'.'),
        "invalid firmware image version"
    );
    Ok(value)
}
fn tokens(value: &str) -> Result<Vec<&str>> {
    let mut result: Vec<_> = value.split_whitespace().collect();
    result.sort_unstable();
    ensure!(
        !result.windows(2).any(|w| w[0] == w[1]),
        "duplicate policy unit"
    );
    Ok(result)
}
fn match_policy(
    actual: &service::PolicySnapshot,
    expected: &BTreeMap<String, String>,
) -> Result<()> {
    for key in service::POLICY {
        let wanted = expected.get(*key).context("missing expected policy")?;
        let got = actual.property(key).context("missing actual policy")?;
        let same = if matches!(*key, "Conflicts" | "After" | "OnSuccess" | "OnFailure") {
            tokens(got)? == tokens(wanted)?
        } else {
            got == wanted
        };
        ensure!(same, "loaded recovery policy mismatch: {key}");
    }
    Ok(())
}
fn helper_state(
    s: &service::Snapshot,
    process: &ProcessIdentity,
    policy: &BTreeMap<String, String>,
) -> Result<()> {
    match_policy(&s.policy(), policy)?;
    for (key, value) in [
        ("Id", HELPER),
        ("LoadState", "loaded"),
        ("ActiveState", "activating"),
        ("SubState", "start"),
        ("ControlPID", "0"),
    ] {
        ensure!(
            s.property(key) == Some(value),
            "unexpected recovery helper state: {key}"
        );
    }
    ensure!(
        s.main_pid() == process.pid
            && s.property("ExecMainPID") == Some(process.pid.to_string().as_str())
            && s.start_micros() != 0
            && s.control_group() == format!("/system.slice/{HELPER}"),
        "helper process mismatch"
    );
    validate_invocation(s.invocation())?;
    Ok(())
}

pub(super) fn verify_process(
    process: &ProcessIdentity,
    executable: &str,
    digest: [u8; 32],
    argv: &[&str],
    deadline: Instant,
) -> Result<()> {
    ensure!(
        ProcessIdentity::read(process.pid)? == *process,
        "process incarnation changed"
    );
    let base = PathBuf::from(format!("/proc/{}", process.pid));
    ensure!(
        fs::read_link(base.join("exe"))? == Path::new(executable),
        "running executable path mismatch"
    );
    // This one intentional kernel magic-link open hashes the actual running
    // executable. Ordinary installation paths always use no-follow traversal.
    let mut file = File::open(base.join("exe"))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o22 == 0,
        "untrusted running executable"
    );
    ensure!(
        hash_reader(&mut file, MAX_BYTES, deadline)? == digest
            && metadata_equal(&metadata, &file.metadata()?),
        "running executable bytes mismatch"
    );
    let mut cmdline = File::open(base.join("cmdline"))?;
    let bytes = bounded_bytes(&mut cmdline, 16 * 1024, deadline)?;
    let expected: Vec<_> = argv
        .iter()
        .flat_map(|arg| arg.as_bytes().iter().copied().chain([0]))
        .collect();
    ensure!(bytes == expected, "running process argv mismatch");
    let env = bounded_bytes(&mut File::open(base.join("environ"))?, 64 * 1024, deadline)?;
    ensure!(
        !env.split(|b| *b == 0)
            .any(|entry| entry.starts_with(b"LD_") || entry.starts_with(b"DYLD_")),
        "loader override in running process"
    );
    ensure!(
        fs::read_link(base.join("exe"))? == Path::new(executable)
            && metadata_equal(&metadata, &File::open(base.join("exe"))?.metadata()?)
            && ProcessIdentity::read(process.pid)? == *process,
        "running process changed during verification"
    );
    check_deadline(deadline)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StockState {
    Ready,
    Startable,
    Pending,
    Stopping,
}
fn stock_state(s: &service::Snapshot) -> Result<StockState> {
    ensure!(
        s.property("Id") == Some(STOCK) && s.property("LoadState") == Some("loaded"),
        "stock unit not loaded"
    );
    let active = s.property("ActiveState").context("missing stock state")?;
    let sub = s.property("SubState").context("missing stock substate")?;
    let job = s.property("Job").context("missing stock job")?;
    // systemd 255.21's service state table. This permits waiting only: the
    // backend must bind it to the original stock before it can lead to a start.
    if active == "deactivating" {
        ensure!(
            matches!(
                sub,
                "stop"
                    | "stop-watchdog"
                    | "stop-sigterm"
                    | "stop-sigkill"
                    | "stop-post"
                    | "final-watchdog"
                    | "final-sigterm"
                    | "final-sigkill"
            ),
            "unknown stock shutdown substate"
        );
        ensure!(
            s.property("ControlPID") != Some("1"),
            "invalid stock control PID"
        );
        return Ok(StockState::Stopping);
    }
    ensure!(
        s.property("ControlPID") == Some("0"),
        "stock control process active"
    );
    if active == "active" && sub == "running" && job.is_empty() && s.main_pid() > 1 {
        ensure!(
            s.property("ExecMainPID") == Some(s.main_pid().to_string().as_str())
                && s.start_micros() != 0,
            "stock main-process evidence incomplete"
        );
        validate_invocation(s.invocation())?;
        ensure!(
            s.control_group() == format!("/system.slice/{STOCK}"),
            "stock service group mismatch"
        );
        return Ok(StockState::Ready);
    }
    if ((active == "inactive" && sub == "dead") || (active == "failed" && sub == "failed"))
        && job.is_empty()
        && s.main_pid() == 0
    {
        return Ok(StockState::Startable);
    }
    // Do not guess the kind of an existing PID1 job or replace it with a start.
    // Waiting is bounded; only the final independently verified state can succeed.
    if active == "activating" || !job.is_empty() {
        return Ok(StockState::Pending);
    }
    bail!("stock state does not permit restoration")
}

/// A verified original shutdown may settle before this helper's FIRST start.
/// The systemd timestamp is sampled here, never converted from proc start ticks.
#[derive(Debug, Eq, PartialEq)]
struct StoppingIdentity {
    invocation: String,
    pid: u32,
    start_micros: u64,
}
impl StoppingIdentity {
    fn capture(s: &service::Snapshot, original: &super::StockBaseline) -> Result<Self> {
        ensure!(
            stock_state(s)? == StockState::Stopping,
            "stock is not stopping"
        );
        ensure!(
            s.invocation() == original.invocation
                && s.property("ExecMainPID") == Some(original.process.pid.to_string().as_str())
                && (s.main_pid() == 0 || s.main_pid() == original.process.pid)
                && s.start_micros() != 0
                && s.control_group() == format!("/system.slice/{STOCK}"),
            "shutdown is not the original stock incarnation"
        );
        Ok(Self {
            invocation: s.invocation().to_owned(),
            pid: original.process.pid,
            start_micros: s.start_micros(),
        })
    }
    fn terminal(&self, s: &service::Snapshot) -> Result<()> {
        ensure!(
            stock_state(s)? == StockState::Startable,
            "stock shutdown is not terminal"
        );
        let retained = s.invocation() == self.invocation
            && s.property("ExecMainPID") == Some(self.pid.to_string().as_str())
            && s.start_micros() == self.start_micros;
        let cleared = s.invocation().is_empty()
            && s.property("ExecMainPID") == Some("0")
            && s.start_micros() == 0
            && s.property("ActiveState") == Some("inactive")
            && s.property("SubState") == Some("dead");
        ensure!(
            retained || cleared,
            "stock identity changed or partially cleared after shutdown"
        );
        Ok(())
    }
}

/// Service-level result only, never UI readiness or document-persistence proof.
#[derive(Debug)]
pub struct RestoredStock {
    pub process: ProcessIdentity,
    pub invocation: String,
    pub start_requested: bool,
}

#[derive(Debug)]
pub enum RecoveryOutcome {
    /// A no-op observation, not proof of stock readiness or past nonactivation.
    NoRecordedActivation,
    Restored(RestoredStock),
}

/// Fixed installed operation. No provider/config/Store startup and no arbitrary
/// unit/path overrides. Errors retain the activation record and any queued job.
pub fn run_installed() -> Result<RecoveryOutcome> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "stock recovery requires root"
    );
    let deadline = Instant::now() + Duration::from_secs(RECOVERY_SECONDS);
    let identity = installed_directory_identity()?.context("installed lease directory absent")?;
    let lease = Lease::reopen_if_present(Path::new(LEASE_DIRECTORY))?;
    ensure!(
        installed_directory_identity()? == Some(identity),
        "installed lease directory changed"
    );
    let Some(lease) = lease else {
        check_deadline(deadline)?;
        return Ok(RecoveryOutcome::NoRecordedActivation);
    };
    let installed = Installed::load(&lease, deadline)?;
    restore(&mut Native { installed, lease }, deadline).map(RecoveryOutcome::Restored)
}

// Private test seam runs the same sequencing; callers cannot inject a command,
// substitute a filesystem or manufacture an admitted installed profile.
trait Backend {
    fn original_stock(&self) -> &super::StockBaseline;
    fn admit(&mut self, deadline: Instant) -> Result<()>;
    fn stock(&mut self, deadline: Instant) -> Result<service::Snapshot>;
    fn start(&mut self, deadline: Instant) -> Result<()>;
    fn empty_stock_group(&mut self, deadline: Instant) -> Result<()>;
    fn verify(
        &mut self,
        snapshot: &service::Snapshot,
        deadline: Instant,
    ) -> Result<ProcessIdentity>;
    fn pace(&mut self, deadline: Instant) {
        thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
struct Native {
    installed: Installed,
    lease: Lease,
}
impl Backend for Native {
    fn original_stock(&self) -> &super::StockBaseline {
        &self.lease.record().stock
    }
    fn admit(&mut self, deadline: Instant) -> Result<()> {
        self.installed.admit(&self.lease, deadline)
    }
    fn stock(&mut self, deadline: Instant) -> Result<service::Snapshot> {
        let snapshot = service::observe(STOCK, deadline)?;
        match_policy(&snapshot.policy(), &self.installed.profile.stock_policy)?;
        Ok(snapshot)
    }
    fn start(&mut self, deadline: Instant) -> Result<()> {
        service::start_stock(deadline)
    }
    fn empty_stock_group(&mut self, deadline: Instant) -> Result<()> {
        let original = &self.lease.record().group.context;
        let before = super::GroupContext::read(Path::new(MOUNT))?;
        ensure!(&before.identity == original, "stock group context changed");
        let identity = empty_group(&before.slice)?;
        let after = super::GroupContext::read(Path::new(MOUNT))?;
        ensure!(
            &after.identity == original && empty_group(&after.slice)? == identity,
            "stock group changed during emptiness check"
        );
        check_deadline(deadline)
    }
    fn verify(
        &mut self,
        snapshot: &service::Snapshot,
        deadline: Instant,
    ) -> Result<ProcessIdentity> {
        let group = GroupBaseline::capture_process(Path::new(MOUNT), STOCK, snapshot.main_pid())?;
        verify_process(
            group.parent(),
            STOCK_EXE,
            self.installed.profile.stock_executable_sha256,
            &[STOCK_EXE, "--system"],
            deadline,
        )?;
        group.current_process()?;
        ensure!(
            ProcessIdentity::read(group.parent.pid)? == group.parent,
            "restored stock process changed"
        );
        let after = self.stock(deadline)?;
        ensure!(
            snapshot == &after && stock_state(&after)? == StockState::Ready,
            "stock changed during verification"
        );
        Ok(group.parent)
    }
}
fn restore(backend: &mut impl Backend, deadline: Instant) -> Result<RestoredStock> {
    let mut requested = false;
    let mut observed_pending = false;
    let mut stopping: Option<StoppingIdentity> = None;
    loop {
        check_deadline(deadline)?;
        backend.admit(deadline)?;
        let snapshot = backend.stock(deadline)?;
        match stock_state(&snapshot)? {
            StockState::Ready => {
                let process = backend.verify(&snapshot, deadline)?;
                backend.admit(deadline)?;
                // Admission can take time; do not report a now-replaced stock.
                ensure!(
                    backend.stock(deadline)? == snapshot,
                    "stock changed at recovery completion"
                );
                check_deadline(deadline)?;
                return Ok(RestoredStock {
                    process,
                    invocation: snapshot.invocation().to_owned(),
                    start_requested: requested,
                });
            }
            StockState::Startable => {
                ensure!(
                    !requested && !observed_pending,
                    "stock start did not reach running; no retry"
                );
                if let Some(identity) = &stopping {
                    identity.terminal(&snapshot)?;
                }
                backend.admit(deadline)?;
                backend.empty_stock_group(deadline)?;
                ensure!(
                    backend.stock(deadline)? == snapshot,
                    "stock changed before start request"
                );
                check_deadline(deadline)?;
                requested = true;
                backend.start(deadline)?;
            }
            StockState::Pending => {
                observed_pending = true;
            }
            StockState::Stopping => {
                let identity = StoppingIdentity::capture(&snapshot, backend.original_stock())?;
                if let Some(original) = &stopping {
                    ensure!(original == &identity, "stock shutdown identity changed");
                } else {
                    stopping = Some(identity);
                }
                // Neither requested nor observed_pending is ever reset here.
            }
        }
        backend.pace(deadline);
    }
}

fn empty_group(slice: &File) -> Result<Option<super::Identity>> {
    match open_at(slice, STOCK, true, false) {
        Ok(group) => {
            ensure!(!super::populated(&group)?, "stock group still populated");
            Ok(Some(super::Identity::of(&group.metadata()?)))
        }
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
