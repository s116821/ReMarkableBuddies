//! Development-only PID1 controller. Never selected by installed application code.
use super::{baseline::ServiceBaseline, *};
use crate::device::{
    request_input::{InputProgress, RequestInput},
    touch::TriggerCorner,
};
use crate::workflow::resident_supervisor::{
    open_at,
    recovery::{trusted_file, verify_process, Pinned},
    GroupBaseline, ProcessIdentity,
};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

const MOUNT: &str = "/sys/fs/cgroup/unified";
const DATA: &str = "/home/root/.local/share/remarkable/xochitl";
const STOCK: &str = "/usr/bin/xochitl";
const BUDDY: &str = "/home/root/.local/lib/reader-buddy/reader-buddy";
// The reviewed legacy service invokes this alias; /proc/PID/exe still resolves
// to BUDDY. Exact argv and canonical executable identity are separate checks.
const BUDDY_ARGV0: &str = "/opt/bin/reader-buddy";
const MAX_RECORD: u64 = 256 * 1024;

pub mod setup;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Native,
    Fixture,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Fault {
    None,
    ActorKill,
    ActorHang,
    AfterBuddyStop,
    RecoveryExit,
    RecoveryAfterIntent,
    RecoveryAfterStart,
    SourceMismatch,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExpectedService {
    baseline: String,
    executable_sha256: String,
}
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SourceEntry {
    path: String,
    sha256: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UnitFile {
    path: String,
    sha256: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    version: u32,
    nonce: String,
    kind: Kind,
    fault: Fault,
    boot: String,
    controller_sha256: String,
    observer_sha256: String,
    backup_sha256: String,
    stock: ExpectedService,
    buddy: ExpectedService,
    source: Vec<SourceEntry>,
    unit_files: Vec<UnitFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<setup::CapturePlan>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ownership {
    version: u32,
    candidate_sha256: String,
    actor: String,
    stock: String,
    buddy: String,
}

struct Context {
    nonce: String,
    stage: PathBuf,
    root: PathBuf,
    candidate: Candidate,
    digest: String,
    pins: Vec<Pinned>,
    own: Option<GroupBaseline>,
    own_snapshot: Option<Snapshot>,
    root_identity: Option<(u64, u64)>,
}

fn check(deadline: Instant) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "bootstrap control deadline expired"
    );
    Ok(())
}
fn nonce(value: &str) -> Result<()> {
    ensure!(
        value.len() == 32
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid nonce"
    );
    Ok(())
}
fn digest(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid SHA256"
    );
    let mut result = [0; 32];
    for (i, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
    }
    ensure!(result != [0; 32], "empty SHA256");
    Ok(result)
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn stage(nonce: &str) -> PathBuf {
    PathBuf::from(format!("/home/root/rsdk-bootstrap-{nonce}"))
}
fn root(nonce: &str) -> PathBuf {
    PathBuf::from(format!("/run/rsdk-control-{nonce}"))
}
fn owned(nonce: &str, role: &str) -> String {
    format!("rsdk-bootstrap-{nonce}-{role}.service")
}
fn target(kind: Kind, nonce: &str, role: &str) -> String {
    match (kind, role) {
        (Kind::Native, "stock") => "xochitl.service".into(),
        (Kind::Native, "buddy") => "reader-buddy.service".into(),
        _ => owned(nonce, role),
    }
}
fn executable(kind: Kind, nonce: &str, role: &str) -> PathBuf {
    match (kind, role) {
        (Kind::Native, "stock") => STOCK.into(),
        (Kind::Native, "buddy") => BUDDY.into(),
        _ => stage(nonce).join("resident_bootstrap_control"),
    }
}
fn argv(kind: Kind, nonce: &str, role: &str) -> Vec<String> {
    let executable = executable(kind, nonce, role).to_string_lossy().into_owned();
    match (kind, role) {
        (Kind::Native, "stock") => vec![executable, "--system".into()],
        (Kind::Native, "buddy") => vec![BUDDY_ARGV0.into()],
        _ => vec![executable, format!("standin-{role}"), nonce.into()],
    }
}

fn verify_service_process(
    kind: Kind,
    nonce: &str,
    role: &str,
    baseline: &ServiceBaseline,
    hash: [u8; 32],
    deadline: Instant,
) -> Result<()> {
    let path = executable(kind, nonce, role);
    let args = argv(kind, nonce, role);
    verify_process(
        baseline.group().parent(),
        path.to_str().context("invalid service executable path")?,
        hash,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        deadline,
    )
    .with_context(|| format!("{role} process verification failed"))
}

fn unit_text(nonce: &str, kind: Kind, role: &str) -> Result<String> {
    let actor = owned(nonce, "actor");
    let recovery = owned(nonce, "recovery");
    let binary = stage(nonce).join("resident_bootstrap_control");
    let (unit, service) = match role {
        "actor" => (
            format!("OnSuccess={recovery}\nOnFailure={recovery}\nOnSuccessJobMode=fail\nOnFailureJobMode=fail\n"),
            format!(
                "Type=exec\nRuntimeMaxSec={}s\nTimeoutStartSec=5s\n",
                if kind == Kind::Native { 310 } else { 12 }
            ),
        ),
        "recovery" => (
            format!("Conflicts={actor}\nAfter={actor}\n"),
            "Type=oneshot\nTimeoutStartSec=130s\n".into(),
        ),
        "stock" | "buddy" if kind == Kind::Fixture => (
            String::new(),
            "Type=exec\nRuntimeMaxSec=600s\nTimeoutStartSec=5s\n".into(),
        ),
        _ => anyhow::bail!("invalid diagnostic unit role"),
    };
    let command = if matches!(role, "stock" | "buddy") {
        format!("standin-{role}")
    } else {
        role.into()
    };
    Ok(format!("[Unit]\nDescription=Bounded resident bootstrap {role} {nonce}\n{unit}[Service]\n{service}ExecStart={} {command} {nonce}\nRestart=no\nWatchdogSec=0\nKillMode=control-group\nSendSIGKILL=yes\nTimeoutStopSec=5s\nUMask=0077\nLimitCORE=0\nLimitFSIZE=67108864\n", binary.display()))
}

// systemd 255 initializes watchdog_original_usec to USEC_INFINITY and copies
// the configured watchdog_usec only when starting the service. Thus an idle,
// never-started helper reports infinity even with the pinned WatchdogSec=0.
// This exception requires exact unit bytes and no drop-ins (checked by units),
// and is never accepted for the running actor or a recovery invocation.
fn diagnostic_watchdog_disabled(s: &Snapshot, unstarted_helper: bool) -> bool {
    s.property("WatchdogUSec") == Some("0") || (unstarted_helper && s.unstarted_watchdog())
}

// Only the recovery helper may use the original actor's independently proven
// stopped/absent observation. The unit's raw snapshot must still be unchanged
// after that proof; no running helper or ownership-free path gets this exception.
fn diagnostic_actor_watchdog(
    snapshot: &Snapshot,
    role: &str,
    name: &str,
    original_actor: Option<&ServiceBaseline>,
    deadline: Instant,
) -> Result<bool> {
    if role != "recovery" || name != "actor" || !snapshot.uninitialized_watchdog() {
        return Ok(false);
    }
    let Some(actor) = original_actor else {
        return Ok(false);
    };
    ensure!(
        actor.original().property("WatchdogUSec") == Some("0")
            && snapshot.property("Id") == Some(actor.group().unit()),
        "diagnostic original actor watchdog identity changed"
    );
    cleared_actor_watchdog(
        snapshot,
        || actor.stopped(deadline),
        || observe(actor.group().unit(), deadline),
    )
}

fn cleared_actor_watchdog(
    snapshot: &Snapshot,
    stopped: impl FnOnce() -> Result<Option<RecoveryObservation>>,
    resample: impl FnOnce() -> Result<Snapshot>,
) -> Result<bool> {
    if !snapshot.uninitialized_watchdog() {
        return Ok(false);
    }
    let Some(terminal) = stopped()? else {
        return Ok(false);
    };
    Ok(terminal.identity == TerminalIdentity::Cleared
        && terminal.group == GroupObservation::OriginalAbsent
        && terminal.watchdog == WatchdogObservation::ClearedUninitialized
        && terminal.policy.matches(&snapshot.policy())
        && resample()?.same_observation(snapshot))
}

fn unit_paths(kind: Kind, nonce: &str) -> Vec<String> {
    if kind == Kind::Native {
        vec![
            "/usr/lib/systemd/system/xochitl.service".into(),
            "/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf".into(),
            "/etc/systemd/system/reader-buddy.service".into(),
        ]
    } else {
        ["stock", "buddy"]
            .iter()
            .map(|role| format!("/run/systemd/system/{}", owned(nonce, role)))
            .collect()
    }
}

fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    ensure!(
        !name.contains('/') && !name.is_empty(),
        "invalid record leaf"
    );
    let directory = private_root(root)?;
    let mut file = open_at(&directory, name, false, true)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(())
}
fn private_root(root: &Path) -> Result<File> {
    let file = trusted_file(root)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_dir() && meta.uid() == 0 && meta.mode() & 0o7777 == 0o700,
        "untrusted control root"
    );
    Ok(file)
}
fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn read_pin(path: &Path, deadline: Instant) -> Result<(Pinned, Vec<u8>)> {
    let mut pin = Pinned::open(path)?;
    let bytes = pin.bytes(MAX_RECORD, deadline)?;
    Ok((pin, bytes))
}
fn source_root(kind: Kind, nonce: &str) -> PathBuf {
    if kind == Kind::Native {
        DATA.into()
    } else {
        stage(nonce).join("fixture-data")
    }
}
fn inventory(base: &Path, deadline: Instant) -> Result<Vec<SourceEntry>> {
    fn walk(
        base: &Path,
        path: &Path,
        depth: usize,
        result: &mut Vec<SourceEntry>,
        total: &mut u64,
        deadline: Instant,
    ) -> Result<()> {
        check(deadline)?;
        ensure!(
            depth <= 8 && result.len() < 512,
            "source inventory exceeds bound"
        );
        let meta = trusted_file(path)?.metadata()?;
        ensure!(
            meta.uid() == 0 && meta.mode() & 0o22 == 0,
            "untrusted source entry"
        );
        let relative = path
            .strip_prefix(base)?
            .to_str()
            .context("source path is not UTF-8")?
            .to_owned();
        if meta.is_dir() {
            if depth != 0 {
                result.push(SourceEntry {
                    path: relative,
                    sha256: None,
                });
            }
            for entry in fs::read_dir(path)? {
                walk(base, &entry?.path(), depth + 1, result, total, deadline)?;
            }
        } else {
            ensure!(
                meta.is_file() && meta.nlink() == 1 && meta.len() <= 64 * 1024 * 1024,
                "unsupported source file"
            );
            *total = total
                .checked_add(meta.len())
                .context("source size overflow")?;
            ensure!(*total <= 128 * 1024 * 1024, "source bytes exceed bound");
            let mut pin = Pinned::open(path)?;
            let bytes = pin.bytes(64 * 1024 * 1024, deadline)?;
            result.push(SourceEntry {
                path: relative,
                sha256: Some(sha(&bytes)),
            });
        }
        check(deadline)
    }
    let mut result = Vec::new();
    walk(base, base, 0, &mut result, &mut 0, deadline)?;
    result.sort_by(|a, b| a.path.cmp(&b.path));
    ensure!(!result.is_empty(), "empty source manifest");
    Ok(result)
}

