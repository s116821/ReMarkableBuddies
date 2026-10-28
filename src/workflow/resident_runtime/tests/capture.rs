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
