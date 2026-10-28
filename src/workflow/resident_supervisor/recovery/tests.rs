use super::*;
use std::{
    collections::VecDeque,
    process::{Child, Command, Stdio},
};

fn snapshot(active: &str, sub: &str, pid: u32, job: &str) -> service::Snapshot {
    let mut fields: BTreeMap<String, String> = service::STATE
        .iter()
        .chain(service::POLICY)
        .map(|key| ((*key).to_owned(), String::new()))
        .collect();
    for (key, value) in [
        ("Id", STOCK),
        ("LoadState", "loaded"),
        ("ActiveState", active),
        ("SubState", sub),
        ("ControlPID", "0"),
        ("Job", job),
        ("ControlGroup", "/system.slice/xochitl.service"),
        ("InvocationID", "12121212121212121212121212121212"),
        ("ExecMainStartTimestampMonotonic", "1234"),
    ] {
        fields.insert(key.into(), value.into());
    }
    fields.insert("MainPID".into(), pid.to_string());
    fields.insert("ExecMainPID".into(), pid.to_string());
    parse(fields)
}
fn parse(fields: BTreeMap<String, String>) -> service::Snapshot {
    let unit = fields["Id"].clone();
    let bytes = fields
        .into_iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect::<String>();
    service::Snapshot::parse(bytes.as_bytes(), &unit).unwrap()
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
fn ready() -> service::Snapshot {
    snapshot("active", "running", 42, "")
}
fn stopped() -> service::Snapshot {
    snapshot("inactive", "dead", 0, "")
}
fn pending() -> service::Snapshot {
    snapshot("activating", "start", 42, "1234")
}

struct Fake {
    original: super::super::StockBaseline,
    expire_on_pace: bool,
    snapshots: VecDeque<service::Snapshot>,
    starts: usize,
    admissions: usize,
    empty_checks: usize,
    fail_admission: Option<usize>,
    populated: bool,
    fail_start: bool,
    fail_verify: bool,
    companion: bool,
    sync_stop_samples: VecDeque<bool>,
    fail_sync_verify: bool,
    fail_sync_delivery: bool,
    sync_verified: bool,
}
impl Fake {
    fn new(snapshots: Vec<service::Snapshot>) -> Self {
        Self {
            original: super::super::StockBaseline {
                process: ProcessIdentity {
                    pid: 42,
                    start_ticks: 100,
                },
                invocation: "12121212121212121212121212121212".into(),
                executable_sha256: [1; 32],
                unit_sha256: [2; 32],
                dropin_sha256: [3; 32],
            },
            expire_on_pace: false,
            snapshots: snapshots.into(),
            starts: 0,
            admissions: 0,
            empty_checks: 0,
            fail_admission: None,
            populated: false,
            fail_start: false,
            fail_verify: false,
            companion: false,
            sync_stop_samples: VecDeque::new(),
            fail_sync_verify: false,
            fail_sync_delivery: false,
            sync_verified: false,
        }
    }
}
impl Backend for Fake {
    fn original_stock(&self) -> &super::super::StockBaseline {
        &self.original
    }
    fn admit(&mut self, _: Instant) -> Result<()> {
        self.admissions += 1;
        ensure!(
            self.fail_admission != Some(self.admissions),
            "admission lost"
        );
        Ok(())
    }
    fn stock(&mut self, _: Instant) -> Result<service::Snapshot> {
        self.snapshots
            .pop_front()
            .context("unexpected extra sample")
    }
    fn start(&mut self, _: Instant) -> Result<()> {
        self.starts += 1;
        ensure!(!self.fail_start, "command outcome unknown");
        Ok(())
    }
    fn empty_stock_group(&mut self, _: Instant) -> Result<()> {
        self.empty_checks += 1;
        ensure!(!self.populated, "stock group populated");
        Ok(())
    }
    fn companion_stopped(&mut self, _: Instant) -> Result<bool> {
        Ok(self.sync_stop_samples.pop_front().unwrap_or(true))
    }
    fn companion_current(&mut self, _: Instant) -> Result<Option<RestoredCompanion>> {
        if self.companion {
            ensure!(
                self.sync_verified && !self.fail_sync_delivery,
                "companion changed at completion"
            );
            Ok(Some(RestoredCompanion {
                process: ProcessIdentity {
                    pid: 43,
                    start_ticks: 101,
                },
                invocation: "34".repeat(16),
            }))
        } else {
            Ok(None)
        }
    }
    fn verify(&mut self, _: &service::Snapshot, _: Instant) -> Result<ProcessIdentity> {
        ensure!(!self.fail_verify, "stock executable or group mismatch");
        if self.companion {
            ensure!(!self.fail_sync_verify, "companion restoration failed");
            self.sync_verified = true;
        }
        Ok(ProcessIdentity {
            pid: 42,
            start_ticks: 100,
        })
    }
    fn pace(&mut self, deadline: Instant) {
        if self.expire_on_pace {
            thread::sleep(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
            );
        }
    }
}

#[test]
fn restoration_starts_once_then_observes_and_verifies() {
    let mut b = Fake::new(vec![stopped(), stopped(), pending(), ready(), ready()]);
    let result = restore(&mut b, deadline()).unwrap();
    assert!(result.start_requested);
    assert_eq!(result.process.pid, 42);
    assert_eq!((b.starts, b.empty_checks), (1, 1));
    assert!(b.snapshots.is_empty());
}

#[test]
fn installed_restore_waits_for_sync_drain_and_reports_separate_identity() {
    let mut b = Fake::new(vec![stopped(), stopped(), stopped(), ready(), ready()]);
    b.companion = true;
    b.sync_stop_samples = [false, true, true].into();
    let restored = restore(&mut b, deadline()).unwrap();
    assert_eq!((b.starts, b.empty_checks), (1, 1));
    assert!(b.sync_stop_samples.is_empty());
    assert_eq!(restored.companion.unwrap().process.pid, 43);
    assert!(b.snapshots.is_empty());
}

#[test]
fn lost_sync_drain_at_dispatch_withholds_stock_start() {
    let mut b = Fake::new(vec![stopped(), stopped()]);
    b.companion = true;
    b.sync_stop_samples = [true, false].into();
    assert!(restore(&mut b, deadline()).is_err());
    assert_eq!(b.starts, 0);
}

#[test]
fn active_stock_needs_sync_restoration_and_closing_proof_but_no_old_sync_stop() {
    for mode in 0..3 {
        let mut b = Fake::new(vec![ready(), ready()]);
        b.companion = true;
        b.sync_stop_samples = [false].into(); // must not be consumed on active path
        b.fail_sync_verify = mode == 1;
        b.fail_sync_delivery = mode == 2;
        let result = restore(&mut b, deadline());
        assert_eq!(result.is_ok(), mode == 0);
        assert_eq!(b.starts, 0);
        assert_eq!(b.sync_stop_samples.len(), 1);
    }
}

#[test]
fn installed_profile_and_lease_cannot_silently_upgrade_each_other() {
    let f = crate::workflow::resident_supervisor::tests::Fixture::new();
    let original = f.record();
    let new = original
        .clone()
        .with_companion(f.companion_record())
        .unwrap();
    let mut p = profile(&original);
    p.validate(&original).unwrap();
    assert!(p.validate(&new).is_err());
    p.version = 2;
    assert!(p.validate_definition().is_err());
    p.companion = Some(service::vendor::PROFILE_ID.into());
    p.validate(&new).unwrap();
    assert!(p.validate(&original).is_err());
    p.companion = Some("unqualified-profile".into());
    assert!(p.validate_definition().is_err());
    p.version = 1;
    p.companion = Some(service::vendor::PROFILE_ID.into());
    assert!(p.validate_definition().is_err());
}
#[test]
fn startable_recheck_tolerates_only_dependency_order_after_group_admission() {
    let first = changed(
        &stopped(),
        &[
            ("After", "a.target b.service"),
            ("Conflicts", "c.target d.service"),
        ],
    );
    let last = changed(
        &first,
        &[
            ("After", "b.service a.target"),
            ("Conflicts", "d.service c.target"),
        ],
    );
    assert_ne!(first, last);
    let mut b = Fake::new(vec![first.clone(), last.clone(), ready(), ready()]);
    assert!(restore(&mut b, deadline()).unwrap().start_requested);
    assert_eq!((b.starts, b.empty_checks), (1, 1));
    for (key, value) in [
        ("Job", "42"),
        ("WatchdogUSec", "infinity"),
        ("InvocationID", "34343434343434343434343434343434"),
        ("After", "a.target"),
        ("Conflicts", "c.target c.target d.service"),
    ] {
        let mut b = Fake::new(vec![first.clone(), changed(&last, &[(key, value)])]);
        assert!(restore(&mut b, deadline()).is_err(), "{key}");
        assert_eq!((b.starts, b.empty_checks), (0, 1));
    }
    let mut b = Fake::new(vec![first, last]);
    b.populated = true;
    assert!(restore(&mut b, deadline()).is_err());
    assert_eq!(b.starts, 0);
}

#[test]
fn active_or_already_starting_stock_is_never_restarted() {
    for samples in [vec![ready(), ready()], vec![pending(), ready(), ready()]] {
        let mut b = Fake::new(samples);
        assert!(!restore(&mut b, deadline()).unwrap().start_requested);
        assert_eq!((b.starts, b.empty_checks), (0, 0));
    }
}
#[test]
fn admission_group_and_concurrent_job_refuse_before_command() {
    for mode in 0..4 {
        let mut b = Fake::new(vec![stopped(), stopped()]);
        match mode {
            0 => b.fail_admission = Some(1),
            1 => b.fail_admission = Some(2),
            2 => b.populated = true,
            _ => b.snapshots[1] = pending(),
        }
        assert!(restore(&mut b, deadline()).is_err());
        assert_eq!(b.starts, 0);
    }
}
#[test]
fn command_uncertainty_or_failed_start_does_not_retry() {
    let mut uncertain = Fake::new(vec![stopped(), stopped()]);
    uncertain.fail_start = true;
    assert!(restore(&mut uncertain, deadline())
        .unwrap_err()
        .to_string()
        .contains("unknown"));
    assert_eq!(uncertain.starts, 1);
    let mut failed = Fake::new(vec![stopped(), stopped(), stopped()]);
    assert!(restore(&mut failed, deadline())
        .unwrap_err()
        .to_string()
        .contains("no retry"));
    assert_eq!(failed.starts, 1);
    let mut prior = Fake::new(vec![pending(), stopped()]);
    assert!(restore(&mut prior, deadline()).is_err());
    assert_eq!(prior.starts, 0);
}
#[test]
fn completion_requires_process_and_closing_admission_and_stock() {
    for mode in 0..3 {
        let mut b = Fake::new(vec![ready(), ready()]);
        match mode {
            0 => b.fail_verify = true,
            1 => b.fail_admission = Some(2),
            _ => b.snapshots[1] = stopped(),
        }
        assert!(restore(&mut b, deadline()).is_err());
        assert_eq!(b.starts, 0);
    }
    let mut b = Fake::new(vec![]);
    assert!(restore(&mut b, Instant::now()).is_err());
    assert_eq!(b.admissions, 0);
}
#[test]
fn unknown_or_incomplete_stock_state_is_not_ready_or_startable() {
    for s in [
        snapshot("deactivating", "unknown", 42, ""),
        snapshot("active", "exited", 0, ""),
        snapshot("inactive", "dead", 42, ""),
        snapshot("active", "running", 0, ""),
    ] {
        assert!(stock_state(&s).is_err());
    }
}

#[test]
fn empty_group_checks_real_files_and_refuses_missing_events_and_links() {
    let root = std::env::temp_dir().join(format!("buddy-recovery-group-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let slice = File::open(&root).unwrap();
    assert!(empty_group(&slice).unwrap().is_none());
    let group = root.join(STOCK);
    fs::create_dir(&group).unwrap();
    assert!(empty_group(&slice).is_err());
    fs::write(group.join("cgroup.events"), "populated 1\n").unwrap();
    assert!(empty_group(&slice).is_err());
    fs::write(group.join("cgroup.events"), "populated 0\n").unwrap();
    assert!(empty_group(&slice).unwrap().is_some());
    fs::remove_file(group.join("cgroup.events")).unwrap();
    fs::remove_dir(&group).unwrap();
    std::os::unix::fs::symlink("/does-not-exist", &group).unwrap();
    assert!(empty_group(&slice).is_err());
    fs::remove_file(&group).unwrap();
    fs::remove_dir(&root).unwrap();
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn actual_running_executable_argv_incarnation_and_loader_environment() {
    use std::os::unix::process::CommandExt;
    let executable = fs::canonicalize("/usr/bin/python3").unwrap();
    let executable = executable.to_str().unwrap();
    let script = "import time; time.sleep(30)";
    let digest = sha256(&fs::read(executable).unwrap());
    for (bad_environment, argv0) in [
        (false, executable),
        (true, executable),
        (false, "/reviewed/service-alias"),
        (true, "/reviewed/service-alias"),
    ] {
        let mut command = Command::new(executable);
        command
            .arg0(argv0)
            .args(["-c", script])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if bad_environment {
            command.env("LD_RECOVERY_TEST", "1");
        }
        let child = OwnedChild(command.spawn().unwrap());
        let limit = deadline();
        let process = loop {
            if let Ok(process) = ProcessIdentity::read(child.0.id()) {
                if fs::read_link(format!("/proc/{}/exe", process.pid))
                    .ok()
                    .as_deref()
                    == Some(Path::new(executable))
                {
                    break process;
                }
            }
            assert!(Instant::now() < limit, "fixture did not exec");
            thread::sleep(Duration::from_millis(5));
        };
        let argv = [argv0, "-c", script];
        let result = verify_process(&process, executable, digest, &argv, limit);
        if bad_environment {
            assert!(result.unwrap_err().to_string().contains("loader override"));
        } else {
            result.unwrap();
            if argv0 != executable {
                let error = verify_process(
                    &process,
                    executable,
                    digest,
                    &[executable, "-c", script],
                    deadline(),
                )
                .unwrap_err();
                assert!(error.to_string().contains("argv mismatch"));
            }
            assert!(verify_process(&process, executable, [1; 32], &argv, deadline()).is_err());
            assert!(
                verify_process(&process, executable, digest, &[executable], deadline()).is_err()
            );
            let changed = ProcessIdentity {
                start_ticks: process.start_ticks + 1,
                ..process
            };
            assert!(verify_process(&changed, executable, digest, &argv, deadline()).is_err());
        }
    }
}

#[test]
fn bounded_reads_and_trusted_artifact_paths_refuse_unsafe_inputs() {
    assert!(bounded_bytes(&mut &b"12345"[..], 4, deadline()).is_err());
    assert!(bounded_bytes(&mut &b""[..], 4, Instant::now()).is_err());
    assert_eq!(
        bounded_bytes(&mut &b"1234"[..], 4, deadline()).unwrap(),
        b"1234"
    );
    // /tmp is writable by other users, even when its final file is protected.
    let path = std::env::temp_dir().join(format!("buddy-recovery-path-{}", uuid::Uuid::new_v4()));
    fs::write(&path, b"x").unwrap();
    assert!(Pinned::open(&path).is_err());
    fs::remove_file(path).unwrap();
    assert!(trusted_file(Path::new("/usr/bin/../bin/python3")).is_err());
}

#[test]
fn nested_policy_json_rejects_duplicate_missing_and_unknown_fields() {
    #[derive(Deserialize)]
    struct Policy {
        #[serde(deserialize_with = "policy_fields")]
        fields: BTreeMap<String, String>,
    }
    let properties = service::POLICY
        .iter()
        .map(|key| format!("\"{key}\":\"\""))
        .collect::<Vec<_>>()
        .join(",");
    let good = format!("{{\"fields\":{{{properties}}}}}");
    assert_eq!(
        serde_json::from_str::<Policy>(&good).unwrap().fields.len(),
        service::POLICY.len()
    );
    for bad in [
        format!("{{\"fields\":{{{properties},\"Type\":\"oneshot\"}}}}"),
        format!("{{\"fields\":{{{properties},\"OnSuccessJobMode\":\"fail\"}}}}"),
        format!("{{\"fields\":{{{properties},\"OnFailureJobMode\":\"fail\"}}}}"),
        format!("{{\"fields\":{{{properties},\"Unknown\":\"x\"}}}}"),
        "{\"fields\":{\"Type\":\"oneshot\"}}".to_owned(),
    ] {
        assert!(serde_json::from_str::<Policy>(&bad).is_err());
    }
    for missing in ["OnSuccessJobMode", "OnFailureJobMode"] {
        let properties = service::POLICY
            .iter()
            .filter(|key| **key != missing)
            .map(|key| format!("\"{key}\":\"\""))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            serde_json::from_str::<Policy>(&format!("{{\"fields\":{{{properties}}}}}")).is_err()
        );
    }
}

pub(super) fn profile(record: &LeaseRecord) -> Profile {
    fn policy(fragment: &str) -> BTreeMap<String, String> {
        let mut p: BTreeMap<_, _> = service::POLICY
            .iter()
            .map(|k| ((*k).into(), String::new()))
            .collect();
        for (key, value) in [
            ("FragmentPath", fragment),
            ("NeedDaemonReload", "no"),
            ("KillMode", "control-group"),
            ("Delegate", "no"),
            ("SendSIGKILL", "yes"),
            ("Restart", "no"),
            ("DefaultDependencies", "yes"),
            ("Conflicts", "shutdown.target"),
            ("OnSuccessJobMode", "fail"),
            ("OnFailureJobMode", "replace"),
        ] {
            p.insert(key.into(), value.into());
        }
        p
    }
    let mut buddy_policy = policy(BUDDY_UNIT);
    buddy_policy.insert("Type".into(), "notify".into());
    buddy_policy.insert("OnSuccess".into(), HELPER.into());
    buddy_policy.insert("OnFailure".into(), HELPER.into());
    buddy_policy.insert("OnFailureJobMode".into(), "fail".into());
    buddy_policy.insert("WatchdogUSec".into(), "2min".into());
    let mut helper_policy = policy(HELPER_UNIT);
    helper_policy.insert("Type".into(), "oneshot".into());
    helper_policy.insert("WatchdogUSec".into(), "0".into());
    helper_policy.insert("TimeoutStartUSec".into(), "2min 10s".into());
    helper_policy.insert("Conflicts".into(), format!("shutdown.target {BUDDY}"));
    helper_policy.insert("After".into(), BUDDY.into());
    let mut stock_policy = policy(STOCK_UNIT);
    stock_policy.insert("DropInPaths".into(), STOCK_DROPIN.into());
    stock_policy.insert("TimeoutStopUSec".into(), "1min 30s".into());
    Profile {
        version: 1,
        companion: None,
        firmware: "3.28.0.172".into(),
        kernel: "5.4.70-v1.6.3-rm11x".into(),
        native_profile_sha256: record.profile_sha256,
        adapter_sha256: record.adapter_sha256,
        app_sha256: [7; 32],
        buddy_unit_sha256: [8; 32],
        helper_unit_sha256: [9; 32],
        stock_executable_sha256: record.stock.executable_sha256,
        stock_unit_sha256: record.stock.unit_sha256,
        stock_dropin_sha256: record.stock.dropin_sha256,
        buddy_policy,
        helper_policy,
        stock_policy,
    }
}

#[test]
fn installed_buddy_requires_non_replacing_trigger_modes_without_rewriting_stock() {
    let fixture = crate::workflow::resident_supervisor::tests::Fixture::new();
    let record = fixture.record();
    let p = profile(&record);
    let stock = p.stock_policy.clone();
    let helper = p.helper_policy.clone();
    p.validate(&record).unwrap();
    assert_eq!(p.stock_policy["OnFailureJobMode"], "replace");
    assert_eq!(p.helper_policy["OnFailureJobMode"], "replace");
    assert_eq!(p.stock_policy, stock);
    assert_eq!(p.helper_policy, helper);
    for key in ["OnSuccessJobMode", "OnFailureJobMode"] {
        for value in [
            None,
            Some(""),
            Some("replace"),
            Some("replace-irreversibly"),
            Some("ignore-dependencies"),
            Some("fail replace"),
        ] {
            let mut p = profile(&record);
            if let Some(value) = value {
                p.buddy_policy.insert(key.into(), value.into());
            } else {
                p.buddy_policy.remove(key);
            }
            assert!(p.validate(&record).is_err(), "{key}={value:?}");
        }
    }
}

#[test]
fn profile_is_bound_to_original_artifacts_and_complete_loaded_policy() {
    let fixture = crate::workflow::resident_supervisor::tests::Fixture::new();
    let record = fixture.record();
    profile(&record).validate(&record).unwrap();
    for mode in 0..10 {
        let mut p = profile(&record);
        match mode {
            0 => p.native_profile_sha256 = [0; 32],
            1 => p.adapter_sha256 = [0; 32],
            2 => p.stock_executable_sha256 = [0; 32],
            3 => p.stock_unit_sha256 = [0; 32],
            4 => p.stock_dropin_sha256 = [0; 32],
            5 => {
                p.helper_policy.insert("After".into(), "".into());
            }
            6 => {
                p.buddy_policy.insert("Restart".into(), "on-failure".into());
            }
            7 => {
                p.stock_policy
                    .insert("NeedDaemonReload".into(), "yes".into());
            }
            8 => {
                p.helper_policy.remove("Delegate");
            }
            _ => p.version = 2,
        }
        assert!(p.validate(&record).is_err(), "case {mode}");
    }
    let mut old = record.clone();
    old.version = 1;
    assert!(old.validate().is_err());
    let mut missing = serde_json::to_value(&record).unwrap();
    missing
        .as_object_mut()
        .unwrap()
        .remove("recovery_profile_sha256");
    assert!(serde_json::from_value::<LeaseRecord>(missing).is_err());
}

#[test]
fn helper_requires_actual_main_process_invocation_and_oneshot_start() {
    let fixture = crate::workflow::resident_supervisor::tests::Fixture::new();
    let p = profile(&fixture.record());
    let process = ProcessIdentity {
        pid: 42,
        start_ticks: 100,
    };
    let mut fields: BTreeMap<String, String> = service::STATE
        .iter()
        .map(|k| ((*k).into(), String::new()))
        .collect();
    fields.extend(p.helper_policy.clone());
    for (key, value) in [
        ("Id", HELPER),
        ("LoadState", "loaded"),
        ("ActiveState", "activating"),
        ("SubState", "start"),
        ("MainPID", "42"),
        ("ExecMainPID", "42"),
        ("ControlPID", "0"),
        ("ExecMainStartTimestampMonotonic", "1234"),
        ("InvocationID", "12121212121212121212121212121212"),
        (
            "ControlGroup",
            "/system.slice/reader-buddy-stock-recovery.service",
        ),
        ("Job", "42"),
    ] {
        fields.insert(key.into(), value.into());
    }
    helper_state(&parse(fields.clone()), &process, &p.helper_policy).unwrap();
    for (key, value) in [
        ("MainPID", "41"),
        ("ExecMainPID", "41"),
        ("InvocationID", ""),
        ("ExecMainStartTimestampMonotonic", "0"),
        ("ActiveState", "active"),
        ("SubState", "running"),
        ("Type", "simple"),
        ("ControlPID", "41"),
        ("ControlGroup", "/wrong"),
        ("NeedDaemonReload", "yes"),
    ] {
        let mut changed = fields.clone();
        changed.insert(key.into(), value.into());
        assert!(
            helper_state(&parse(changed), &process, &p.helper_policy).is_err(),
            "{key}"
        );
    }
}

#[test]
fn firmware_uses_observed_image_version_without_shell_evaluation() {
    assert_eq!(
        firmware_version("ID=codex\nVERSION_ID=5.8.203\nIMG_VERSION=\"3.28.0.172\"\n").unwrap(),
        "3.28.0.172"
    );
    assert_eq!(
        firmware_version("IMG_VERSION=3.28.0.172\n").unwrap(),
        "3.28.0.172"
    );
    for input in [
        "VERSION_ID=5.8.203\n",
        "IMG_VERSION=3\nIMG_VERSION=4\n",
        "IMG_VERSION=\"3\n",
        "IMG_VERSION=$(uname)\n",
        "IMG_VERSION=\n",
    ] {
        assert!(firmware_version(input).is_err());
    }
}

fn changed(s: &service::Snapshot, changes: &[(&str, &str)]) -> service::Snapshot {
    let mut fields: BTreeMap<_, _> = service::STATE
        .iter()
        .chain(service::POLICY)
        .map(|key| ((*key).to_owned(), s.property(key).unwrap().to_owned()))
        .collect();
    for (key, value) in changes {
        fields.insert((*key).into(), (*value).into());
    }
    parse(fields)
}
fn stopping() -> service::Snapshot {
    changed(
        &snapshot("deactivating", "stop-sigterm", 42, "5678"),
        &[("ControlPID", "43")],
    )
}
fn stopped_original() -> service::Snapshot {
    changed(
        &stopping(),
        &[
            ("ActiveState", "inactive"),
            ("SubState", "dead"),
            ("MainPID", "0"),
            ("ControlPID", "0"),
            ("Job", ""),
        ],
    )
}
fn stopped_cleared() -> service::Snapshot {
    changed(
        &stopped_original(),
        &[
            ("InvocationID", ""),
            ("ExecMainPID", "0"),
            ("ExecMainStartTimestampMonotonic", "0"),
            ("ControlGroup", ""),
        ],
    )
}

#[test]
fn original_shutdown_with_control_process_allows_first_start_after_terminal() {
    for terminal in [
        stopped_original(),
        stopped_cleared(),
        changed(
            &stopped_original(),
            &[("ActiveState", "failed"), ("SubState", "failed")],
        ),
    ] {
        let mut b = Fake::new(vec![
            stopping(),
            terminal.clone(),
            terminal,
            pending(),
            ready(),
            ready(),
        ]);
        assert!(restore(&mut b, deadline()).unwrap().start_requested);
        assert_eq!((b.starts, b.empty_checks), (1, 1));
        assert!(b.snapshots.is_empty());
    }
}

#[test]
fn shutdown_identity_refuses_partial_clearing_replacement_and_changed_timestamp() {
    for changes in [
        vec![("InvocationID", "")],
        vec![("ExecMainPID", "0")],
        vec![("ExecMainStartTimestampMonotonic", "0")],
        vec![("InvocationID", "34343434343434343434343434343434")],
        vec![("ExecMainStartTimestampMonotonic", "1235")],
        vec![("ControlPID", "43")],
        vec![("MainPID", "42")],
    ] {
        let terminal = changed(&stopped_original(), &changes);
        let mut b = Fake::new(vec![stopping(), terminal]);
        assert!(restore(&mut b, deadline()).is_err(), "{changes:?}");
        assert_eq!(b.starts, 0);
    }
    let failed_cleared = changed(
        &stopped_cleared(),
        &[("ActiveState", "failed"), ("SubState", "failed")],
    );
    let mut b = Fake::new(vec![stopping(), failed_cleared]);
    assert!(restore(&mut b, deadline()).is_err());
    assert_eq!(b.starts, 0);
    let mut b = Fake::new(vec![
        stopping(),
        changed(&stopping(), &[("ExecMainStartTimestampMonotonic", "1235")]),
    ]);
    assert!(restore(&mut b, deadline())
        .unwrap_err()
        .to_string()
        .contains("identity changed"));
    assert_eq!(b.starts, 0);
}

#[test]
fn waiting_shutdown_never_clears_prior_start_uncertainty() {
    for first in [pending(), snapshot("active", "running", 42, "7890")] {
        let mut b = Fake::new(vec![first, stopping(), stopped_original()]);
        assert!(restore(&mut b, deadline())
            .unwrap_err()
            .to_string()
            .contains("no retry"));
        assert_eq!(b.starts, 0);
    }
    let mut b = Fake::new(vec![stopped(), stopped(), stopping(), stopped_original()]);
    assert!(restore(&mut b, deadline())
        .unwrap_err()
        .to_string()
        .contains("no retry"));
    assert_eq!(b.starts, 1);
    let mut b = Fake::new(vec![stopping(), pending(), stopped_original()]);
    assert!(restore(&mut b, deadline())
        .unwrap_err()
        .to_string()
        .contains("no retry"));
    assert_eq!(b.starts, 0);
}

#[test]
fn original_shutdown_expiry_and_populated_group_never_start_stock() {
    let mut b = Fake::new(vec![stopping()]);
    b.expire_on_pace = true;
    assert!(restore(&mut b, Instant::now() + Duration::from_millis(20))
        .unwrap_err()
        .to_string()
        .contains("deadline expired"));
    assert_eq!(b.starts, 0);
    let mut b = Fake::new(vec![stopping(), stopped_original()]);
    b.populated = true;
    assert!(restore(&mut b, deadline()).is_err());
    assert_eq!(b.starts, 0);
}

#[test]
fn shutdown_wait_requires_known_substate_and_original_stock() {
    let b = Fake::new(vec![]);
    for sub in [
        "stop",
        "stop-watchdog",
        "stop-sigterm",
        "stop-sigkill",
        "stop-post",
        "final-watchdog",
        "final-sigterm",
        "final-sigkill",
    ] {
        StoppingIdentity::capture(&changed(&stopping(), &[("SubState", sub)]), &b.original)
            .unwrap();
    }
    for changes in [
        vec![("SubState", "future-stop")],
        vec![("ExecMainPID", "44")],
        vec![("InvocationID", "34343434343434343434343434343434")],
        vec![("MainPID", "44")],
        vec![("ControlGroup", "/wrong")],
        vec![("ExecMainStartTimestampMonotonic", "0")],
        vec![("ControlPID", "1")],
    ] {
        assert!(StoppingIdentity::capture(&changed(&stopping(), &changes), &b.original).is_err());
    }
    StoppingIdentity::capture(&changed(&stopping(), &[("MainPID", "0")]), &b.original).unwrap();
    assert!(stock_state(&changed(&pending(), &[("ControlPID", "43")])).is_err());
}

#[test]
fn profile_requires_supported_watchdog_and_both_service_timeout_budgets() {
    let fixture = crate::workflow::resident_supervisor::tests::Fixture::new();
    let record = fixture.record();
    for value in ["", "infinity", "1s", "2min"] {
        let mut p = profile(&record);
        p.helper_policy.insert("WatchdogUSec".into(), value.into());
        assert!(p.validate(&record).is_err(), "helper watchdog {value}");
    }
    for value in ["", "0", "infinity", "129s", "130s", "2min", "2min 11s"] {
        let mut p = profile(&record);
        p.helper_policy
            .insert("TimeoutStartUSec".into(), value.into());
        assert!(p.validate(&record).is_err(), "{value}");
    }
    let mut p = profile(&record);
    p.stock_policy
        .insert("TimeoutStopUSec".into(), "2min".into());
    assert!(p.validate(&record).is_err());
    let mut p = profile(&record);
    p.buddy_policy.insert("WatchdogUSec".into(), "1min".into());
    assert!(p.validate(&record).is_err());
}