impl Context {
    fn load(nonce: &str, deadline: Instant) -> Result<Self> {
        let stage = stage(nonce);
        let root = root(nonce);
        let meta = trusted_file(&stage)?.metadata()?;
        ensure!(
            meta.is_dir() && meta.uid() == 0 && meta.mode() & 0o7777 == 0o700,
            "untrusted staging root"
        );
        let (pin, bytes) = read_pin(&stage.join("candidate.json"), deadline)?;
        let candidate: Candidate = serde_json::from_slice(&bytes)?;
        ensure!(
            ((candidate.version == 1 && candidate.capture.is_none())
                || (candidate.version == 2
                    && candidate.kind == Kind::Native
                    && candidate.capture.is_some()))
                && candidate.nonce == nonce,
            "candidate identity mismatch"
        );
        if let Some(plan) = &candidate.capture {
            plan.facts()?;
        }
        ensure!(
            candidate.kind == Kind::Fixture || candidate.fault == Fault::None,
            "native fault injection forbidden"
        );
        if candidate.kind == Kind::Native {
            ensure!(
                fs::read_to_string("/proc/sys/kernel/osrelease")?.trim() == "5.4.70-v1.6.3-rm11x",
                "unqualified native kernel"
            );
        }
        ensure!(
            candidate.source.len() <= 512 && !candidate.source.is_empty(),
            "invalid source manifest"
        );
        digest(&candidate.backup_sha256)?;
        ensure!(
            candidate.boot == fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim(),
            "candidate boot changed"
        );
        let control = stage.join("resident_bootstrap_control");
        let control_pin = Pinned::verified(
            control.to_str().context("invalid controller path")?,
            digest(&candidate.controller_sha256)?,
            deadline,
        )?;
        verify_process(
            &ProcessIdentity::current()?,
            control.to_str().unwrap(),
            digest(&candidate.controller_sha256)?,
            &std::env::args()
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            deadline,
        )?;
        let mut result = Self {
            nonce: nonce.into(),
            stage,
            root,
            digest: sha(&bytes),
            candidate,
            pins: vec![pin, control_pin],
            own: None,
            own_snapshot: None,
            root_identity: None,
        };
        let paths = unit_paths(result.candidate.kind, nonce);
        ensure!(
            result
                .candidate
                .unit_files
                .iter()
                .map(|v| &v.path)
                .eq(paths.iter()),
            "unexpected unit file inventory"
        );
        for file in &result.candidate.unit_files {
            result.pins.push(Pinned::verified(
                &file.path,
                digest(&file.sha256)?,
                deadline,
            )?);
        }
        for role in ["stock", "buddy"] {
            let expected = result.expected(role);
            let baseline = ServiceBaseline::decode(expected.baseline.as_bytes())?;
            ensure!(
                baseline.group().unit() == target(result.candidate.kind, nonce, role),
                "candidate service mismatch"
            );
            let path = executable(result.candidate.kind, nonce, role);
            let hash = digest(&expected.executable_sha256)?;
            result
                .pins
                .push(Pinned::verified(path.to_str().unwrap(), hash, deadline)?);
        }
        if result.candidate.kind == Kind::Native {
            result.pins.push(Pinned::verified(
                result
                    .stage
                    .join("resident_bootstrap_observer")
                    .to_str()
                    .unwrap(),
                digest(&result.candidate.observer_sha256)?,
                deadline,
            )?);
        } else {
            ensure!(
                result.candidate.observer_sha256 == result.candidate.controller_sha256,
                "fixture observer must be controller stand-in"
            );
        }
        Ok(result)
    }
    fn expected(&self, role: &str) -> &ExpectedService {
        if role == "stock" {
            &self.candidate.stock
        } else {
            &self.candidate.buddy
        }
    }
    fn current(&self, deadline: Instant) -> Result<()> {
        check(deadline)?;
        if let Some(identity) = self.root_identity {
            let meta = private_root(&self.root)?.metadata()?;
            ensure!(
                (meta.dev(), meta.ino()) == identity,
                "control root replaced"
            );
        }
        for pin in &self.pins {
            pin.current()?;
        }
        ensure!(
            self.candidate.boot == fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim(),
            "boot changed"
        );
        if let (Some(group), Some(snapshot)) = (&self.own, &self.own_snapshot) {
            let before = observe(group.unit(), deadline)?;
            ensure!(&before == snapshot, "controller service changed");
            group.current_caller()?;
            ensure!(
                observe(group.unit(), deadline)? == before,
                "controller changed across ownership check"
            );
        }
        check(deadline)
    }
    fn bind_root(&mut self) -> Result<()> {
        let meta = private_root(&self.root)?.metadata()?;
        self.root_identity = Some((meta.dev(), meta.ino()));
        Ok(())
    }
    fn preserve(&self, deadline: Instant) -> Result<()> {
        ensure!(
            inventory(&source_root(self.candidate.kind, &self.nonce), deadline)?
                == self.candidate.source,
            "source manifest changed"
        );
        Ok(())
    }
    fn units(
        &mut self,
        role: &str,
        original_actor: Option<&ServiceBaseline>,
        deadline: Instant,
    ) -> Result<()> {
        for name in ["actor", "recovery"] {
            let unit = owned(&self.nonce, name);
            let path = PathBuf::from(format!("/run/systemd/system/{unit}"));
            let (pin, bytes) = read_pin(&path, deadline)?;
            ensure!(
                bytes == unit_text(&self.nonce, self.candidate.kind, name)?.as_bytes(),
                "unit bytes differ from reviewed policy"
            );
            self.pins.push(pin);
            let s = observe(&unit, deadline)?;
            for (key, value) in [
                ("LoadState", "loaded"),
                ("FragmentPath", path.to_str().unwrap()),
                ("DropInPaths", ""),
                ("NeedDaemonReload", "no"),
                ("Restart", "no"),
                ("RestartForceExitStatus", ""),
                ("KillMode", "control-group"),
                ("Delegate", "no"),
                ("SendSIGKILL", "yes"),
                ("DefaultDependencies", "yes"),
                ("TimeoutStopUSec", "5s"),
            ] {
                ensure!(
                    s.property(key) == Some(value),
                    "unsafe diagnostic unit {name}: {key}"
                );
            }
            ensure!(
                diagnostic_watchdog_disabled(&s, name == "recovery" && role == "actor")
                    || diagnostic_actor_watchdog(&s, role, name, original_actor, deadline)?,
                "unsafe diagnostic unit {name}: WatchdogUSec={:?}",
                s.property("WatchdogUSec")
            );
            ensure!(
                s.property("Conflicts")
                    .unwrap()
                    .split_whitespace()
                    .any(|v| v == "shutdown.target"),
                "shutdown conflict absent"
            );
            if name == "actor" {
                for (key, value) in [
                    ("Type", "exec"),
                    ("OnSuccess", owned(&self.nonce, "recovery").as_str()),
                    ("OnFailure", owned(&self.nonce, "recovery").as_str()),
                    ("TimeoutStartUSec", "5s"),
                ] {
                    ensure!(
                        s.property(key) == Some(value),
                        "actor policy mismatch: {key}"
                    );
                }
            } else {
                for (key, value) in [
                    ("Type", "oneshot"),
                    ("OnSuccess", ""),
                    ("OnFailure", ""),
                    ("TimeoutStartUSec", "2min 10s"),
                ] {
                    ensure!(
                        s.property(key) == Some(value),
                        "recovery policy mismatch: {key}"
                    );
                }
                for key in ["After", "Conflicts"] {
                    ensure!(
                        s.property(key)
                            .unwrap()
                            .split_whitespace()
                            .any(|v| v == owned(&self.nonce, "actor")),
                        "recovery ordering absent"
                    );
                }
                if role == "actor" {
                    for (key, value) in [
                        ("ActiveState", "inactive"),
                        ("SubState", "dead"),
                        ("MainPID", "0"),
                        ("ControlPID", "0"),
                        ("Job", ""),
                    ] {
                        ensure!(s.property(key) == Some(value), "recovery is not idle");
                    }
                }
            }
            let extra = extra(&unit, deadline)?;
            if name == "actor" {
                diagnostic_trigger_modes(&extra)?;
            }
            let runtime = if name == "actor" {
                if self.candidate.kind == Kind::Native {
                    "5min 10s"
                } else {
                    "12s"
                }
            } else {
                "infinity"
            };
            ensure!(
                extra["RuntimeMaxUSec"] == runtime,
                "independent runtime bound mismatch"
            );
            ensure!(
                extra["Before"]
                    .split_whitespace()
                    .any(|v| v == "shutdown.target"),
                "shutdown ordering absent"
            );
            let exec = format!(
                "path={} ; argv[]={} {name} {} ; ignore_errors=no ;",
                self.stage.join("resident_bootstrap_control").display(),
                self.stage.join("resident_bootstrap_control").display(),
                self.nonce
            );
            ensure!(
                extra["ExecStart"].matches("path=").count() == 1
                    && extra["ExecStart"].contains(&exec),
                "loaded executable differs"
            );
        }
        let unit = owned(&self.nonce, role);
        let before = observe(&unit, deadline)?;
        let group = GroupBaseline::capture(Path::new(MOUNT), &unit)?;
        ensure!(
            before.main_pid() == std::process::id() && before.property("ControlPID") == Some("0"),
            "controller is not service main process"
        );
        let expected = if role == "actor" {
            ("active", "running")
        } else {
            ("activating", "start")
        };
        ensure!(
            before.property("ActiveState") == Some(expected.0)
                && before.property("SubState") == Some(expected.1),
            "controller phase mismatch"
        );
        ensure!(
            observe(&unit, deadline)? == before,
            "controller changed during admission"
        );
        self.own = Some(group);
        self.own_snapshot = Some(before);
        self.current(deadline)
    }
    fn verified_running(&self, role: &str, deadline: Instant) -> Result<ServiceBaseline> {
        let unit = target(self.candidate.kind, &self.nonce, role);
        let baseline = ServiceBaseline::capture(Path::new(MOUNT), &unit, deadline)?;
        verify_service_process(
            self.candidate.kind,
            &self.nonce,
            role,
            &baseline,
            digest(&self.expected(role).executable_sha256)?,
            deadline,
        )?;
        let expected = ServiceBaseline::decode(self.expected(role).baseline.as_bytes())?;
        ensure!(
            baseline
                .original()
                .policy()
                .matches(&expected.original().policy()),
            "restored service policy changed"
        );
        baseline.current(deadline)?;
        Ok(baseline)
    }
    fn intent(&self, name: &str) -> Result<()> {
        write_new(&self.root, name, self.nonce.as_bytes())
    }
}

