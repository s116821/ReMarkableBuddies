use super::*;
use crate::storage::{StorePaths, Uuid as StoreUuid};
use remarkable_open_sdk::native_adapter::{Rejection, Reply};

mod requests;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdk-{}", StoreUuid::new_v4().simple()));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        for name in ["profile", "adapter"] {
            let mut file = create_private(&path.join(name)).unwrap();
            file.write_all(name.as_bytes()).unwrap();
        }
        Self(path)
    }
    fn store(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        )
    }
    fn launch(&self) -> Launch {
        let artifact = |path: PathBuf| Artifact {
            sha256: Sha256::digest(fs::read(&path).unwrap()).into(),
            path,
        };
        Launch {
            profile: artifact(self.0.join("profile")),
            executable: artifact(
                fs::canonicalize("/usr/bin/python3")
                    .expect("Linux runtime fixtures require Python3"),
            ),
            adapter: artifact(self.0.join("adapter")),
            directory: self.0.join("run"),
            environment: BTreeMap::from([("LANG".into(), "C".into())]),
            arguments: Vec::new(),
            operation_budget_ms: 1000,
        }
    }
    fn start(
        &self,
        mode: &str,
        timeout: Duration,
    ) -> std::result::Result<ResidentRuntime, StartupFailure> {
        self.start_admitted(mode, timeout, |_| Ok(()))
    }
    fn start_admitted(
        &self,
        mode: &str,
        timeout: Duration,
        admit: impl FnMut(Instant) -> Result<()>,
    ) -> std::result::Result<ResidentRuntime, StartupFailure> {
        let mut input = self.launch();
        input.arguments = [
            "-S",
            "-c",
            include_str!("../../../tests/fixtures/resident_runtime_child.py"),
            mode,
            "--system",
            "two words",
            "",
            "$(not-a-command); *",
            "λ",
        ]
        .map(str::to_owned)
        .to_vec();
        ResidentRuntime::launch_checked(
            self.store(),
            input,
            Instant::now() + timeout,
            |command| {
                // Owned protocol fixture, explicitly no actual LD_PRELOAD/native code.
                command.env_remove("LD_PRELOAD");
            },
            admit,
        )
    }
    fn preserved_and_reaped(&self, mode: &str, failure: StartupFailure) {
        let report = failure.shutdown.as_ref().unwrap_or_else(|| {
            panic!("post-spawn scenario {mode} failed before launch: {failure:?}")
        });
        assert!(report.reaped, "{report:?}");
        assert!(!Path::new(&format!("/proc/{}", report.pid)).exists());
        for name in [
            "admission.bin",
            "stdout.log",
            "stderr.log",
            "shutdown.txt",
            "reader",
            "adapter.so",
            "profile.json",
        ] {
            assert!(self.0.join("run").join(name).is_file(), "missing {name}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn staged_socket_wait_services_admission_and_connects_only_after_listen() {
    let fixture = Fixture::new();
    let mut staged_checks = 0;
    let runtime = fixture
        .start_admitted("delayed-listen", Duration::from_secs(5), |_| {
            if fixture.0.join("run/sdk.pending").exists() && staged_checks < 3 {
                assert!(!fixture.0.join("run/sdk.sock").exists());
                assert!(!fixture.0.join("run/handshakes").exists());
                staged_checks += 1;
                if staged_checks == 3 {
                    fs::write(fixture.0.join("run/permit-listen"), b"go").unwrap();
                }
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(staged_checks, 3);
    assert_eq!(
        fs::read_to_string(fixture.0.join("run/handshakes")).unwrap(),
        "1"
    );
    assert!(!fixture.0.join("run/sdk.pending").exists());
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(report.reaped && report.error.is_none(), "{report:?}");
}

#[test]
fn real_child_consumes_exact_admission_connects_once_and_is_reaped() {
    let fixture = Fixture::new();
    let mut runtime = fixture.start("ready", Duration::from_secs(5)).unwrap();
    let identity = runtime.identity().clone();
    assert_ne!(identity.pid, std::process::id());
    assert_eq!(process_start(identity.pid).unwrap(), identity.start_ticks);
    let reply = runtime
        .connection()
        .unwrap()
        .observe(
            OperationId(nonce()),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(reply, Reply::Rejected(Rejection::Unsupported));
    let root = runtime.evidence_directory().to_owned();
    assert_eq!(fs::metadata(&root).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(
        fs::metadata(root.join("admission.bin")).unwrap().mode() & 0o7777,
        0o600
    );
    assert_eq!(
        fs::metadata(root.join("reader")).unwrap().mode() & 0o7777,
        0o500
    );
    assert_eq!(
        fs::metadata(root.join("adapter.so")).unwrap().mode() & 0o7777,
        0o400
    );
    assert_eq!(fs::metadata(root.join("admission.bin")).unwrap().nlink(), 2);
    assert_eq!(fs::read(root.join("handshakes")).unwrap(), b"1");
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(report.reaped && report.error.is_none(), "{report:?}");
    assert_eq!(report.lost, LostRequest::NoPendingMutation);
    assert!(!Path::new(&format!("/proc/{}", identity.pid)).exists());
    assert!(root.join("admission.consumed").exists());
    assert!(
        fixture.start("ready", Duration::from_secs(5)).is_err(),
        "never reuse consumed directory"
    );
}

#[test]
fn reconnect_retains_child_and_admission_after_idle_eof_and_cancellation() {
    use remarkable_open_sdk::native_adapter::transport::Failure;
    use resident_client::ConnectionFailure;
    let fixture = Fixture::new();
    let mut runtime = fixture
        .start("reconnect-idle", Duration::from_secs(5))
        .unwrap();
    let original = runtime.identity().clone();
    let admission = fs::metadata(fixture.0.join("run/admission.consumed")).unwrap();
    let end = || Instant::now() + Duration::from_secs(2);
    let marker_deadline = end();
    while !fixture.0.join("run/idle-retired").exists() {
        before(marker_deadline).unwrap();
        thread::sleep(POLL);
    }
    let error = runtime
        .connection()
        .unwrap()
        .observe(OperationId(nonce()), end())
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ConnectionFailure>().unwrap().cause,
        Failure::Io(_)
    ));
    runtime.reconnect(end(), &mut || Ok(())).unwrap();
    assert_eq!(runtime.identity(), &original);
    assert_eq!(fs::read(fixture.0.join("run/handshakes")).unwrap(), b"2");
    assert!(same_file(
        &admission,
        &fs::metadata(fixture.0.join("run/admission.consumed")).unwrap()
    ));
    // Cancellation retires the socket, never the process or consumed activation.
    let refused = runtime
        .reconnect(end(), &mut || anyhow::bail!("request canceled"))
        .unwrap_err();
    assert!(matches!(
        refused.downcast_ref::<ConnectionFailure>().unwrap().cause,
        Failure::Progress(_)
    ));
    assert!(runtime.connection.is_none());
    runtime.check_child().unwrap();
    runtime.reconnect(end(), &mut || Ok(())).unwrap();
    assert_eq!(runtime.identity(), &original);
    assert_eq!(fs::read(fixture.0.join("run/handshakes")).unwrap(), b"3");
    assert_eq!(
        runtime
            .connection()
            .unwrap()
            .observe(OperationId(nonce()), end())
            .unwrap(),
        Reply::Rejected(Rejection::Unsupported)
    );
    let report = runtime.shutdown(end());
    assert!(report.reaped && report.error.is_none(), "{report:?}");
}

#[test]
fn reconnect_refuses_changed_artifacts_or_consumption_before_handshake() {
    for name in ["adapter.so", "admission.consumed"] {
        let fixture = Fixture::new();
        let mut runtime = fixture.start("reconnect", Duration::from_secs(5)).unwrap();
        let path = fixture.0.join("run").join(name);
        fs::rename(&path, path.with_extension("original")).unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(runtime
            .reconnect(Instant::now() + Duration::from_secs(2), &mut || Ok(()))
            .is_err());
        assert_eq!(fs::read(fixture.0.join("run/handshakes")).unwrap(), b"1");
        assert!(runtime.connection.is_none());
        runtime.check_child().unwrap();
        assert!(
            runtime
                .shutdown(Instant::now() + Duration::from_secs(1))
                .reaped
        );
    }
}

#[test]
fn reconnect_preserves_dispatched_cancellation_through_later_observation_failure() {
    use remarkable_open_sdk::{
        native_adapter::{transport::Failure, ReaderFacts, SelectAdjacent},
        PageKey,
    };
    let fixture = Fixture::new();
    let mut runtime = fixture
        .start("reconnect-pending", Duration::from_secs(5))
        .unwrap();
    let identity = runtime.identity().clone();
    let end = || Instant::now() + Duration::from_secs(2);
    let page = PageKey {
        document: nonce(),
        page: nonce(),
    };
    let selection = SelectAdjacent {
        expected: ReaderFacts::new(page, vec![page.page]).unwrap(),
        target: page.page,
    };
    let original = OperationId(nonce());
    let failure = runtime
        .connection()
        .unwrap()
        .select_adjacent_with_progress(
            StoreUuid::new_v4(),
            original,
            selection.clone(),
            end(),
            &mut || {
                ensure!(
                    !fixture.0.join("run/effect-received").exists(),
                    "request canceled after dispatch"
                );
                Ok(())
            },
        )
        .unwrap_err();
    assert!(matches!(
        failure.cause,
        resident_client::EffectCause::Transport(Failure::Progress(_))
    ));
    assert!(
        matches!(failure.lost, LostRequest::Indeterminate { operation, .. } if operation == original)
    );
    runtime.reconnect(end(), &mut || Ok(())).unwrap();
    assert_eq!(runtime.identity(), &identity);
    let error = runtime
        .connection()
        .unwrap()
        .observe_with_progress(OperationId(nonce()), end(), &mut || {
            anyhow::bail!("second request canceled")
        })
        .unwrap_err();
    let typed = error
        .downcast_ref::<resident_client::ConnectionFailure>()
        .unwrap();
    assert!(matches!(typed.cause, Failure::Progress(_)));
    assert_eq!(typed.lost, failure.lost);
    runtime.reconnect(end(), &mut || Ok(())).unwrap();
    let refused = runtime
        .connection()
        .unwrap()
        .select_adjacent(StoreUuid::new_v4(), OperationId(nonce()), selection, end())
        .unwrap_err();
    assert!(matches!(
        refused.cause,
        resident_client::EffectCause::Unresolved
    ));
    assert_eq!(refused.lost, failure.lost);
    assert_eq!(
        runtime
            .connection()
            .unwrap()
            .observe(OperationId(nonce()), end())
            .unwrap(),
        Reply::Rejected(Rejection::Unsupported)
    );
    let history = resident_client::attempts(&runtime.store).unwrap();
    assert_eq!(history.len(), 1);
    assert!(!history[0].known_not_dispatched && history[0].reply.is_none());
    let report = runtime.shutdown(end());
    assert!(report.reaped && report.error.is_none(), "{report:?}");
    assert_eq!(report.lost, failure.lost);
}

#[test]
fn reconnect_carries_reply_retention_uncertainty_into_new_conversations_and_shutdown() {
    use remarkable_open_sdk::{
        native_adapter::{ReaderFacts, SelectAdjacent},
        PageKey,
    };
    for after in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start("reconnect-selection", Duration::from_secs(5))
            .unwrap();
        let identity = runtime.identity().clone();
        let end = || Instant::now() + Duration::from_secs(2);
        let page = PageKey {
            document: nonce(),
            page: nonce(),
        };
        let selection = SelectAdjacent {
            expected: ReaderFacts::new(page, vec![page.page]).unwrap(),
            target: page.page,
        };
        resident_client::fail_next_outcome_publication(after);
        let failure = runtime
            .connection()
            .unwrap()
            .select_adjacent(
                StoreUuid::new_v4(),
                OperationId(nonce()),
                selection.clone(),
                end(),
            )
            .unwrap_err();
        assert!(matches!(
            failure.cause,
            resident_client::EffectCause::Retention
        ));
        let canceled = runtime
            .reconnect(end(), &mut || anyhow::bail!("reconnect canceled"))
            .unwrap_err();
        let typed = canceled
            .downcast_ref::<resident_client::ConnectionFailure>()
            .unwrap();
        assert!(matches!(
            typed.cause,
            remarkable_open_sdk::native_adapter::transport::Failure::Progress(_)
        ));
        assert_eq!(typed.lost, failure.lost);
        assert!(runtime.connection.is_none());
        runtime.check_child().unwrap();
        runtime.reconnect(end(), &mut || Ok(())).unwrap();
        assert_eq!(runtime.identity(), &identity);
        let refused = runtime
            .connection()
            .unwrap()
            .select_adjacent(StoreUuid::new_v4(), OperationId(nonce()), selection, end())
            .unwrap_err();
        assert!(matches!(
            refused.cause,
            resident_client::EffectCause::Unresolved
        ));
        assert_eq!(refused.lost, failure.lost);
        // Observation still works and proves the refused effect sent no bytes.
        assert_eq!(
            runtime
                .connection()
                .unwrap()
                .observe(OperationId(nonce()), end())
                .unwrap(),
            Reply::Rejected(Rejection::Unsupported)
        );
        assert_eq!(resident_client::attempts(&runtime.store).unwrap().len(), 1);
        let report = runtime.shutdown(end());
        assert!(report.reaped && report.error.is_none(), "{report:?}");
        assert_eq!(report.lost, failure.lost);
    }
}

#[test]
fn final_admission_after_preparation_refuses_before_any_child_is_spawned() {
    for mode in ["canceled", "expired", "replaced"] {
        let fixture = Fixture::new();
        let mut called = 0;
        let failure = fixture
            .start_admitted("ready", Duration::from_secs(2), |deadline| {
                called += 1;
                // Admission is after all copied files and the synchronized record.
                for name in ["admission.bin", "reader", "adapter.so", "profile.json"] {
                    assert!(fixture.0.join("run").join(name).is_file(), "{name}");
                }
                match mode {
                    "canceled" => anyhow::bail!("source canceled during preparation"),
                    "expired" => thread::sleep(
                        deadline.saturating_duration_since(Instant::now())
                            + Duration::from_millis(1),
                    ),
                    _ => {
                        let file = fixture.0.join("run/profile.json");
                        fs::rename(&file, fixture.0.join("original-profile")).unwrap();
                        fs::write(file, b"replacement").unwrap();
                    }
                }
                Ok(())
            })
            .err()
            .expect("final admission must refuse");
        assert_eq!(called, 1);
        assert!(failure.shutdown.is_none(), "{failure:?}");
        assert!(!fixture.0.join("run/sdk.sock").exists());
        assert_eq!(
            fs::metadata(fixture.0.join("run/stdout.log"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            fs::metadata(fixture.0.join("run/stderr.log"))
                .unwrap()
                .len(),
            0
        );
        if mode == "canceled" {
            assert!(failure.error.to_string().contains("source canceled"));
        }
        if mode == "expired" {
            assert!(failure.error.to_string().contains("deadline"));
        }
    }
}

#[test]
fn admitted_prepared_child_still_uses_normal_handshake_and_owned_cleanup() {
    let fixture = Fixture::new();
    let mut admissions = 0;
    let runtime = fixture
        .start_admitted("ready", Duration::from_secs(5), |_| {
            admissions += 1;
            assert!(fixture.0.join("run/admission.bin").is_file());
            if admissions == 1 {
                assert!(!fixture.0.join("run/sdk.sock").exists());
            }
            Ok(())
        })
        .unwrap();
    assert!(admissions > 1);
    let stopped = runtime.shutdown(Instant::now() + Duration::from_secs(2));
    assert!(stopped.reaped && stopped.error.is_none());
}

#[test]
fn startup_progress_cancellation_reaps_before_socket_or_during_handshake() {
    for mode in ["no-socket", "late-ready"] {
        let fixture = Fixture::new();
        let mut calls = 0;
        let mut original_deadline = None;
        let failure = fixture
            .start_admitted(mode, Duration::from_secs(5), |deadline| {
                assert_eq!(*original_deadline.get_or_insert(deadline), deadline);
                calls += 1;
                if (mode == "no-socket" && calls > 1) || fixture.0.join("run/handshakes").exists() {
                    anyhow::bail!("owned input cancellation during startup");
                }
                Ok(())
            })
            .err()
            .expect("startup cancellation must refuse");
        assert!(calls > 1);
        assert!(failure
            .error
            .to_string()
            .contains("owned input cancellation"));
        if mode == "late-ready" {
            assert!(fixture.0.join("run/handshakes").exists());
        } else {
            assert!(!fixture.0.join("run/sdk.sock").exists());
        }
        fixture.preserved_and_reaped(mode, failure);
    }
}

#[test]
fn startup_faults_keep_evidence_and_stop_only_the_owned_child() {
    for mode in [
        "exit",
        "no-socket",
        "late-ready",
        "no-consume",
        "replaced-admission",
        "bad-ready",
    ] {
        let fixture = Fixture::new();
        let started = Instant::now();
        // This includes hashing/copying/synchronizing the real interpreter.
        // Keep setup margin while the timeout fixtures still delay for 10s.
        let result = fixture.start(mode, Duration::from_secs(3));
        let failure = match result {
            Err(error) => error,
            Ok(_) => panic!("unexpected success: {mode}"),
        };
        fixture.preserved_and_reaped(mode, failure);
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "startup/cleanup must not renew: {mode}"
        );
        assert!(Path::new(&format!("/proc/{}", std::process::id())).exists());
    }
}

#[test]
fn expired_startup_deadline_refuses_before_directory_or_child() {
    let fixture = Fixture::new();
    let failure = ResidentRuntime::launch(fixture.store(), fixture.launch(), Instant::now())
        .err()
        .expect("expired deadline must refuse");
    assert!(failure.shutdown.is_none(), "{failure:?}");
    assert!(!fixture.0.join("run").exists());
}

#[test]
fn wrong_artifact_and_loader_environment_refuse_before_launch() {
    let fixture = Fixture::new();
    let mut input = fixture.launch();
    input.adapter.sha256 = [1; 32];
    let failure = ResidentRuntime::launch(
        fixture.store(),
        input,
        Instant::now() + Duration::from_secs(5),
    )
    .err()
    .unwrap();
    assert!(failure.shutdown.is_none());
    assert!(!fixture.0.join("run/admission.bin").exists());
    for key in [
        "LD_AUDIT",
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "QML_IMPORT_PATH",
        "QT_PLUGIN_PATH",
        "NOTIFY_SOCKET",
        "WATCHDOG_USEC",
        "WATCHDOG_PID",
    ] {
        let fixture = Fixture::new();
        let mut input = fixture.launch();
        input.environment.insert(key.into(), "/unapproved".into());
        assert!(ResidentRuntime::launch(
            fixture.store(),
            input,
            Instant::now() + Duration::from_secs(5)
        )
        .is_err());
        assert!(!fixture.0.join("run").exists());
    }
}

#[test]
fn source_replacement_after_copy_cannot_change_launched_bytes() {
    let fixture = Fixture::new();
    let input = fixture.launch();
    let adapter_hash = input.adapter.sha256;
    let mut prepared = Prepared::new(input, Instant::now() + Duration::from_secs(5)).unwrap();
    fs::rename(fixture.0.join("adapter"), fixture.0.join("old-adapter")).unwrap();
    fs::write(fixture.0.join("adapter"), b"replacement").unwrap();
    prepared.current(false).unwrap();
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(
            fs::read(&prepared.artifacts[2].path).unwrap()
        )),
        adapter_hash
    );
    assert_eq!(
        prepared.command.get_program(),
        prepared.artifacts[1].path.as_os_str()
    );
    assert!(prepared
        .command
        .get_envs()
        .all(|(name, _)| name != "LD_AUDIT" && name != "LD_LIBRARY_PATH"));
    // Replacement of a private launch copy, unlike source replacement, refuses.
    fs::rename(
        fixture.0.join("run/adapter.so"),
        fixture.0.join("run/old-adapter.so"),
    )
    .unwrap();
    fs::write(fixture.0.join("run/adapter.so"), b"replacement").unwrap();
    assert!(prepared.current(false).is_err());
}

#[test]
fn dropping_the_owner_closes_and_reaps_the_child() {
    let fixture = Fixture::new();
    let runtime = fixture.start("ready", Duration::from_secs(5)).unwrap();
    let pid = runtime.identity().pid;
    drop(runtime);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(fixture.0.join("run/shutdown.txt").is_file());
}

#[test]
fn effect_publication_loss_stays_original_through_close_and_runtime_shutdown() {
    use remarkable_open_sdk::{
        native_adapter::{ReaderFacts, SelectAdjacent},
        PageKey,
    };
    for after in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture.start("selection", Duration::from_secs(5)).unwrap();
        let page = PageKey {
            document: nonce(),
            page: nonce(),
        };
        let selection = SelectAdjacent {
            expected: ReaderFacts::new(page, vec![page.page]).unwrap(),
            target: page.page,
        };
        let original = OperationId(nonce());
        resident_client::fail_next_outcome_publication(after);
        let failure = runtime
            .connection()
            .unwrap()
            .select_adjacent(
                StoreUuid::new_v4(),
                original,
                selection.clone(),
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap_err();
        assert!(
            matches!(failure.lost, LostRequest::Indeterminate { operation, .. } if operation == original)
        );
        assert_eq!(runtime.connection.as_mut().unwrap().close(), failure.lost);
        let repeated = runtime
            .connection
            .as_mut()
            .unwrap()
            .select_adjacent(
                StoreUuid::new_v4(),
                OperationId(nonce()),
                selection,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap_err();
        assert_eq!(repeated.lost, failure.lost);
        let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
        assert!(report.reaped, "{report:?}");
        assert_eq!(report.lost, failure.lost);
        let attempts = resident_client::attempts(&fixture.store()).unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].request.frame().operation, original);
        assert_eq!(attempts[0].reply.is_some(), after);
    }
}

#[test]
fn preload_separators_and_excessive_deadline_refuse_before_directory_creation() {
    for name in [
        "space name",
        "colon:name",
        "tab\tname",
        "trailing/",
        "$ORIGIN",
        "${LIB}",
    ] {
        let fixture = Fixture::new();
        let mut input = fixture.launch();
        input.directory = fixture.0.join(name);
        assert!(ResidentRuntime::launch(
            fixture.store(),
            input,
            Instant::now() + Duration::from_secs(5)
        )
        .is_err());
        assert!(!fixture.0.join(name).exists());
    }
    let fixture = Fixture::new();
    assert!(ResidentRuntime::launch(
        fixture.store(),
        fixture.launch(),
        Instant::now() + Duration::from_secs(61)
    )
    .is_err());
    assert!(!fixture.0.join("run").exists());
}

#[test]
fn replaced_directory_refuses_connection_and_cleanup_stays_in_the_owned_inode() {
    let fixture = Fixture::new();
    let mut runtime = fixture.start("ready", Duration::from_secs(5)).unwrap();
    let old = fixture.0.join("original-run");
    fs::rename(fixture.0.join("run"), &old).unwrap();
    DirBuilder::new()
        .mode(0o700)
        .create(fixture.0.join("run"))
        .unwrap();
    assert!(runtime.connection().is_err());
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(1));
    assert!(report.reaped && report.error.is_none(), "{report:?}");
    assert!(old.join("shutdown.txt").exists());
    assert_eq!(fs::read_dir(fixture.0.join("run")).unwrap().count(), 0);
}

#[test]
fn invalid_arguments_refuse_before_directory_creation() {
    for arguments in [
        vec!["embedded\0nul".to_owned()],
        vec![String::new(); 17],
        vec!["a".repeat(16 * 1024 + 1)],
        vec!["λ".repeat(8 * 1024 + 1)],
    ] {
        let fixture = Fixture::new();
        let mut input = fixture.launch();
        input.arguments = arguments;
        let error = ResidentRuntime::launch(
            fixture.store(),
            input,
            Instant::now() + Duration::from_secs(5),
        )
        .err()
        .expect("invalid arguments refuse");
        assert!(error.shutdown.is_none());
        assert!(!fixture.0.join("run").exists());
    }
}
