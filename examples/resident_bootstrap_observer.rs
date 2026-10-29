//! Explicit development bootstrap experiment, never an installed entry point.
//! A separately reviewed controller MUST own stock/Buddy stop, original-group
//! proof, an independent runtime bound and stock recovery before invoking this.
//! This program never issues service commands, input, navigation or mutations.
use anyhow::{ensure, Result};

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
mod native {
    use super::*;
    use remarkable_open_sdk::{native_adapter::HistoricalCapture, OperationId};
    use remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::setup::{
        capture_after_setup, visual, Binding, ObserverSetup,
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
    // Revision-4 SDKf03cc72 visual-native-library-r1 build. Earlier observers
    // remain frozen with their original spent candidates and are not replayed.
    const PROFILE: &str = "c0d8cbbb9b4618a870de348fedaedbf242b8009bf4615d67bd83d4f3c03a538d";
    const ADAPTER: &str = "c0121f577e19a484c962ad98ad8f60121e14d0695a3f9198e95978ad07895380";
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
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("observation.json"))?;
        file.write_all(&serde_json::to_vec_pretty(&value)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::File::open(root)?.sync_all()?;
        println!("evidence={} native_authority=false", root.display());
        Ok(())
    }

    struct VisualTransport<'a> {
        runtime: &'a mut ResidentRuntime,
        vendor: Option<Arc<service::vendor::OriginalCompanion>>,
    }
    impl visual::Transport for VisualTransport<'_> {
        fn progress(&mut self, deadline: Instant) -> Result<()> {
            self.runtime.with_connection(
                deadline,
                &mut || inactive_services(deadline, self.vendor.as_deref()),
                |_, progress| progress(),
            )
        }
        fn capture(
            &mut self,
            _stage: visual::Stage,
            deadline: Instant,
            guard: &mut dyn FnMut() -> Result<()>,
        ) -> Result<visual::Image> {
            visual::capture_on_runtime(self.runtime, deadline, &mut || {
                guard()?;
                inactive_services(deadline, self.vendor.as_deref())?;
                guard()
            })
        }
    }

    fn capture(
        runtime: &mut ResidentRuntime,
        mut input: Option<RequestInput>,
        setup: &mut ObserverSetup,
        nonce: &str,
        operation: OperationId,
        overall: Instant,
    ) -> Result<HistoricalCapture> {
        let binding = Binding::new(nonce, &setup.candidate_sha256, runtime.identity())?;
        let setup_deadline = overall.min(Instant::now() + Duration::from_secs(120));
        let vendor = setup.vendor();
        if setup.plan.visual_setup == Some(1) {
            setup.admit_visual(
                &binding,
                &mut input,
                setup_deadline,
                &mut VisualTransport {
                    runtime,
                    vendor: vendor.clone(),
                },
            )?;
        } else {
            runtime.with_connection(
                setup_deadline,
                &mut || inactive_services(setup_deadline, vendor.as_deref()),
                |_, progress| setup.admit(&binding, &mut input, setup_deadline, progress),
            )?;
        }
        let deadline = overall.min(Instant::now() + Duration::from_secs(10));
        let facts = setup.plan.facts()?;
        capture_after_setup(runtime, facts, operation, deadline, &mut || {
            setup.current()?;
            admit(
                input
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("missing capture guard"))?,
                deadline,
                vendor.as_deref(),
            )
        })
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
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("LANG".into(), "C".into()),
            ]),
            arguments: vec!["--system".into()],
            operation_budget_ms: 10_000,
        };
        let mut runtime = match ResidentRuntime::launch_admitted(store, launch, deadline, |end| {
            admit(&mut input, end, vendor.as_deref())
        }) {
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
            let captured = capture(
                &mut runtime,
                Some(input),
                setup,
                nonce,
                operation,
                start + Duration::from_secs(180),
            );
            // Setup and capture failures take the same owned-child cleanup path.
            let shutdown = runtime.shutdown(Instant::now() + Duration::from_secs(3));
            let result = match &captured {
                Ok(image) => serde_json::json!({"captured": true,
                    "before": format!("{:?}", image.before()), "after": format!("{:?}", image.after()),
                    "dimensions": image.dimensions(), "dpr": image.device_pixel_ratio(),
                    "sha256": image.sha256(), "png_bytes": image.png().len()}),
                Err(error) => serde_json::json!({"captured": false, "error": format!("{error:#}")}),
            };
            record(
                &root,
                serde_json::json!({"nonce": nonce, "stage": "capture",
                "candidate_sha256": setup.candidate_sha256, "runtime": format!("{identity:?}"),
                "operation": operation.0.to_string(), "result": result,
                "shutdown": format!("{shutdown:?}"), "native_authority": false}),
            )?;
            if let Ok(image) = &captured {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(root.join("capture.png"))?;
                file.write_all(image.png())?;
                file.sync_all()?;
                fs::File::open(&root)?.sync_all()?;
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
