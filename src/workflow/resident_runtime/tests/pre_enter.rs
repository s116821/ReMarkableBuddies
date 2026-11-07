//! Real Unix peer/wrapper tests; no native handle, input or tablet qualification.
use super::*;

struct Admission {
    original: Instant,
    boundary: usize,
    entered: bool,
    received: bool,
    retained: Vec<(bool, bool)>,
    fault: &'static str,
    received_path: PathBuf,
}
impl ReadOnlyAdmission for Admission {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        assert_eq!(
            deadline, self.original,
            "original absolute deadline changed"
        );
        if (self.fault == "progress" && self.received_path.exists())
            || (self.fault == "closing" && self.received)
        {
            anyhow::bail!("fixture ownership lost");
        }
        Ok(())
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.pulse(deadline)?;
        self.boundary += 1;
        if self.fault == "boundary" {
            anyhow::bail!("fixture opening admission lost");
        }
        Ok(())
    }
    fn before_api(&mut self, deadline: Instant) -> Result<()> {
        assert_eq!(deadline, self.original);
        self.entered = true;
        if self.fault == "entry" {
            anyhow::bail!("fixture entry refused");
        }
        Ok(())
    }
    fn reply_observed(&mut self) {
        self.received = true;
    }
    fn after_api(
        &mut self,
        deadline: Instant,
        invoked: bool,
        result: &Result<Reply>,
    ) -> Result<()> {
        assert_eq!(deadline, self.original);
        self.retained.push((invoked, result.is_ok()));
        if self.fault == "retention" || self.fault == "both" {
            anyhow::bail!("fixture retention failed");
        }
        Ok(())
    }
}

fn run(mode: &str, offset: u32, count: u16, fault: &'static str, expected: Option<Reply>) {
    let fixture = Fixture::new();
    let mut runtime = fixture.start(mode, Duration::from_secs(5)).unwrap();
    let deadline = if fault == "expired" {
        Instant::now() - Duration::from_millis(1)
    } else {
        Instant::now()
            + if mode.ends_with("-wait") {
                Duration::from_millis(120)
            } else {
                Duration::from_secs(3)
            }
    };
    let mut admission = Admission {
        original: deadline,
        boundary: 0,
        entered: false,
        received: false,
        retained: vec![],
        fault,
        received_path: fixture.0.join("run/pre-enter-requests"),
    };
    let operation = OperationId(nonce());
    let result = runtime.request_read_only(
        operation,
        ReadOnlyRequest::CapturePreEnter { offset, count },
        deadline,
        &mut admission,
    );
    if let Some(expected) = expected {
        // Reply has no PartialEq. Narrow exactly; never decode/normalize units.
        match (result.unwrap(), expected) {
            (Reply::PreEnterText(actual), Reply::PreEnterText(expected)) => {
                assert_eq!(actual, expected)
            }
            (Reply::Rejected(actual), Reply::Rejected(expected)) => assert_eq!(actual, expected),
            (actual, expected) => panic!("{actual:?} != {expected:?}"),
        }
        assert_eq!(admission.boundary, 3);
        assert_eq!(admission.retained, [(true, true)]);
    } else {
        let error = result.unwrap_err();
        if fault == "both" {
            assert!(format!("{error:#}").contains("API diagnostic retention failed"));
            assert!(error
                .downcast_ref::<resident_client::ConnectionFailure>()
                .is_some());
        }
        match fault {
            "boundary" | "expired" => {
                assert!(!admission.entered);
                assert!(admission.retained.is_empty());
            }
            "entry" => assert_eq!(admission.retained, [(false, false)]),
            "retention" => assert_eq!(admission.retained, [(true, true)]),
            _ => assert_eq!(admission.retained, [(true, false)]),
        }
    }
    let requests = fs::read(&admission.received_path).unwrap_or_default();
    assert!(
        requests.is_empty() || requests.len() == 22,
        "replayed request"
    );
    if matches!(fault, "expired" | "boundary" | "entry" | "invalid") {
        assert!(requests.is_empty(), "refused request reached the peer");
    } else {
        assert_eq!(
            requests.len(),
            22,
            "expected the original request at the peer"
        );
    }
    if !requests.is_empty() {
        assert_eq!(&requests[..16], operation.0.as_bytes());
        assert_eq!(&requests[16..20], &offset.to_be_bytes());
        assert_eq!(&requests[20..], &count.to_be_bytes());
    }
    assert_eq!(
        fs::read_to_string(fixture.0.join("run/handshakes")).unwrap(),
        "1"
    );
    assert!(resident_client::attempts(&runtime.store)
        .unwrap()
        .is_empty());
    let shutdown = runtime.shutdown(Instant::now() + Duration::from_secs(2));
    assert!(shutdown.reaped, "{shutdown:?}");
    assert_eq!(shutdown.lost, LostRequest::NoPendingMutation);
}

#[test]
fn pre_enter_raw_units_empty_refusal_and_typed_rejections() {
    let pattern = [0x0041, 0, 0x2029, 0xd800, 0xdc00, 0xdfff];
    for (offset, count) in [(0, 0), (0, 12), (0, 4096), (i32::MAX as u32 - 1, 0)] {
        run(
            "reconnect-pre-enter",
            offset,
            count,
            "none",
            Some(Reply::PreEnterText(Some(
                (0..usize::from(count))
                    .map(|i| pattern[i % pattern.len()])
                    .collect(),
            ))),
        );
    }
    run(
        "reconnect-pre-enter-refused",
        0,
        0,
        "none",
        Some(Reply::PreEnterText(None)),
    );
    for (mode, rejection) in [
        ("unsupported", Rejection::Unsupported),
        ("capacity", Rejection::Capacity),
    ] {
        run(
            &format!("reconnect-pre-enter-{mode}"),
            0,
            12,
            "none",
            Some(Reply::Rejected(rejection)),
        );
    }
}

#[test]
fn pre_enter_invalid_ranges_never_reach_peer() {
    for (offset, count) in [
        (0, 4097),
        (i32::MAX as u32, 0),
        (i32::MAX as u32 - 12, 12),
        (u32::MAX, 4096),
    ] {
        run("reconnect-pre-enter", offset, count, "invalid", None);
    }
}

#[test]
fn pre_enter_wrong_reply_correlation_never_succeeds_or_replays() {
    for fault in [
        "wrong-type",
        "wrong-count",
        "wrong-digest",
        "wrong-operation",
        "wrong-sequence",
        "wrong-scope",
    ] {
        run(&format!("reconnect-pre-enter-{fault}"), 0, 12, "none", None);
    }
}

#[test]
fn pre_enter_original_deadline_and_admission_retention_fail_closed() {
    for fault in ["expired", "boundary", "entry", "closing", "retention"] {
        run("reconnect-pre-enter", 0, 12, fault, None);
    }
    run("reconnect-pre-enter-progress", 0, 12, "progress", None);
    run("reconnect-pre-enter-wait", 0, 12, "none", None);
    run("reconnect-pre-enter-wrong-digest", 0, 12, "both", None);
}
