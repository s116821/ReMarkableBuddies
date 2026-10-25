use super::*;
use crate::workflow::resident_supervisor::tests::Fixture;

fn retained(record: &LeaseRecord) -> Snapshot {
    let mut fields: BTreeMap<String, String> = STATE
        .iter()
        .chain(POLICY)
        .map(|key| ((*key).to_owned(), String::new()))
        .collect();
    for (key, value) in [
        ("Id", "fixture.service"),
        ("LoadState", "loaded"),
        ("ActiveState", "inactive"),
        ("SubState", "dead"),
        ("MainPID", "0"),
        ("ControlPID", "0"),
        ("ControlGroup", "/system.slice/fixture.service"),
        ("KillMode", "control-group"),
        ("Delegate", "no"),
        ("Restart", "no"),
        ("RestartMode", "normal"),
        ("SendSIGKILL", "yes"),
        ("DefaultDependencies", "yes"),
        ("FragmentPath", "/run/systemd/system/fixture.service"),
        ("OnSuccess", "fixture-recovery.service"),
        ("OnFailure", "fixture-recovery.service"),
        ("WatchdogUSec", "3s"),
        ("TimeoutStopUSec", "2s"),
    ] {
        fields.insert(key.to_owned(), value.to_owned());
    }
    fields.insert("ExecMainPID".into(), record.group.parent.pid.to_string());
    fields.insert(
        "ExecMainStartTimestampMonotonic".into(),
        record.buddy_start_micros.to_string(),
    );
    fields.insert("InvocationID".into(), record.buddy_invocation.clone());
    Snapshot::parse(&encode(&fields), "fixture.service").unwrap()
}
fn encode(fields: &BTreeMap<String, String>) -> Vec<u8> {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn stock_start_is_fixed_nonblocking_and_never_replaces_a_job() {
    let command = start_command();
    assert_eq!(command.get_program(), "/bin/systemctl");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--system",
            "--no-pager",
            "--no-ask-password",
            "--no-block",
            "--job-mode=fail",
            "start",
            "--",
            "xochitl.service",
        ]
    );
}
fn cleared(mut snapshot: Snapshot) -> Snapshot {
    for (key, value) in [
        ("InvocationID", ""),
        ("ExecMainPID", "0"),
        ("ExecMainStartTimestampMonotonic", "0"),
        ("ControlGroup", ""),
    ] {
        snapshot.fields.insert(key.into(), value.into());
    }
    snapshot
}

#[test]
fn exact_properties_are_required_and_canonical() {
    let f = Fixture::new();
    let expected = retained(&f.record());
    let bytes = encode(&expected.fields);
    assert_eq!(
        Snapshot::parse(&bytes, "fixture.service").unwrap(),
        expected
    );
    for key in STATE.iter().chain(POLICY) {
        let mut missing = expected.fields.clone();
        missing.remove(*key);
        assert!(
            Snapshot::parse(&encode(&missing), "fixture.service").is_err(),
            "{key}"
        );
        let mut duplicate = bytes.clone();
        duplicate.extend_from_slice(format!("{key}=\n").as_bytes());
        assert!(
            Snapshot::parse(&duplicate, "fixture.service").is_err(),
            "{key}"
        );
    }
    for (key, value) in [
        ("MainPID", "+0"),
        ("ControlPID", "00"),
        ("ExecMainPID", "4294967296"),
        ("ExecMainStartTimestampMonotonic", "-1"),
        ("InvocationID", "00000000000000000000000000000000"),
        ("Id", "other.service"),
        ("Unknown", "x"),
    ] {
        let mut changed = expected.fields.clone();
        changed.insert(key.into(), value.into());
        assert!(
            Snapshot::parse(&encode(&changed), "fixture.service").is_err(),
            "{key}"
        );
    }
    for invalid in [
        vec![b'x'; LIMIT + 1],
        vec![255],
        b"Id=fixture.service\0\n".to_vec(),
    ] {
        assert!(Snapshot::parse(&invalid, "fixture.service").is_err());
    }
}

#[test]
fn retained_terminal_identity_requires_original_incarnation_and_safe_state() {
    let f = Fixture::new();
    let record = f.record();
    let s = retained(&record);
    for group in [
        GroupObservation::EmptyOriginal,
        GroupObservation::OriginalAbsent,
    ] {
        assert_eq!(
            bracket(&record, &s, group, &s).unwrap(),
            TerminalIdentity::Retained
        );
    }
    assert!(bracket(&record, &s, GroupObservation::Populated, &s).is_err());
    for (key, value) in [
        ("LoadState", "not-found"),
        ("LoadState", "masked"),
        ("MainPID", "1234"),
        ("ControlPID", "1234"),
        ("Job", "123 start"),
        ("ActiveState", "activating"),
        ("SubState", "exited"),
        ("InvocationID", "abababababababababababababababab"),
        ("ExecMainPID", "42"),
        ("ExecMainStartTimestampMonotonic", "999"),
        ("ControlGroup", "/wrong"),
        ("KillMode", "process"),
        ("Delegate", "yes"),
        ("Restart", "on-failure"),
        ("RestartForceExitStatus", "1"),
        ("SendSIGKILL", "no"),
        ("DefaultDependencies", "no"),
    ] {
        let mut bad = s.clone();
        bad.fields.insert(key.into(), value.into());
        assert!(
            bracket(&record, &bad, GroupObservation::OriginalAbsent, &bad).is_err(),
            "{key}"
        );
    }
    let mut failed = s.clone();
    failed.fields.insert("ActiveState".into(), "failed".into());
    failed.fields.insert("SubState".into(), "failed".into());
    assert_eq!(
        bracket(&record, &failed, GroupObservation::OriginalAbsent, &failed).unwrap(),
        TerminalIdentity::Retained
    );
}

