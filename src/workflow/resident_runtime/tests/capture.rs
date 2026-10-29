use super::*;
use crate::workflow::resident_request::{self as request, FailureKind, InputProgress, Request};
use remarkable_open_sdk::{
    native_adapter::{HistoricalCapture, ReaderFacts},
    PageKey,
};

fn end() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn quiet() -> Result<InputProgress> {
    Ok(InputProgress::Quiet)
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
fn diagnostic_capture_reconnects_after_setup_idle_retirement_without_observe_or_replay() {
    use crate::workflow::resident_supervisor::service::bootstrap::setup::capture_after_setup;
    for refusal in ["none", "input", "expired", "after-handshake"] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start("reconnect-idle", Duration::from_secs(5))
            .unwrap();
        let identity = runtime.identity().clone();
        let deadline = end();
        while !fixture.0.join("run/idle-retired").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let expected = facts();
        let result = capture_after_setup(
            &mut runtime,
            expected.clone(),
            OperationId(nonce()),
            if refusal == "expired" {
                Instant::now()
            } else {
                deadline
            },
            &mut || {
                ensure!(refusal != "input", "fresh input canceled");
                ensure!(
                    refusal != "after-handshake"
                        || fs::read_to_string(fixture.0.join("run/handshakes"))? != "2",
                    "guard canceled after reconnect"
                );
                Ok(())
            },
        );
        assert_eq!(runtime.identity(), &identity);
        if refusal == "none" {
            let image = result.unwrap();
            assert_eq!(image.before(), &expected);
            assert_eq!(fs::read(fixture.0.join("run/commands")).unwrap(), [2, 5]);
        } else {
            assert!(result.is_err(), "{refusal}");
            assert!(!fixture.0.join("run/capture-received").exists());
            assert!(!fixture.0.join("run/commands").exists());
        }
        assert_eq!(
            fs::read_to_string(fixture.0.join("run/handshakes")).unwrap(),
            if matches!(refusal, "none" | "after-handshake") {
                "2"
            } else {
                "1"
            }
        );
        assert!(resident_client::attempts(&runtime.store)
            .unwrap()
            .is_empty());
        assert!(runtime.shutdown(end()).reaped);
    }
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
fn visual_pair_then_source_use_three_explicit_connections_to_the_same_child() {
    use crate::workflow::resident_supervisor::service::bootstrap::setup::{
        capture_after_setup, visual::capture_on_runtime,
    };
    let fixture = Fixture::new();
    let mut runtime = fixture
        .start("reconnect-idle", Duration::from_secs(5))
        .unwrap();
    let identity = runtime.identity().clone();
    let deadline = end();
    while !fixture.0.join("run/idle-retired").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let before = capture_on_runtime(&mut runtime, deadline, &mut || Ok(())).unwrap();
    let after = capture_on_runtime(&mut runtime, deadline, &mut || Ok(())).unwrap();
    assert_ne!(before.operation, after.operation);
    let pixels: Vec<u8> = (0..2304u32)
        .flat_map(|i| Sha256::digest(i.to_be_bytes()).to_vec())
        .collect();
    for visual in [&before.image, &after.image] {
        assert_eq!(visual.dimensions(), [192, 128]);
        assert_eq!(
            image::load_from_memory(visual.png())
                .unwrap()
                .into_rgb8()
                .as_raw(),
            &pixels
        );
    }
    let expected = facts();
    let source = capture_after_setup(
        &mut runtime,
        expected.clone(),
        OperationId(nonce()),
        deadline,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(source.before(), &expected);
    assert_eq!(runtime.identity(), &identity);
    assert_eq!(
        fs::read(fixture.0.join("run/commands")).unwrap(),
        [2, 6, 2, 6, 2, 5]
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("run/handshakes")).unwrap(),
        "4"
    );
    assert!(resident_client::attempts(&runtime.store)
        .unwrap()
        .is_empty());
    assert!(runtime.shutdown(end()).reaped);
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
fn visual_refusal_before_dispatch_and_damaged_responses_create_no_mutation_history() {
    use crate::workflow::resident_supervisor::service::bootstrap::setup::visual::capture_on_runtime;
    for fault in ["input", "expired", "after-handshake", "corrupt", "eof"] {
        let fixture = Fixture::new();
        let mode = match fault {
            "corrupt" => "reconnect-capture-corrupt",
            "eof" => "reconnect-capture-eof",
            _ => "reconnect-idle",
        };
        let mut runtime = fixture.start(mode, Duration::from_secs(5)).unwrap();
        let identity = runtime.identity().clone();
        let result = capture_on_runtime(
            &mut runtime,
            if fault == "expired" {
                Instant::now()
            } else {
                end()
            },
            &mut || {
                ensure!(fault != "input", "fresh input canceled");
                ensure!(
                    fault != "after-handshake"
                        || fs::read_to_string(fixture.0.join("run/handshakes"))? != "2",
                    "guard canceled after reconnect"
                );
                Ok(())
            },
        );
        assert!(result.is_err(), "{fault}");
        assert_eq!(runtime.identity(), &identity);
        assert_eq!(
            fixture.0.join("run/capture-received").exists(),
            matches!(fault, "corrupt" | "eof")
        );
        assert!(resident_client::attempts(&runtime.store)
            .unwrap()
            .is_empty());
        assert!(runtime.shutdown(end()).reaped);
    }
}
fn facts() -> ReaderFacts {
    let page = PageKey {
        document: nonce(),
        page: nonce(),
    };
    ReaderFacts::new(page, vec![page.page]).unwrap()
}
fn capture(runtime: &mut ResidentRuntime, expected: ReaderFacts) -> HistoricalCapture {
    match request::execute(
        runtime,
        OperationId(nonce()),
        Request::CaptureSource(expected),
        end(),
        &mut quiet,
        &mut || Ok(()),
    )
    .unwrap()
    {
        Reply::Captured(image) => image,
        other => panic!("expected historical image, got {other:?}"),
    }
}

#[test]
fn large_capture_checks_all_pixels_without_mutation_journal_or_child_restart() {
    let fixture = Fixture::new();
    let mut runtime = fixture.start("reconnect", Duration::from_secs(5)).unwrap();
    let identity = runtime.identity().clone();
    let expected = facts();
    for _ in 0..2 {
        let image = capture(&mut runtime, expected.clone());
        assert_eq!(image.before(), &expected);
        assert_eq!(image.after(), &expected);
        assert_eq!(image.dimensions(), [192, 128]);
        assert!(image.png().len() > 65_536);
        let decoded = image::load_from_memory(image.png()).unwrap().into_rgb8();
        let pixels: Vec<u8> = (0..2304u32)
            .flat_map(|i| Sha256::digest(i.to_be_bytes()).to_vec())
            .collect();
        assert_eq!(decoded.as_raw(), &pixels);
        assert!(resident_client::attempts(&runtime.store)
            .unwrap()
            .is_empty());
        assert_eq!(runtime.identity(), &identity);
        request::reconnect(&mut runtime, end(), &mut quiet, &mut || Ok(())).unwrap();
    }
    assert!(runtime.shutdown(end()).reaped);
}

#[test]
fn capture_refuses_corruption_source_mismatch_and_truncation() {
    for mode in [
        "reconnect-capture-corrupt",
        "reconnect-capture-source",
        "reconnect-capture-eof",
    ] {
        let fixture = Fixture::new();
        let mut runtime = fixture.start(mode, Duration::from_secs(5)).unwrap();
        let failure = request::execute(
            &mut runtime,
            OperationId(nonce()),
            Request::CaptureSource(facts()),
            end(),
            &mut quiet,
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            failure.kind,
            if mode == "reconnect-capture-eof" {
                FailureKind::Connection
            } else {
                FailureKind::Integrity
            },
            "{mode}: {failure:?}"
        );
        assert_eq!(failure.lost, LostRequest::NoPendingMutation);
        assert!(runtime.connection.is_none());
        assert!(resident_client::attempts(&runtime.store)
            .unwrap()
            .is_empty());
        assert!(runtime.shutdown(end()).reaped);
    }
}

#[test]
fn capture_cancellation_and_deadline_allow_explicit_same_child_reconnect() {
    for mode in ["reconnect-capture-cancel", "reconnect-capture-timeout"] {
        let fixture = Fixture::new();
        let mut runtime = fixture.start(mode, Duration::from_secs(5)).unwrap();
        let identity = runtime.identity().clone();
        let expected = facts();
        let failure = request::execute(
            &mut runtime,
            OperationId(nonce()),
            Request::CaptureSource(expected.clone()),
            if mode.ends_with("timeout") {
                Instant::now() + Duration::from_millis(300)
            } else {
                end()
            },
            &mut || {
                Ok(
                    if mode.ends_with("cancel") && fixture.0.join("run/capture-received").exists() {
                        InputProgress::Canceled
                    } else {
                        InputProgress::Quiet
                    },
                )
            },
            &mut || Ok(()),
        )
        .unwrap_err();
        assert!(fixture.0.join("run/capture-received").exists());
        assert_eq!(
            failure.kind,
            if mode.ends_with("cancel") {
                FailureKind::Canceled
            } else {
                FailureKind::Deadline
            },
            "{failure:?}"
        );
        assert_eq!(failure.lost, LostRequest::NoPendingMutation);
        runtime.check_child().unwrap();
        request::reconnect(&mut runtime, end(), &mut quiet, &mut || Ok(())).unwrap();
        assert_eq!(runtime.identity(), &identity);
        assert_eq!(capture(&mut runtime, expected.clone()).before(), &expected);
        assert!(resident_client::attempts(&runtime.store)
            .unwrap()
            .is_empty());
        assert!(runtime.shutdown(end()).reaped);
    }
}

#[cfg(feature = "development-resident-bootstrap")]
#[test]
#[ignore = "protected root diagnostic evidence; run in the owned Linux root container"]
fn diagnosed_visual_api_records_reconnect_and_failure_before_owned_shutdown() {
    use crate::workflow::resident_supervisor::service::bootstrap::setup::{
        visual::{capture_on_runtime_diagnosed, Diagnostics, Phase, Stage},
        Binding,
    };
    for fault in ["input", "corrupt"] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start(
                if fault == "corrupt" {
                    "reconnect-capture-corrupt"
                } else {
                    "reconnect-idle"
                },
                Duration::from_secs(5),
            )
            .unwrap();
        let root = PathBuf::from(format!(
            "/root/visual-runtime-diagnostics-{}",
            StoreUuid::new_v4()
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        let binding = Binding::new(
            "0123456789abcdef0123456789abcdef",
            &"11".repeat(32),
            runtime.identity(),
        )
        .unwrap();
        let mut trace = Diagnostics::new(&root, &binding).unwrap();
        let records = || {
            let mut paths: Vec<_> = fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            paths.sort();
            paths
                .into_iter()
                .map(|path| {
                    serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()
                })
                .collect::<Vec<_>>()
        };
        let result = trace.step(None, Phase::CaptureResult, end(), |trace| {
            capture_on_runtime_diagnosed(
                &mut runtime,
                Stage::Before,
                end(),
                &mut || {
                    ensure!(fault != "input", "guard refused before reconnect");
                    Ok(())
                },
                trace,
            )
        });
        assert!(result.is_err());
        let retained = records();
        assert_eq!(retained.last().unwrap()["phase"], "capture-result");
        assert_eq!(retained.last().unwrap()["outcome"], "error");
        assert!(retained.iter().any(|record| record["phase"] == "reconnect"));
        assert_eq!(
            retained
                .iter()
                .any(|record| record["phase"] == "capture-api"),
            fault == "corrupt"
        );
        assert_eq!(
            fixture.0.join("run/capture-received").exists(),
            fault == "corrupt"
        );
        let store = runtime.store.clone();
        let shutdown = trace.shutdown(end(), || {
            assert_eq!(records().last().unwrap()["phase"], "shutdown");
            assert_eq!(records()[..retained.len()], retained);
            runtime.shutdown(end())
        });
        assert!(shutdown.reaped);
        assert!(resident_client::attempts(&store).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
