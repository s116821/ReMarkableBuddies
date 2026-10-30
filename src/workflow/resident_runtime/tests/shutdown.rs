use super::*;

fn signals(report: &ChildShutdown) -> Vec<i32> {
    report
        .signal_attempts
        .iter()
        .map(|attempt| attempt.signal)
        .collect()
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn cooperative_companion_finishes_before_reader_term_and_reports_real_exits() {
    let fixture = Fixture::new();
    let runtime = fixture
        .launch_companion(
            fixture.companion_profile("term-report"),
            "reconnect-companion-term",
            Duration::from_secs(5),
        )
        .unwrap();
    let report = runtime.shutdown(Instant::now() + Duration::from_secs(2));
    assert!(report.reaped && report.error.is_none(), "{report:?}");
    for child in [
        report.companion.as_ref().unwrap(),
        report.reader.as_ref().unwrap(),
    ] {
        assert_eq!(signals(child), vec![libc::SIGTERM], "{child:?}");
        assert!(child.signal_attempts[0].error.is_none());
        assert_eq!(child.exit_code, Some(0));
        assert_eq!(child.exit_signal, None);
    }
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.0.join("run/reader-term")).unwrap()).unwrap();
    assert_eq!(receipt["companion_absent"], true);
    assert_eq!(receipt["companion_term"], true);
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn stubborn_children_escalate_within_the_original_budget_and_both_reap() {
    let fixture = Fixture::new();
    let runtime = fixture
        .launch_companion(
            fixture.companion_profile("ignore-term"),
            "reconnect-companion-ignore-term",
            Duration::from_secs(5),
        )
        .unwrap();
    let start = Instant::now();
    let report = runtime.shutdown(start + Duration::from_millis(800));
    assert!(report.reaped && report.error.is_none(), "{report:?}");
    for child in [
        report.companion.as_ref().unwrap(),
        report.reader.as_ref().unwrap(),
    ] {
        assert_eq!(
            signals(child),
            vec![libc::SIGTERM, libc::SIGKILL],
            "{child:?}"
        );
        assert!(child
            .signal_attempts
            .iter()
            .all(|attempt| attempt.error.is_none()));
        assert_eq!(child.exit_code, None);
        assert_eq!(child.exit_signal, Some(libc::SIGKILL));
    }
    assert!(start.elapsed() < Duration::from_secs(2));
}

// Retain the actual Child handles for fixture cleanup after an intentionally
// exhausted deadline. Cleanup never signals the PID from a receipt.
fn take_owned(runtime: &mut ResidentRuntime) -> (Option<Stopping>, Option<Stopping>) {
    runtime.retire_connection();
    let companion = runtime
        .prepared
        .companion
        .as_mut()
        .unwrap()
        .take_child()
        .map(|(child, start)| Stopping::new(child, start));
    let reader = runtime
        .child
        .take()
        .map(|child| Stopping::new(child, Some(runtime.prepared.identity.start_ticks)));
    (reader, companion)
}
fn cleanup(reader: &mut Option<Stopping>, companion: &mut Option<Stopping>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    for pending in [companion.as_mut(), reader.as_mut()].into_iter().flatten() {
        pending.signal(libc::SIGKILL);
    }
    for pending in [companion.as_mut(), reader.as_mut()].into_iter().flatten() {
        pending.reap_until(deadline);
        assert!(pending.report.reaped, "{:?}", pending.report);
    }
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn expired_budget_still_dispatches_force_to_both_owned_children() {
    let fixture = Fixture::new();
    let mut runtime = fixture
        .launch_companion(
            fixture.companion_profile("ignore-term"),
            "reconnect-companion-ignore-term",
            Duration::from_secs(5),
        )
        .unwrap();
    let (mut reader, mut companion) = take_owned(&mut runtime);
    stop_owned(&mut reader, &mut companion, true, Instant::now());
    let reports = [
        companion.as_ref().unwrap().report.clone(),
        reader.as_ref().unwrap().report.clone(),
    ];
    cleanup(&mut reader, &mut companion);
    for report in reports {
        assert_eq!(signals(&report), vec![libc::SIGKILL], "{report:?}");
        assert!(report.signal_attempts[0].error.is_none());
        if !report.reaped {
            assert!(report.error.is_some());
            assert_eq!(report.exit_signal, None);
            assert_eq!(report.exit_code, None);
        }
    }
}

#[test]
#[ignore = "requires root for the fixed vendor process contract"]
fn modeled_companion_signal_failure_is_retained_and_does_not_skip_reader() {
    let fixture = Fixture::new();
    let mut runtime = fixture
        .launch_companion(
            fixture.companion_profile("ignore-term"),
            "reconnect-companion-ignore-term",
            Duration::from_secs(5),
        )
        .unwrap();
    let (mut reader, mut companion) = take_owned(&mut runtime);
    companion.as_mut().unwrap().fail_signal = Some(libc::SIGTERM);
    stop_owned(
        &mut reader,
        &mut companion,
        true,
        Instant::now() + Duration::from_millis(800),
    );
    let companion_report = companion.as_ref().unwrap().report.clone();
    let reader_report = reader.as_ref().unwrap().report.clone();
    cleanup(&mut reader, &mut companion);
    assert!(companion_report.reaped && reader_report.reaped);
    assert_eq!(
        signals(&companion_report),
        vec![libc::SIGTERM, libc::SIGKILL]
    );
    assert!(companion_report.signal_attempts[0].error.is_some());
    assert!(companion_report.signal_attempts[1].error.is_none());
    assert!(companion_report.error.is_some());
    assert_eq!(companion_report.exit_signal, Some(libc::SIGKILL));
    assert_eq!(signals(&reader_report), vec![libc::SIGTERM, libc::SIGKILL]);
    assert!(reader_report.error.is_none());
}
