use super::*;
use crate::workflow::resident_request::{self as request, FailureKind, InputProgress, Request};

impl Fixture {
    pub(super) fn companion_profile(&self, mode: &str) -> CompanionProfile {
        let path = self.0.join("vendor-companion");
        fs::copy(fs::canonicalize("/usr/bin/python3").unwrap(), &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        CompanionProfile::fixture(
            path,
            vec![
                "-S".into(),
                "-c".into(),
                include_str!("../../../../tests/fixtures/resident_companion_child.py").into(),
                mode.into(),
                self.0.join("broker").display().to_string(),
                self.0.join("companion-evidence").display().to_string(),
                self.0.join("run").display().to_string(),
            ],
            self.0.join("broker"),
        )
    }
    fn start_companion(
        &self,
        mode: &str,
        timeout: Duration,
    ) -> std::result::Result<ResidentRuntime, StartupFailure> {
        self.launch_companion(
            self.companion_profile(mode),
            "reconnect-companion-startup",
            timeout,
        )
    }
    pub(super) fn launch_companion(
        &self,
        companion: CompanionProfile,
        reader_mode: &str,
        timeout: Duration,
    ) -> std::result::Result<ResidentRuntime, StartupFailure> {
        let mut input = self.launch();
        input.companion = Some(companion);
        input.arguments = vec![
            "-S".into(),
            "-c".into(),
            include_str!("../../../../tests/fixtures/resident_runtime_child.py").into(),
            reader_mode.into(),
            "--system".into(),
            "two words".into(),
            "".into(),
            "$(not-a-command); *".into(),
            "λ".into(),
        ];
        ResidentRuntime::launch_checked(
            self.store(),
            input,
            Instant::now() + timeout,
            |command| {
                command.env_remove("LD_PRELOAD");
            },
            |_| Ok(()),
        )
    }
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn observed_group_or_namespace_mismatch_permanently_retires_the_session() {
    for namespace in [false, true] {
        let f = Fixture::new();
        let mut runtime = f.start_companion("ready", Duration::from_secs(5)).unwrap();
        runtime
            .prepared
            .companion
            .as_mut()
            .unwrap()
            .fixture_group(Some(namespace));
        assert!(runtime
            .with_connection(
                Instant::now() + Duration::from_secs(2),
                &mut || Ok(()),
                |_, _| -> Result<()> {
                    panic!("observed membership loss must prevent request exposure")
                }
            )
            .is_err());
        // Restoring the modeled expectation cannot clear the integrity latch.
        runtime
            .prepared
            .companion
            .as_mut()
            .unwrap()
            .fixture_group(None);
        assert!(runtime
            .reconnect(Instant::now() + Duration::from_secs(2), &mut || Ok(()))
            .is_err());
        let report = runtime.shutdown(Instant::now() + Duration::from_secs(2));
        assert!(report.reaped, "{report:?}");
    }
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn two_owned_children_handshake_then_broker_ready_and_both_reap() {
    let f = Fixture::new();
    let mut runtime = f
        .start_companion("after-handshake", Duration::from_secs(5))
        .unwrap();
    let pid = runtime.prepared.companion.as_ref().unwrap().pid();
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(f.0.join("companion-evidence")).unwrap()).unwrap();
    assert_eq!(value["pid"], pid);
    assert_eq!(value["cwd"], "/");
    assert_eq!(value["nice"], 19);
    assert_eq!(value["umask"], 0o022);
    assert_eq!(value["env"]["HOME"], "/home/root");
    for key in [
        "LD_PRELOAD",
        "LD_BIND_NOW",
        "RSDK_ACTIVATION_DIRECTORY",
        "NOTIFY_SOCKET",
        "WATCHDOG_USEC",
        "JOURNAL_STREAM",
    ] {
        assert!(value["env"].get(key).is_none(), "{key}");
    }
    assert!(runtime.connection().is_err());
    runtime
        .with_connection(
            Instant::now() + Duration::from_secs(2),
            &mut || Ok(()),
            |_, check| check(),
        )
        .unwrap();
    runtime.retire_connection();
    runtime
        .reconnect(Instant::now() + Duration::from_secs(2), &mut || Ok(()))
        .unwrap();
    assert_eq!(runtime.prepared.companion.as_ref().unwrap().pid(), pid);
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(
        report.reaped && report.reader_reaped && report.error.is_none(),
        "{report:?}"
    );
    let companion = report.companion.unwrap();
    assert_eq!(companion.pid, pid);
    assert!(companion.reaped && companion.error.is_none());
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn companion_exit_and_missing_broker_fail_startup_and_reap_both() {
    for mode in ["exit", "missing"] {
        let f = Fixture::new();
        let error = match f.start_companion(mode, Duration::from_secs(5)) {
            Ok(_) => panic!("unexpected startup: {mode}"),
            Err(error) => error,
        };
        if mode == "missing" {
            assert!(f.0.join("run/handshakes").is_file(), "{error:?}");
            assert_eq!(
                fs::read(f.0.join("broker.missing-observed")).unwrap(),
                b"missing-owner"
            );
            assert!(
                error
                    .error
                    .chain()
                    .any(|cause| cause.is::<request::DeadlineExpired>()),
                "missing broker must exhaust readiness after the SDK handshake: {error:?}"
            );
        }
        let report = error
            .shutdown
            .as_ref()
            .expect("both processes were launched");
        assert!(report.reaped && report.reader_reaped, "{error:?}");
        assert!(
            report.companion.as_ref().is_some_and(|child| child.reaped),
            "{error:?}"
        );
        assert!(!Path::new(&format!("/proc/{}", report.pid)).exists());
    }
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn foreign_broker_owner_withholds_delivery_retires_and_cannot_reconnect() {
    let f = Fixture::new();
    let mut runtime = f.start_companion("ready", Duration::from_secs(5)).unwrap();
    let broker = f.0.join("broker");
    let original = fs::read(&broker).unwrap();
    let result = runtime.with_connection(
        Instant::now() + Duration::from_secs(2),
        &mut || Ok(()),
        |_, check| {
            check()?;
            fs::write(&broker, std::process::id().to_string())?;
            Ok("must not deliver")
        },
    );
    assert!(result.is_err());
    assert!(runtime.connection.is_none());
    fs::write(&broker, original).unwrap();
    assert!(runtime
        .reconnect(Instant::now() + Duration::from_secs(2), &mut || Ok(()))
        .is_err());
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(report.reaped, "{report:?}");
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn companion_death_refuses_next_request_without_dispatch() {
    let f = Fixture::new();
    let mut runtime = f.start_companion("ready", Duration::from_secs(5)).unwrap();
    let pid = runtime.prepared.companion.as_ref().unwrap().pid();
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
    // Observe death through the retained Child; a signal return is not evidence.
    let deadline = Instant::now() + Duration::from_secs(1);
    while runtime
        .prepared
        .companion
        .as_mut()
        .unwrap()
        .current()
        .is_ok()
    {
        assert!(Instant::now() < deadline);
        thread::sleep(POLL);
    }
    let mut dispatched = false;
    assert!(runtime
        .with_connection(deadline, &mut || Ok(()), |_, _| {
            dispatched = true;
            Ok(())
        })
        .is_err());
    assert!(!dispatched);
    assert!(runtime.connection.is_none());
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(report.reaped, "{report:?}");
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn companion_request_cancellation_and_deadline_allow_same_session_reconnect() {
    for canceled in [true, false] {
        let f = Fixture::new();
        let mut runtime = f.start_companion("ready", Duration::from_secs(5)).unwrap();
        let identity = runtime.identity().clone();
        let pid = runtime.prepared.companion.as_ref().unwrap().pid();
        let deadline = Instant::now() + Duration::from_millis(150);
        let mut polls = 0;
        let error = request::execute(
            &mut runtime,
            OperationId(nonce()),
            Request::Observe,
            deadline,
            &mut || {
                polls += 1;
                if polls == 3 {
                    if canceled {
                        return Ok(InputProgress::Canceled);
                    }
                    thread::sleep(
                        deadline.saturating_duration_since(Instant::now())
                            + Duration::from_millis(1),
                    );
                }
                Ok(InputProgress::Quiet)
            },
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            if canceled {
                FailureKind::Canceled
            } else {
                FailureKind::Deadline
            },
            "{error:?}"
        );
        assert_eq!(error.lost, LostRequest::NoPendingMutation);
        request::reconnect(
            &mut runtime,
            Instant::now() + Duration::from_secs(2),
            &mut || Ok(InputProgress::Quiet),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(runtime.identity(), &identity);
        assert_eq!(runtime.prepared.companion.as_ref().unwrap().pid(), pid);
        assert!(
            runtime
                .shutdown(Instant::now() + Duration::from_secs(1))
                .reaped
        );
    }
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn companion_spawn_failure_still_reaps_the_already_owned_reader() {
    let f = Fixture::new();
    let path = f.0.join("bad-vendor-executable");
    // ENOEXEC may launch a shell fallback. A missing interpreter produces an
    // actual spawn error instead, exercising the no-companion-child cleanup path.
    fs::write(&path, format!("#!{}/missing-interpreter\n", f.0.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let profile = CompanionProfile::fixture(path, Vec::new(), f.0.join("broker"));
    let failure = match f.launch_companion(profile, "reconnect", Duration::from_secs(5)) {
        Ok(_) => panic!("invalid executable unexpectedly launched"),
        Err(error) => error,
    };
    assert!(
        failure
            .error
            .to_string()
            .contains("spawn owned vendor companion"),
        "{failure:?}"
    );
    let report = failure.shutdown.unwrap();
    assert!(
        report.reaped && report.reader_reaped && report.companion.is_none(),
        "{report:?}"
    );
    assert!(!Path::new(&format!("/proc/{}", report.pid)).exists());
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn dispatched_effect_uncertainty_survives_companion_death_and_refused_reconnect() {
    use remarkable_open_sdk::{
        native_adapter::{InsertAfter, ReaderFacts},
        PageKey,
    };
    let f = Fixture::new();
    let mut runtime = f
        .launch_companion(
            f.companion_profile("ready"),
            "reconnect-pending",
            Duration::from_secs(5),
        )
        .unwrap();
    let pid = runtime.prepared.companion.as_ref().unwrap().pid();
    let page = PageKey {
        document: nonce(),
        page: nonce(),
    };
    let operation = OperationId(nonce());
    let mut killed = false;
    let error = request::execute(
        &mut runtime,
        operation,
        Request::Insert {
            conversation: StoreUuid::new_v4(),
            insertion: InsertAfter {
                source: ReaderFacts::new(page, vec![page.page]).unwrap(),
                target: nonce(),
            },
        },
        Instant::now() + Duration::from_secs(3),
        &mut || {
            if !killed && f.0.join("run/effect-received").exists() {
                assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
                killed = true;
            }
            Ok(InputProgress::Quiet)
        },
        &mut || Ok(()),
    )
    .unwrap_err();
    assert!(killed);
    assert_eq!(error.kind, FailureKind::Integrity, "{error:?}");
    assert!(
        matches!(error.lost, LostRequest::Indeterminate { operation: original, .. } if original == operation)
    );
    let journal = runtime
        .store
        .paths
        .data
        .join("native-insertion-attempts-v1.json");
    let retained = fs::read(&journal).unwrap();
    let history = resident_client::attempts(&runtime.store).unwrap();
    assert_eq!(history.len(), 1);
    assert!(!history[0].known_not_dispatched && history[0].reply.is_none());
    let reconnect = request::reconnect(
        &mut runtime,
        Instant::now() + Duration::from_secs(2),
        &mut || Ok(InputProgress::Quiet),
        &mut || Ok(()),
    )
    .unwrap_err();
    assert_eq!(reconnect.kind, FailureKind::Integrity);
    assert_eq!(reconnect.lost, error.lost);
    assert_eq!(fs::read(&journal).unwrap(), retained);
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(report.reaped, "{report:?}");
    assert_eq!(report.lost, error.lost);
}