fn diagnostic_trigger_modes(fields: &BTreeMap<String, String>) -> Result<()> {
    for key in ["OnSuccessJobMode", "OnFailureJobMode"] {
        ensure!(
            fields.get(key).map(String::as_str) == Some("fail"),
            "diagnostic trigger may replace conflicting jobs: {key}"
        );
    }
    Ok(())
}

fn extra(unit: &str, deadline: Instant) -> Result<BTreeMap<String, String>> {
    let mut command = Command::new("/bin/systemctl");
    command.args([
        "--system",
        "--no-pager",
        "--no-ask-password",
        "--all",
        "show",
        "--property=RuntimeMaxUSec",
        "--property=ExecStart",
        "--property=Before",
        "--property=OnSuccessJobMode",
        "--property=OnFailureJobMode",
        "--",
        unit,
    ]);
    let bytes = run(command, deadline)?;
    let input = std::str::from_utf8(&bytes)?;
    let mut fields = BTreeMap::new();
    for line in input.lines() {
        let (k, v) = line.split_once('=').context("malformed extra policy")?;
        ensure!(
            [
                "RuntimeMaxUSec",
                "ExecStart",
                "Before",
                "OnSuccessJobMode",
                "OnFailureJobMode"
            ]
            .contains(&k)
                && fields.insert(k.into(), v.into()).is_none(),
            "unexpected extra policy"
        );
    }
    ensure!(fields.len() == 5, "incomplete extra policy");
    Ok(fields)
}
fn request(action: &str, unit: &str, deadline: Instant) -> Result<()> {
    ensure!(matches!(action, "start" | "stop"), "invalid action");
    validate_unit(unit)?;
    let mut command = Command::new("/bin/systemctl");
    command.args([
        "--system",
        "--no-pager",
        "--no-ask-password",
        "--no-block",
        "--job-mode=fail",
        action,
        "--",
        unit,
    ]);
    ensure!(
        run(command, deadline)?.is_empty(),
        "unexpected service request output"
    );
    Ok(())
}
fn quiet(input: &mut Option<RequestInput>) -> Result<()> {
    if let Some(input) = input {
        ensure!(
            input.poll()? == InputProgress::Quiet,
            "bootstrap canceled by input"
        );
    }
    Ok(())
}
fn pace(deadline: Instant) {
    thread::sleep(
        Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
    );
}

