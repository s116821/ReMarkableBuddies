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
    use remarkable_open_sdk::OperationId;
    use remarkable_reader_buddy::{
        device::{
            request_input::{InputProgress, RequestInput},
            touch::TriggerCorner,
        },
        storage::{Store, StorePaths},
        workflow::{
            resident_runtime::{Artifact, Launch, ResidentRuntime},
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
    // Revision-3 SDK80b29eb capture-native-library-r2 build. Historical revision-2
    // observers remain frozen with their original candidates and are not replayed.
    const PROFILE: &str = "cd1d38507aedadc7df1406b5a74843ba7ab307fb51c8213311d71cab6bb0d8ae";
    const ADAPTER: &str = "3e5dee321c68384bfd042e3697dc405f6f5d0de3891bfadb541bb55efac2b21d";
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

    fn inactive_services(deadline: Instant) -> Result<()> {
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
        Ok(())
    }

    fn admit(input: &mut RequestInput, deadline: Instant) -> Result<()> {
        ensure!(Instant::now() < deadline, "Observation deadline expired");
        ensure!(
            input.poll()? == InputProgress::Quiet,
            "Observation canceled by input"
        );
        inactive_services(deadline)?;
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

    pub fn run(nonce: &str) -> Result<()> {
        ensure!(
            unsafe { nix::libc::geteuid() } == 0,
            "Native observation requires root"
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        inactive_services(deadline)?;
        let mut input = RequestInput::new(TriggerCorner::LowerLeft, None)?;
        admit(&mut input, deadline)?;
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
            admit(&mut input, end)
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
        let observe_deadline = Instant::now() + Duration::from_secs(10);
        let operation = OperationId(
            remarkable_open_sdk::Uuid::parse(&uuid::Uuid::new_v4().to_string())
                .expect("fresh UUID"),
        );
        let observed = runtime.with_connection(
            observe_deadline,
            &mut || admit(&mut input, observe_deadline),
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
