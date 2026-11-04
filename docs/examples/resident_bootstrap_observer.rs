//! Explicit development bootstrap experiment, never an installed entry point.
//! A separately reviewed controller MUST own stock/Buddy stop, original-group
//! proof, an independent runtime bound and stock recovery before invoking this.
//! Ordinary builds remain observation-only. Explicit compiled development packets may
//! issue one journaled Insert OR Select; independent controller/recovery remains mandatory.
use anyhow::{Result, ensure};

fn nonce(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<String> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    ensure!(
        arguments.len() == 1,
        "Usage: resident_bootstrap_observer NONCE"
    );
    let nonce = arguments[0]
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Nonce must be UTF-8"))?;
    ensure!(
        nonce.len() == 32
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Nonce must be 32 lowercase hex characters"
    );
    Ok(nonce.into())
}

#[cfg(target_os = "linux")]
#[path = "support/resident_effect_probe.rs"]
mod effect_probe;

#[cfg(target_os = "linux")]
#[path = "support/diagnostic_pen_line.rs"]
mod diagnostic_pen_line;

#[cfg(target_os = "linux")]
#[path = "support/resident_storage_probe.rs"]
mod storage_probe;

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use anyhow::Context;
    use remarkable_open_sdk::{OperationId, native_adapter::HistoricalCapture};
    use remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::setup::{
        Binding, ObserverSetup, capture_after_setup_admitted, visual,
    };
    use remarkable_reader_buddy::{
        device::{
            request_input::{InputProgress, RequestInput},
            touch::TriggerCorner,
        },
        storage::{Store, StorePaths},
        workflow::{
            resident_runtime::{Artifact, CompanionProfile, Launch, ResidentRuntime},
            resident_supervisor::service,
        },
    };
    use std::{
        collections::BTreeMap,
        fs::{self, DirBuilder, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::Path,
        sync::Arc,
        time::{Duration, Instant},
    };

    // Reviewed read-only candidate only. A new native artifact requires a source
    // revision and review, not a command-line override of these identities.
    // Opt-in terminal-peer union retirement fix; unchanged profile/timers. Earlier observers
    // remain frozen with their original spent candidates and are not replayed.
    const PROFILE: &str = if super::effect_probe::DEVELOPMENT_EFFECT_PROBE {
        "91ba3d8c200f8bfa3898ef7410595bf4ddff62f25fba3106c7f4ed442938eb5b"
    } else {
        "c0d8cbbb9b4618a870de348fedaedbf242b8009bf4615d67bd83d4f3c03a538d"
    };
    const ADAPTER: &str = if super::effect_probe::DEVELOPMENT_EFFECT_PROBE {
        "fe52512fd08f26bc727357106a7e9e5e0bdacdd0f93c8b3a6ba1e0ee63b843fc"
    } else {
        "43ae4fd0754849e104d0120ca14d8f69b90cff2d001998ba132dc4e4c25f6c23"
    };
    const STOCK: &str = "071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df";

    fn artifact(path: impl Into<std::path::PathBuf>, hash: &str) -> Artifact {
        let mut sha256 = [0; 32];
        for (index, byte) in sha256.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hash[index * 2..index * 2 + 2], 16).expect("fixed SHA256");
        }
        Artifact {
            path: path.into(),
            sha256,
        }
    }

    fn inactive_services(
        deadline: Instant,
        vendor: Option<&service::vendor::OriginalCompanion>,
    ) -> Result<()> {
        // This prevents accidental invocation against the running legacy stack.
        // It does NOT replace the controller's original-cgroup and recovery proof.
        for unit in ["xochitl.service", "reader-buddy.service"] {
            let state = service::observe(unit, deadline)?;
            for (key, expected) in [
                ("LoadState", "loaded"),
                ("ActiveState", "inactive"),
                ("SubState", "dead"),
                ("MainPID", "0"),
                ("ControlPID", "0"),
                ("Job", ""),
            ] {
                ensure!(
                    state.property(key) == Some(expected),
                    "{unit} is not inactive: {key}"
                );
            }
            ensure!(
                state == service::observe(unit, deadline)?,
                "{unit} changed during admission"
            );
        }
        if let Some(vendor) = vendor {
            ensure!(
                vendor.stopped(deadline)?,
                "original companion is not stopped"
            );
        }
        Ok(())
    }

    fn admit(
        input: &mut RequestInput,
        deadline: Instant,
        vendor: Option<&service::vendor::OriginalCompanion>,
    ) -> Result<()> {
        ensure!(Instant::now() < deadline, "Observation deadline expired");
        ensure!(
            input.poll()? == InputProgress::Quiet,
            "Observation canceled by input"
        );
        inactive_services(deadline, vendor)?;
        ensure!(
            input.poll()? == InputProgress::Quiet,
            "Observation canceled during service check"
        );
        ensure!(Instant::now() < deadline, "Observation deadline expired");
        Ok(())
    }

    fn record(root: &Path, value: serde_json::Value) -> Result<()> {
        record_named(root, "observation.json", value)
    }

    fn record_named(root: &Path, name: &str, value: serde_json::Value) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(name))?;
        file.write_all(&serde_json::to_vec_pretty(&value)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::File::open(root)?.sync_all()?;
        println!("evidence={} native_authority=false", root.display());
        Ok(())
    }

    struct VisualTransport<'a> {
        runtime: &'a mut ResidentRuntime,
        services: Arc<remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::setup::ObserverServices>,
    }
    struct ObserverAdmission<'a> {
        services: &'a remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::setup::ObserverServices,
        guard: &'a mut dyn FnMut() -> Result<()>,
    }
    impl remarkable_reader_buddy::workflow::resident_runtime::ReadOnlyAdmission
        for ObserverAdmission<'_>
    {
        fn pulse(&mut self, deadline: Instant) -> Result<()> {
            ensure!(Instant::now() < deadline, "observer pulse expired");
            (self.guard)()?;
            self.services.pulse()?;
            (self.guard)()
        }
        fn boundary(&mut self, deadline: Instant) -> Result<()> {
            self.services.boundary(deadline, self.guard)
        }
        fn boundary_with_pulse(
            &mut self,
            deadline: Instant,
            runtime_pulse: &mut dyn FnMut() -> Result<()>,
        ) -> Result<()> {
            self.services.boundary(deadline, &mut || {
                runtime_pulse()?;
                (self.guard)()
            })
        }
    }
    impl visual::Transport for VisualTransport<'_> {
        fn progress(&mut self, deadline: Instant) -> Result<()> {
            ensure!(Instant::now() < deadline, "observer progress expired");
            self.services.pulse()?;
            self.runtime.pulse()?;
            ensure!(Instant::now() < deadline, "observer progress expired");
            Ok(())
        }
        fn boundary(&mut self, deadline: Instant) -> Result<()> {
            self.runtime.pulse()?;
            self.services
                .boundary(deadline, &mut || self.runtime.pulse())?;
            self.runtime.pulse()
        }
        fn boundary_guarded(
            &mut self,
            deadline: Instant,
            guard: &mut dyn FnMut() -> Result<()>,
        ) -> Result<()> {
            self.services.boundary(deadline, &mut || {
                guard()?;
                self.runtime.pulse()?;
                guard()
            })
        }
        fn capture(
            &mut self,
            _stage: visual::Stage,
            deadline: Instant,
            guard: &mut dyn FnMut() -> Result<()>,
        ) -> Result<visual::Image> {
            visual::capture_on_runtime_admitted(
                self.runtime,
                deadline,
                &mut ObserverAdmission {
                    services: &self.services,
                    guard,
                },
            )
        }
        fn capture_diagnosed(
            &mut self,
            stage: visual::Stage,
            deadline: Instant,
            guard: &mut dyn FnMut() -> Result<()>,
            diagnostics: &mut visual::Diagnostics,
        ) -> Result<visual::Image> {
            visual::capture_on_runtime_diagnosed_admitted(
                self.runtime,
                stage,
                deadline,
                &mut ObserverAdmission {
                    services: &self.services,
                    guard,
                },
                diagnostics,
            )
        }
    }

    fn capture(
        runtime: &mut ResidentRuntime,
        input: &mut Option<RequestInput>,
        setup: &mut ObserverSetup,
        binding: &Binding,
        operation: OperationId,
        bounds: (
            Instant,
            Instant,
            Option<(&super::effect_probe::Plan, &Store, &Path)>,
        ),
        diagnostics: Option<&mut visual::Diagnostics>,
    ) -> Result<HistoricalCapture> {
        let (overall, setup_deadline, probe) = bounds;
        let vendor = setup.vendor();
        if setup.plan.visual_setup == Some(1) {
            setup.admit_visual_diagnosed(
                binding,
                input,
                setup_deadline,
                &mut VisualTransport {
                    runtime,
                    services: setup.services(),
                },
                diagnostics.context("visual diagnostics unavailable")?,
            )?;
        } else {
            runtime.with_connection(
                setup_deadline,
                &mut || inactive_services(setup_deadline, vendor.as_deref()),
                |_, progress| setup.admit(binding, input, setup_deadline, progress),
            )?;
        }
        // Final source reconnect/API phases share the unchanged original overall
        // bound but do not consume one another's local ten-second API budget.
        let deadline = overall;
        let facts = setup.plan.facts()?;
        let services = setup.services();
        let image = capture_after_setup_admitted(
            runtime,
            facts,
            operation,
            deadline,
            &mut ObserverAdmission {
                services: &services,
                guard: &mut || {
                    setup.current()?;
                    if let Some((plan, _, _)) = probe {
                        plan.current()?;
                    }
                    ensure!(
                        input.as_mut().context("missing capture guard")?.poll()?
                            == InputProgress::Quiet,
                        "capture canceled by input"
                    );
                    ensure!(Instant::now() < deadline, "capture deadline expired");
                    Ok(())
                },
            },
        )?;
        if let Some((plan, store, root)) = probe {
            // Preserve successful source bytes before an effect can fail or cleanup block.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(root.join("capture.png"))?;
            file.write_all(image.png())?;
            file.sync_all()?;
            fs::File::open(root)?.sync_all()?;
            super::effect_probe::capture_ready(root, &image)?;
            let source = image.after().clone();
            let retained_input = std::cell::RefCell::new(input.take());
            super::effect_probe::run(
                runtime,
                store,
                plan,
                &source,
                operation,
                root,
                overall,
                &mut ObserverAdmission {
                    services: &services,
                    guard: &mut || {
                        setup.current()?;
                        plan.current()?;
                        ensure!(
                            retained_input
                                .borrow_mut()
                                .as_mut()
                                .context("missing probe input guard")?
                                .poll()?
                                == InputProgress::Quiet,
                            "probe canceled by input"
                        );
                        Ok(())
                    },
                },
                &mut |runtime, plan, deadline| {
                    ensure!(
                        image.dimensions() == [1404, 1872],
                        "logical gesture requires the declared RM2 portrait image geometry"
                    );
                    runtime.read_only_boundary(
                        deadline,
                        &mut ObserverAdmission {
                            services: &services,
                            guard: &mut || {
                                setup.current()?;
                                plan.current()?;
                                ensure!(
                                    retained_input
                                        .borrow_mut()
                                        .as_mut()
                                        .context("missing pre-gesture input guard")?
                                        .poll()?
                                        == InputProgress::Quiet,
                                    "input canceled logical navigation"
                                );
                                Ok(())
                            },
                        },
                    )?;
                    let handoff = plan.navigation_binding()?;
                    handoff.begin(deadline, &mut || {
                        setup.current()?;
                        plan.current()?;
                        services.pulse()?;
                        runtime.pulse()?;
                        ensure!(retained_input.borrow_mut().as_mut().context("missing handoff input guard")?.poll()? == InputProgress::Quiet, "input canceled before actor handoff");
                        Ok(())
                    })?;
                    // Actor acknowledged its own quiet guard release before any
                    // contact. Never label the bounded own gesture window quiet.
                    drop(retained_input.borrow_mut().take());
                    let dispatched = super::effect_probe::forward_once(deadline, &mut || {
                        setup.current()?;
                        plan.current()?;
                        services.pulse()?;
                        runtime.pulse()
                    });
                    let mut fresh = RequestInput::new(TriggerCorner::LowerLeft, None)?;
                    ensure!(
                        fresh.poll()? == InputProgress::Quiet,
                        "logical gesture did not end in a fresh quiet released input state"
                    );
                    *retained_input.borrow_mut() = Some(fresh);
                    let admitted = handoff.released(deadline, &mut || {
                        setup.current()?;
                        plan.current()?;
                        services.pulse()?;
                        runtime.pulse()?;
                        ensure!(retained_input.borrow_mut().as_mut().context("missing released input guard")?.poll()? == InputProgress::Quiet, "input canceled after own gesture");
                        Ok(())
                    });
                    match (dispatched, admitted) {
                        (Err(error), Err(admission)) => Err(error.context(format!("actor input readmission failed: {admission:#}"))),
                        (Err(error), _) | (_, Err(error)) => Err(error),
                        (Ok(()), Ok(())) => Ok(()),
                    }
                },
                &mut |runtime, plan, deadline| {
                    ensure!(
                        image.dimensions() == [1404, 1872],
                        "diagnostic pen line requires the declared RM2 portrait image geometry"
                    );
                    runtime.read_only_boundary(
                        deadline,
                        &mut ObserverAdmission {
                            services: &services,
                            guard: &mut || {
                                setup.current()?;
                                plan.current()?;
                                ensure!(
                                    retained_input
                                        .borrow_mut()
                                        .as_mut()
                                        .context("missing pre-gesture input guard")?
                                        .poll()?
                                        == InputProgress::Quiet,
                                    "input canceled diagnostic pen contact"
                                );
                                Ok(())
                            },
                        },
                    )?;
                    let handoff = plan.pen_binding()?;
                    handoff.begin(deadline, &mut || {
                        setup.current()?;
                        plan.current()?;
                        services.pulse()?;
                        runtime.pulse()?;
                        ensure!(retained_input.borrow_mut().as_mut().context("missing handoff input guard")?.poll()? == InputProgress::Quiet, "input canceled before actor handoff");
                        Ok(())
                    })?;
                    // Actor acknowledged its own quiet guard release before any
                    // contact. Never label the bounded own gesture window quiet.
                    drop(retained_input.borrow_mut().take());
                    let dispatched = super::diagnostic_pen_line::line_once(deadline, &mut || {
                        setup.current()?;
                        plan.current()?;
                        services.pulse()?;
                        runtime.pulse()
                    }, &mut |phase, value| super::effect_probe::retain_pen_events(root, plan, phase, value));
                    let mut fresh = RequestInput::new(TriggerCorner::LowerLeft, None)?;
                    ensure!(
                        fresh.poll()? == InputProgress::Quiet,
                        "diagnostic pen line did not end in a fresh quiet released input state"
                    );
                    *retained_input.borrow_mut() = Some(fresh);
                    let admitted = handoff.released(deadline, &mut || {
                        setup.current()?;
                        plan.current()?;
                        services.pulse()?;
                        runtime.pulse()?;
                        ensure!(retained_input.borrow_mut().as_mut().context("missing released input guard")?.poll()? == InputProgress::Quiet, "input canceled after own gesture");
                        Ok(())
                    });
                    match (dispatched, admitted) {
                        (Err(error), Err(admission)) => Err(error.context(format!("actor input readmission failed: {admission:#}"))),
                        (Err(error), _) | (_, Err(error)) => Err(error),
                        (Ok(()), Ok(())) => Ok(()),
                    }
                },
            )?;
        }
        Ok(image)
    }

    pub fn run(nonce: &str) -> Result<()> {
        ensure!(
            unsafe { nix::libc::geteuid() } == 0,
            "Native observation requires root"
        );
        let start = Instant::now();
        let deadline = start + Duration::from_secs(60);
        let mut capture_setup = ObserverSetup::load(nonce, deadline)?;
        let vendor = capture_setup.as_ref().and_then(ObserverSetup::vendor);
        inactive_services(deadline, vendor.as_deref())?;
        let mut input = RequestInput::new(TriggerCorner::LowerLeft, None)?;
        admit(&mut input, deadline, vendor.as_deref())?;
        let root = std::path::PathBuf::from(format!("/run/rsdk-bootstrap-{nonce}"));
        let stage = std::path::PathBuf::from(format!("/home/root/rsdk-bootstrap-{nonce}"));
        ensure!(
            fs::canonicalize("/run")? == Path::new("/run"),
            "Runtime parent changed"
        );
        let effect_plan = super::effect_probe::Plan::load(
            &stage,
            nonce,
            capture_setup
                .as_ref()
                .map(|setup| setup.candidate_sha256.as_str()),
        )?;
        ensure!(
            effect_plan.is_none() || capture_setup.is_some(),
            "effect probe requires capture setup"
        );
        DirBuilder::new().mode(0o700).create(&root)?;
        let store = Arc::new(Store::open(StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        })?);
        let launch = Launch {
            // Only the pinned, versioned candidate selects this contract.
            companion: vendor.as_ref().map(|_| CompanionProfile::rm2_3_28_0_172()),
            profile: artifact(stage.join("profile.json"), PROFILE),
            executable: artifact("/usr/bin/xochitl", STOCK),
            adapter: artifact(stage.join("libremarkable-resident.so"), ADAPTER),
            directory: root.join("session"),
            environment: BTreeMap::from([
                ("HOME".into(), "/home/root".into()),
                ("USER".into(), "root".into()),
                ("LOGNAME".into(), "root".into()),
                (
                    "PATH".into(),
                    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin".into(),
                ),
                ("LANG".into(), "en_US.UTF-8".into()),
            ]),
            arguments: vec!["--system".into()],
            operation_budget_ms: if effect_plan.is_some() {
                30_000
            } else {
                10_000
            },
        };
        let launched = if let Some(setup) = &capture_setup {
            let services = setup.services();
            ResidentRuntime::launch_read_only_admitted(
                store.clone(),
                launch,
                deadline,
                &mut ObserverAdmission {
                    services: &services,
                    guard: &mut || {
                        setup.current()?;
                        ensure!(
                            input.poll()? == InputProgress::Quiet,
                            "launch canceled by input"
                        );
                        Ok(())
                    },
                },
            )
        } else {
            ResidentRuntime::launch_admitted(store.clone(), launch, deadline, |end| {
                admit(&mut input, end, vendor.as_deref())
            })
        };
        let mut runtime = match launched {
            Ok(runtime) => runtime,
            Err(error) => {
                record(
                    &root,
                    serde_json::json!({"nonce": nonce, "stage": "startup", "error": format!("{error:#}"),
                    "shutdown": format!("{:?}", error.shutdown), "native_authority": false}),
                )?;
                return Err(error.into());
            }
        };
        let identity = runtime.identity().clone();
        let operation = OperationId(
            remarkable_open_sdk::Uuid::parse(&uuid::Uuid::new_v4().to_string())
                .expect("fresh UUID"),
        );
        if let Some(setup) = &mut capture_setup {
            let overall = start + Duration::from_secs(240);
            // Charge diagnostic construction and entry retention to this same
            // development setup window; neither can move its 180-second anchor.
            let setup_deadline = overall.min(Instant::now() + Duration::from_secs(180));
            let binding = Binding::new(nonce, &setup.candidate_sha256, runtime.identity());
            let mut diagnostics = binding
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error:#}"))
                .and_then(|binding| {
                    if setup.plan.visual_setup == Some(1) {
                        visual::Diagnostics::new(
                            &std::path::PathBuf::from(format!("/run/rsdk-control-{nonce}")),
                            binding,
                        )
                        .map(Some)
                    } else {
                        Ok(None)
                    }
                });
            let mut capture_input = Some(input);
            let probe = effect_plan
                .as_ref()
                .map(|plan| (plan, store.as_ref(), root.as_path()));
            let captured = match (&binding, &mut diagnostics) {
                (Ok(binding), Ok(Some(trace))) => {
                    trace.step(None, visual::Phase::CaptureResult, overall, |trace| {
                        capture(
                            &mut runtime,
                            &mut capture_input,
                            setup,
                            binding,
                            operation,
                            (overall, setup_deadline, probe),
                            Some(trace),
                        )
                    })
                }
                (Ok(binding), Ok(None)) => capture(
                    &mut runtime,
                    &mut capture_input,
                    setup,
                    binding,
                    operation,
                    (overall, setup_deadline, probe),
                    None,
                ),
                (_, Err(error)) => Err(anyhow::anyhow!("{error:#}")),
                (Err(error), _) => Err(anyhow::anyhow!("{error:#}")),
            };
            // Actor setup admission still requires the Reader incarnation. Once
            // shutdown retires it, PID1 may terminate this observer before return.
            // Preserve the primary refusal before that transition; pending cleanup
            // is explicit and never upgraded if its separate receipt is absent.
            let failure_retention = captured.as_ref().err().map(|error| {
                record(
                    &root,
                    serde_json::json!({"nonce": nonce, "stage": "capture",
                    "candidate_sha256": setup.candidate_sha256, "runtime": format!("{identity:?}"),
                    "operation": operation.0.to_string(),
                    "result": {"captured": false, "error": format!("{error:#}")},
                    "shutdown": "pending; separate observation.shutdown.json required for returned cleanup status",
                    "native_authority": false}),
                )
            });
            // Retention failure must never bypass owned-child cleanup.
            let shutdown_deadline = Instant::now() + Duration::from_secs(3);
            let shutdown = match &mut diagnostics {
                Ok(Some(trace)) => {
                    trace.shutdown(shutdown_deadline, || runtime.shutdown(shutdown_deadline))
                }
                _ => runtime.shutdown(shutdown_deadline),
            };
            if let Err(error) = &captured {
                let shutdown_retention = record_named(
                    &root,
                    "observation.shutdown.json",
                    serde_json::json!({"nonce": nonce, "candidate_sha256": setup.candidate_sha256,
                    "operation": operation.0.to_string(), "shutdown": format!("{shutdown:?}"),
                    "native_authority": false}),
                );
                anyhow::bail!(
                    "capture failed: {error:#}; shutdown: {shutdown:?}; primary retention: {failure_retention:?}; shutdown retention: {shutdown_retention:?}"
                );
            }
            let result = match &captured {
                Ok(image) => serde_json::json!({"captured": true,
                    "before": format!("{:?}", image.before()), "after": format!("{:?}", image.after()),
                    "dimensions": image.dimensions(), "dpr": image.device_pixel_ratio(),
                    "sha256": image.sha256(), "png_bytes": image.png().len()}),
                Err(error) => serde_json::json!({"captured": false, "error": format!("{error:#}")}),
            };
            record(
                &root,
                serde_json::json!({"nonce": nonce, "stage": if effect_plan.is_some() {"capture-and-effect-probe"} else {"capture"}, "effect_probe_selected": effect_plan.is_some(),
                "candidate_sha256": setup.candidate_sha256, "runtime": format!("{identity:?}"),
                "operation": operation.0.to_string(), "result": result,
                "shutdown": format!("{shutdown:?}"), "native_authority": false}),
            )?;
            if let Ok(image) = &captured {
                if effect_plan.is_none() {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(root.join("capture.png"))?;
                    file.write_all(image.png())?;
                    file.sync_all()?;
                    fs::File::open(&root)?.sync_all()?;
                }
            }
            ensure!(
                shutdown.reaped && shutdown.error.is_none(),
                "Owned native cleanup incomplete: {shutdown:?}"
            );
            captured?;
            return Ok(());
        }
        let observe_deadline = Instant::now() + Duration::from_secs(10);
        let observed = runtime.with_connection(
            observe_deadline,
            &mut || admit(&mut input, observe_deadline, vendor.as_deref()),
            |connection, progress| {
                connection.observe_with_progress(operation, observe_deadline, progress)
            },
        );
        // Always shut down before reporting or propagating observation/receipt errors.
        let shutdown = runtime.shutdown(Instant::now() + Duration::from_secs(3));
        record(
            &root,
            serde_json::json!({"nonce": nonce, "stage": "observe", "runtime": format!("{identity:?}"),
            "operation": operation.0.to_string(), "result": format!("{observed:?}"),
            "shutdown": format!("{shutdown:?}"), "native_authority": false}),
        )?;
        ensure!(
            shutdown.reaped && shutdown.error.is_none(),
            "Owned native cleanup incomplete: {shutdown:?}"
        );
        // A Rejected reply is a completed diagnostic, not native page readiness.
        observed?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let nonce = nonce(std::env::args_os().skip(1))?;
    #[cfg(target_os = "linux")]
    {
        native::run(&nonce)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = nonce;
        anyhow::bail!("Native bootstrap observation requires Linux")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_one_canonical_nonce_is_accepted() {
        for args in [
            vec![],
            vec!["../payload"],
            vec!["0123456789abcdef0123456789ABCDEF"],
            vec!["0123456789abcdef0123456789abcdef", "extra"],
        ] {
            assert!(nonce(args.into_iter().map(Into::into)).is_err());
        }
        assert_eq!(
            nonce(["0123456789abcdef0123456789abcdef".into()]).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
    }
}