/// Development diagnostic only: a fresh observation admission after Buddy's
/// proved stop is distinct from the ended handoff admission. Input during that
/// interval is not covered; this must never implement installed request renewal.
fn stop_with_input(
    input: &mut Option<RequestInput>,
    submit: impl FnOnce() -> Result<()>,
    mut terminal: impl FnMut() -> Result<bool>,
    new_admission: bool,
    create: impl FnOnce() -> Result<RequestInput>,
    deadline: Instant,
) -> Result<()> {
    quiet(input)?;
    check(deadline)?;
    submit()?;
    if new_admission {
        *input = None;
    }
    loop {
        check(deadline)?;
        quiet(input)?;
        if terminal()? {
            break;
        }
        pace(deadline);
    }
    check(deadline)?;
    if new_admission {
        *input = Some(create()?);
        quiet(input)?;
        check(deadline)?;
    }
    Ok(())
}

fn stopped(baseline: &ServiceBaseline, deadline: Instant) -> Result<()> {
    ensure!(
        baseline.stopped(deadline)?.is_some(),
        "original service still running/stopping"
    );
    Ok(())
}

fn actor(mut context: Context, deadline: Instant) -> Result<()> {
    fs::DirBuilder::new().mode(0o700).create(&context.root)?;
    context.bind_root()?;
    context.units("actor", None, deadline)?;
    let mut input = if context.candidate.kind == Kind::Native {
        Some(RequestInput::new(TriggerCorner::LowerLeft, None)?)
    } else {
        None
    };
    quiet(&mut input)?;
    context.preserve(deadline)?;
    let mut baselines = Vec::new();
    let mut selected_records = Vec::new();
    for role in ["stock", "buddy"] {
        let baseline = context.verified_running(role, deadline)?;
        selected_records.push(baseline.selected_record(&context.expected(role).baseline)?);
        baselines.push(baseline);
    }
    let actor =
        ServiceBaseline::capture(Path::new(MOUNT), &owned(&context.nonce, "actor"), deadline)?;
    let ownership = Ownership {
        version: 1,
        candidate_sha256: context.digest.clone(),
        actor: String::from_utf8(actor.encode()?)?,
        stock: selected_records[0].clone(),
        buddy: selected_records[1].clone(),
    };
    write_new(
        &context.root,
        "ownership.json",
        &serde_json::to_vec(&ownership)?,
    )?;
    let (pin, _) = read_pin(&context.root.join("ownership.json"), deadline)?;
    context.pins.push(pin);
    for (role, index) in [("buddy", 1), ("stock", 0)] {
        context.current(deadline)?;
        quiet(&mut input)?;
        baselines[index].current(deadline)?;
        context.intent(&format!("{role}.stop-intent"))?;
        context.current(deadline)?;
        baselines[index].current(deadline)?;
        let new_admission = role == "buddy" && context.candidate.kind == Kind::Native;
        stop_with_input(
            &mut input,
            || {
                request("stop", baselines[index].group().unit(), deadline)?;
                if new_admission {
                    context.intent("buddy.input-admission-ended")?;
                }
                Ok(())
            },
            || {
                context.current(deadline)?;
                Ok(baselines[index].stopped(deadline)?.is_some())
            },
            new_admission,
            || RequestInput::new(TriggerCorner::LowerLeft, None),
            deadline,
        )?;
        if new_admission {
            context.intent("observation.input-admission-started")?;
            context.preserve(deadline)?;
        }
        if role == "buddy" && context.candidate.fault == Fault::AfterBuddyStop {
            anyhow::bail!("fixture partial-stop fault");
        }
    }
    context.preserve(deadline)?;
    quiet(&mut input)?;
    for baseline in &baselines {
        stopped(baseline, deadline)?;
    }
    context.current(deadline)?;
    context.intent("observer.launch-intent")?;
    let mut command = Command::new(if context.candidate.kind == Kind::Native {
        context.stage.join("resident_bootstrap_observer")
    } else {
        context.stage.join("resident_bootstrap_control")
    });
    if context.candidate.kind == Kind::Fixture {
        command.arg("child");
    }
    command
        .arg(&context.nonce)
        .env_clear()
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    let directory = private_root(&context.root)?;
    let stdout = open_at(&directory, "observer.stdout", false, true)?;
    let stderr = open_at(&directory, "observer.stderr", false, true)?;
    command.stdout(stdout).stderr(stderr);
    context.current(deadline)?;
    for baseline in &baselines {
        stopped(baseline, deadline)?;
    }
    quiet(&mut input)?;
    check(deadline)?;
    let mut capture_mailbox = if context.candidate.capture.is_some() {
        let mailbox = setup::Mailbox::open(&context.root)?;
        mailbox.empty()?;
        Some(mailbox)
    } else {
        None
    };
    let mut child = command.spawn()?;
    if let Some(mailbox) = &mut capture_mailbox {
        let setup_deadline = deadline.min(Instant::now() + Duration::from_secs(120));
        let binding = loop {
            context.current(setup_deadline)?;
            quiet(&mut input)?;
            for baseline in &baselines {
                stopped(baseline, setup_deadline)?;
            }
            ensure!(child.try_wait()?.is_none(), "observer exited before setup");
            if let Some(binding) = mailbox.receive("observer-ended", setup_deadline)? {
                binding.current(&context, child.id())?;
                break binding;
            }
            pace(setup_deadline);
        };
        setup::actor(
            mailbox,
            &binding,
            &mut input,
            || RequestInput::new(TriggerCorner::LowerLeft, None),
            setup_deadline,
            &mut || {
                context.current(setup_deadline)?;
                binding.current(&context, child.id())?;
                for baseline in &baselines {
                    stopped(baseline, setup_deadline)?;
                }
                Ok(())
            },
        )?;
    }
    if matches!(context.candidate.fault, Fault::ActorKill | Fault::ActorHang) {
        let ready = context.root.join("fixture-child-ready");
        let complete = context.root.join("fixture-child-ready-complete");
        loop {
            context.current(deadline)?;
            ensure!(
                child.try_wait()?.is_none(),
                "fixture child exited before fault"
            );
            if present(&complete)? {
                let (_, completion) = read_pin(&complete, deadline)?;
                ensure!(completion.is_empty(), "invalid child completion marker");
                let (_, bytes) = read_pin(&ready, deadline)?;
                let identity: ProcessIdentity = serde_json::from_slice(&bytes)?;
                ensure!(
                    identity.pid == child.id(),
                    "fixture child identity mismatch"
                );
                let group = GroupBaseline::capture_process(
                    Path::new(MOUNT),
                    &owned(&context.nonce, "actor"),
                    child.id(),
                )?;
                ensure!(group.parent() == &identity, "fixture child changed");
                write_new(
                    &context.root,
                    "fixture-child-observed",
                    &serde_json::to_vec(&group)?,
                )?;
                break;
            }
            pace(deadline);
        }
    }
    if context.candidate.fault == Fault::ActorKill {
        unsafe { libc::raise(libc::SIGKILL) };
        anyhow::bail!("SIGKILL did not terminate fixture");
    }
    if context.candidate.fault == Fault::ActorHang {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    // PID1's control-group termination remains responsible for descendants after
    // any controller error, including failure before the child identity is saved.
    loop {
        context.current(deadline)?;
        quiet(&mut input)?;
        if let Some(mailbox) = &capture_mailbox {
            mailbox.current()?;
        }
        for baseline in &baselines {
            stopped(baseline, deadline)?;
        }
        if let Some(status) = child.try_wait()? {
            write_new(
                &context.root,
                "observer.exit",
                status.to_string().as_bytes(),
            )?;
            if context.candidate.fault == Fault::SourceMismatch {
                ensure!(
                    context.candidate.kind == Kind::Fixture && status.success(),
                    "source mismatch fault requires a successful fixture observer"
                );
                // Add only a new leaf in this candidate's disposable source.
                // The native document root can never be selected by this fault.
                write_new(
                    &context.stage.join("fixture-data"),
                    "fixture-added.txt",
                    context.nonce.as_bytes(),
                )?;
                context.intent("fixture-source-mismatch")?;
            }
            context.preserve(deadline)?;
            ensure!(status.success(), "observer refused/failed: {status}");
            return Ok(());
        }
        pace(deadline);
    }
}

fn restore(
    context: &Context,
    baseline: &ServiceBaseline,
    actor: &ServiceBaseline,
    role: &str,
    attempt: &str,
    deadline: Instant,
) -> Result<()> {
    let durable_intent = context.root.join(format!("{role}.start-intent"));
    let mut prior = if present(&durable_intent)? {
        let (pin, bytes) = read_pin(&durable_intent, deadline)?;
        ensure!(
            bytes == context.nonce.as_bytes(),
            "invalid durable start intent"
        );
        Some(pin)
    } else {
        None
    };
    let mut requested = prior.is_some();
    let mut pending = false;
    let stop_intent = context.root.join(format!("{role}.stop-intent"));
    let admitted = present(&stop_intent)?;
    loop {
        context.current(deadline)?;
        if let Some(pin) = &prior {
            pin.current()?;
        }
        stopped(actor, deadline)?;
        let s = observe(baseline.group().unit(), deadline)?;
        if !admitted {
            baseline.current(deadline)?;
        }
        if !s.policy().matches(&baseline.original().policy()) {
            let terminal = baseline
                .stopped(deadline)?
                .context("recovery target policy changed before terminal state")?;
            ensure!(
                terminal.watchdog == WatchdogObservation::ClearedUninitialized
                    && terminal.policy.matches(&s.policy()),
                "recovery target policy changed"
            );
        }
        if s.property("ActiveState") == Some("active")
            && s.property("SubState") == Some("running")
            && s.property("Job") == Some("")
            && s.property("ControlPID") == Some("0")
        {
            let running = context.verified_running(role, deadline)?;
            ensure!(
                running.original().same_observation(&s),
                "service changed at restored verification"
            );
            context.current(deadline)?;
            stopped(actor, deadline)?;
            running.current(deadline)?;
            // Historical restoration is independently verified service evidence.
            // It grants no input authority across Buddy's own uinput creation.
            write_new(
                &context.root,
                &format!("{attempt}.{role}.restored"),
                &running.encode()?,
            )?;
            return Ok(());
        }
        if s.property("ActiveState") == Some("activating")
            || (s.property("Job") != Some("") && s.property("ActiveState") != Some("deactivating"))
        {
            pending = true;
            pace(deadline);
            continue;
        }
        let terminal = baseline.stopped(deadline)?;
        if terminal.is_some() {
            ensure!(
                admitted && !requested && !pending,
                "unadmitted or uncertain start; no retry"
            );
            let mut input = if role == "buddy" && context.candidate.kind == Kind::Native {
                Some(RequestInput::new(TriggerCorner::LowerLeft, None)?)
            } else {
                None
            };
            context.current(deadline)?;
            stopped(actor, deadline)?;
            context.intent(&format!("{attempt}.{role}.start-intent"))?;
            // This stable claim survives helper interruption. A later helper
            // may verify an active service but cannot replay an uncertain start.
            context.intent(&format!("{role}.start-intent"))?;
            prior = Some(read_pin(&durable_intent, deadline)?.0);
            if role == "stock" && context.candidate.fault == Fault::RecoveryAfterIntent {
                context.intent("fixture-recovery-after-intent")?;
                unsafe { libc::raise(libc::SIGKILL) };
                anyhow::bail!("fixture recovery intent interruption");
            }
            context.current(deadline)?;
            stopped(actor, deadline)?;
            stopped(baseline, deadline)?;
            quiet(&mut input)?;
            check(deadline)?;
            requested = true;
            request("start", baseline.group().unit(), deadline)?;
            if role == "stock" && context.candidate.fault == Fault::RecoveryAfterStart {
                context.intent("fixture-recovery-after-start")?;
                unsafe { libc::raise(libc::SIGKILL) };
                anyhow::bail!("fixture recovery queued-start interruption");
            }
        }
        pace(deadline);
    }
}

fn recover(mut context: Context, deadline: Instant) -> Result<()> {
    if !present(&context.root)? {
        context.units("recovery", None, deadline)?;
        return Ok(());
    } // Actor creates root before any stop intent.
    context.bind_root()?;
    let record = context.root.join("ownership.json");
    if !present(&record)? {
        context.units("recovery", None, deadline)?;
        for leaf in [
            "stock.stop-intent",
            "buddy.stop-intent",
            "observer.launch-intent",
        ] {
            ensure!(
                !present(&context.root.join(leaf))?,
                "stop intent without ownership record"
            );
        }
        return Ok(());
    }
    let (pin, bytes) = read_pin(&record, deadline)?;
    context.pins.push(pin);
    let record: Ownership = serde_json::from_slice(&bytes)?;
    ensure!(
        record.version == 1
            && record.candidate_sha256 == context.digest
            && record.stock == context.candidate.stock.baseline
            && record.buddy == context.candidate.buddy.baseline,
        "ownership record mismatch"
    );
    let actor = ServiceBaseline::decode(record.actor.as_bytes())?;
    ensure!(
        actor.group().unit() == owned(&context.nonce, "actor"),
        "wrong original actor"
    );
    context.units("recovery", Some(&actor), deadline)?;
    stopped(&actor, deadline)?;
    let stock = ServiceBaseline::decode(record.stock.as_bytes())?;
    let buddy = ServiceBaseline::decode(record.buddy.as_bytes())?;
    for (role, baseline) in [("stock", &stock), ("buddy", &buddy)] {
        let intent = context.root.join(format!("{role}.stop-intent"));
        if present(&intent)? {
            let (pin, bytes) = read_pin(&intent, deadline)?;
            ensure!(bytes == context.nonce.as_bytes(), "invalid stop intent");
            context.pins.push(pin);
        } else {
            // No stop was admitted for this service: recovery may only confirm
            // its unchanged original instance, never repair an unrelated stop.
            baseline.current(deadline)?;
        }
    }
    let attempt = format!("recovery-{}", uuid::Uuid::new_v4().simple());
    context.intent(&attempt)?;
    if context.candidate.fault == Fault::RecoveryExit
        && !present(&context.root.join("fixture-recovery-exit"))?
    {
        context.intent("fixture-recovery-exit")?;
        anyhow::bail!("fixture first recovery interruption");
    }
    recovery_sequence(
        || restore(&context, &stock, &actor, "stock", &attempt, deadline),
        || {
            // Stock restoration owns the remaining budget first. A slow or
            // failed inventory can withhold Buddy, never delay stock recovery.
            let after = context.preserve(deadline);
            let record = write_new(
                &context.root,
                &format!("{attempt}.source-after"),
                format!("{after:?}").as_bytes(),
            );
            after?;
            record
        },
        || restore(&context, &buddy, &actor, "buddy", &attempt, deadline),
    )
}

fn recovery_sequence(
    restore_stock: impl FnOnce() -> Result<()>,
    preserve: impl FnOnce() -> Result<()>,
    restore_buddy: impl FnOnce() -> Result<()>,
) -> Result<()> {
    restore_stock()?;
    preserve()?;
    restore_buddy()
}

/// Explicit example entry only; the feature is absent from installed builds.
pub fn entry(arguments: Vec<String>) -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "bootstrap diagnostic requires root"
    );
    ensure!(
        arguments.len() == 2 || arguments.len() == 3,
        "Usage: resident_bootstrap_control ROLE NONCE [native|fixture]"
    );
    nonce(&arguments[1])?;
    let name = &arguments[1];
    let role = arguments[0].as_str();
    if role == "setup-complete" {
        ensure!(arguments.len() == 2, "unexpected setup arguments");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut context = Context::load(name, deadline)?;
        ensure!(
            context.candidate.capture.is_some(),
            "candidate has no capture setup"
        );
        context.bind_root()?;
        let (ownership_pin, bytes) = read_pin(&context.root.join("ownership.json"), deadline)?;
        let ownership: Ownership = serde_json::from_slice(&bytes)?;
        ensure!(
            ownership.version == 1 && ownership.candidate_sha256 == context.digest,
            "setup ownership mismatch"
        );
        let actor = ServiceBaseline::decode(ownership.actor.as_bytes())?;
        actor.current(deadline)?;
        let mut mailbox = setup::Mailbox::open(&context.root)?;
        let binding = mailbox
            .receive("ready", deadline)?
            .context("setup is not ready")?;
        for phase in ["observer-ended", "actor-ended"] {
            ensure!(
                mailbox.receive(phase, deadline)?.as_ref() == Some(&binding),
                "setup admissions did not end for this binding"
            );
        }
        binding.current(&context, binding.observer_pid())?;
        for role in ["stock", "buddy"] {
            stopped(
                &ServiceBaseline::decode(context.expected(role).baseline.as_bytes())?,
                deadline,
            )?;
        }
        context.current(deadline)?;
        ownership_pin.current()?;
        actor.current(deadline)?;
        mailbox.publish("complete", &binding, deadline)?;
        println!("setup_complete=true reader_identity=false");
        return Ok(());
    }
    if matches!(role, "units" | "inspect") {
        ensure!(arguments.len() == 3, "selection kind required");
        let kind = match arguments[2].as_str() {
            "native" => Kind::Native,
            "fixture" => Kind::Fixture,
            _ => anyhow::bail!("invalid selection kind"),
        };
        if role == "units" {
            let mut units = BTreeMap::new();
            for role in ["actor", "recovery", "stock", "buddy"] {
                if kind == Kind::Native && matches!(role, "stock" | "buddy") {
                    continue;
                }
                units.insert(owned(name, role), unit_text(name, kind, role)?);
            }
            println!("{}", serde_json::to_string_pretty(&units)?);
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut services = BTreeMap::new();
        for role in ["stock", "buddy"] {
            let baseline =
                ServiceBaseline::capture(Path::new(MOUNT), &target(kind, name, role), deadline)?;
            let path = executable(kind, name, role);
            let mut pin = Pinned::open(&path)?;
            let hash = sha(&pin.bytes(128 * 1024 * 1024, deadline)?);
            verify_service_process(kind, name, role, &baseline, digest(&hash)?, deadline)?;
            baseline.current(deadline)?;
            services.insert(
                role,
                ExpectedService {
                    baseline: String::from_utf8(baseline.encode()?)?,
                    executable_sha256: hash,
                },
            );
        }
        let mut unit_files = Vec::new();
        for path in unit_paths(kind, name) {
            let mut pin = Pinned::open(Path::new(&path))?;
            unit_files.push(UnitFile {
                sha256: sha(&pin.bytes(MAX_RECORD, deadline)?),
                path,
            });
        }
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"boot":fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim(),"services":services,"unit_files":unit_files,"source":inventory(&source_root(kind,name),deadline)?})
            )?
        );
        return Ok(());
    }
    ensure!(
        arguments.len() == 2
            && matches!(
                role,
                "actor" | "recovery" | "child" | "standin-stock" | "standin-buddy"
            ),
        "invalid diagnostic role"
    );
    if matches!(role, "standin-stock" | "standin-buddy") {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    let start = Instant::now();
    let context = Context::load(name, start + Duration::from_secs(20))?;
    if role == "child" {
        ensure!(
            context.candidate.kind == Kind::Fixture,
            "stand-in child forbidden for native candidate"
        );
        if matches!(context.candidate.fault, Fault::ActorKill | Fault::ActorHang) {
            let group = GroupBaseline::capture(Path::new(MOUNT), &owned(name, "actor"))?;
            write_new(
                &context.root,
                "fixture-child-ready",
                &serde_json::to_vec(group.parent())?,
            )?;
            // Create the empty completion marker only after readiness bytes and
            // their directory entry have been synced. Existence cannot expose a
            // partially written readiness record to the parent.
            write_new(&context.root, "fixture-child-ready-complete", b"")?;
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        thread::sleep(Duration::from_millis(250));
        return Ok(());
    }
    let seconds = if role == "recovery" {
        120
    } else if context.candidate.kind == Kind::Fixture {
        10
    } else {
        300
    };
    let evidence = context.root.clone();
    let result = if role == "actor" {
        actor(context, start + Duration::from_secs(seconds))
    } else {
        recover(context, start + Duration::from_secs(seconds))
    };
    let _ = write_new(
        &evidence,
        &format!("{role}-result-{}", uuid::Uuid::new_v4().simple()),
        format!("{result:?}\n").as_bytes(),
    );
    result
}

#[cfg(test)]
mod tests;