#[test]
fn cleared_retry_needs_deleted_original_and_no_terminal_marker() {
    let f = Fixture::new();
    let record = f.record();
    let s = cleared(retained(&record));
    // A helper can fail before publishing any marker. The immutable original
    // lease and qualified deletion branch remain usable without such a marker.
    assert_eq!(
        bracket(&record, &s, GroupObservation::OriginalAbsent, &s).unwrap(),
        TerminalIdentity::Cleared
    );
    assert!(bracket(&record, &s, GroupObservation::EmptyOriginal, &s).is_err());
    for (key, value) in [
        ("InvocationID", record.buddy_invocation.as_str()),
        ("ExecMainPID", "1234"),
        ("ExecMainStartTimestampMonotonic", "1234"),
        ("ActiveState", "failed"),
        ("SubState", "failed"),
    ] {
        let mut partial = s.clone();
        partial.fields.insert(key.into(), value.into());
        assert!(
            bracket(
                &record,
                &partial,
                GroupObservation::OriginalAbsent,
                &partial
            )
            .is_err(),
            "{key}"
        );
    }
}

#[test]
fn policy_or_identity_transition_inside_bracket_refuses() {
    let f = Fixture::new();
    let record = f.record();
    let s = retained(&record);
    assert!(bracket(
        &record,
        &s,
        GroupObservation::OriginalAbsent,
        &cleared(s.clone())
    )
    .is_err());
    for key in POLICY {
        let mut after = s.clone();
        after.fields.get_mut(*key).unwrap().push('x');
        assert!(
            bracket(&record, &s, GroupObservation::OriginalAbsent, &after).is_err(),
            "{key}"
        );
    }
    assert_eq!(s.policy().property("KillMode"), Some("control-group"));
}

fn python(code: &str) -> Command {
    let mut cmd = Command::new("/usr/bin/python3");
    cmd.args(["-S", "-c", code]);
    cmd
}
#[test]
fn owned_command_preserves_output_and_clears_environment() {
    let mut cmd = python("import os,sys; assert 'SHOULD_BE_REMOVED' not in os.environ; assert os.environ['LANG']=='C'; sys.stdout.write('observed=λ\\n')");
    cmd.env("SHOULD_BE_REMOVED", "yes");
    assert_eq!(
        run(cmd, Instant::now() + Duration::from_secs(2)).unwrap(),
        "observed=λ\n".as_bytes()
    );
    let args: Vec<_> = command("-Hremote.service")
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(&args[args.len() - 2..], ["--", "-Hremote.service"]);
}
#[test]
fn owned_command_faults_are_bounded_and_reaped() {
    for (code, milliseconds, cause) in [
        (
            "import time; time.sleep(10)",
            400,
            "service observation deadline expired",
        ),
        (
            "import sys; sys.exit(71)",
            2000,
            "service command exited unsuccessfully",
        ),
        (
            "import sys; sys.stderr.write('error')",
            2000,
            "service command wrote stderr",
        ),
        (
            "import sys; sys.stdout.write('x'*17000)",
            2000,
            "command output exceeds bound",
        ),
    ] {
        let start = Instant::now();
        let error = run(python(code), start + Duration::from_millis(milliseconds)).unwrap_err();
        let failure = error
            .downcast_ref::<CommandFailure>()
            .expect("spawned child diagnostic");
        assert!(failure.reaped, "{failure:?}");
        assert_eq!(failure.error.to_string(), cause, "{code}: {failure:?}");
        assert!(!std::path::Path::new(&format!("/proc/{}", failure.pid)).exists());
        assert!(start.elapsed() < Duration::from_millis(milliseconds) + Duration::from_secs(3));
    }
    assert!(run(python("raise Exception('must not run')"), Instant::now()).is_err());
}

#[test]
fn both_pipe_eofs_do_not_replace_process_exit() {
    let f = Fixture::new();
    let marker = f.lease_dir().join("pipes-closed");
    let mut command = python("import os,sys,time; os.close(1); os.close(2); open(sys.argv[1],'x').write('closed'); time.sleep(10)");
    command.arg(&marker);
    let start = Instant::now();
    let error = run(command, start + Duration::from_secs(2)).unwrap_err();
    let failure = error.downcast_ref::<CommandFailure>().unwrap();
    assert_eq!(std::fs::read(&marker).unwrap(), b"closed");
    assert_eq!(
        failure.error.to_string(),
        "service observation deadline expired"
    );
    assert!(failure.reaped, "{failure:?}");
    assert!(!std::path::Path::new(&format!("/proc/{}", failure.pid)).exists());
    assert!(start.elapsed() < Duration::from_secs(5));
}
