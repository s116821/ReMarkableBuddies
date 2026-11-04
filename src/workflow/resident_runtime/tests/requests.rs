use super::*;
use crate::workflow::resident_request::{self as request, FailureKind, InputProgress, Request};
use remarkable_open_sdk::{
    native_adapter::{InsertAfter, ReaderFacts, SelectAdjacent},
    PageKey,
};

fn end() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn quiet() -> Result<InputProgress> {
    Ok(InputProgress::Quiet)
}

#[test]
fn owned_input_guard_reaches_request_and_reconnect_failure_classification() {
    use crate::device::{interaction::Interaction, request_input::RequestInput};
    let fixture = Fixture::new();
    let mut runtime = fixture.start("reconnect", Duration::from_secs(5)).unwrap();
    let identity = runtime.identity().clone();
    let mut input = RequestInput::scripted(vec![
        Ok(vec![Interaction::Invalidated]),
        Ok(vec![]),
        Err(anyhow::anyhow!("lost real input observer")),
    ]);
    let canceled = request::execute(
        &mut runtime,
        OperationId(nonce()),
        Request::Observe,
        end(),
        &mut || input.poll(),
        &mut || Ok(()),
    )
    .unwrap_err();
    assert_eq!(canceled.kind, FailureKind::Canceled);
    assert_eq!(canceled.lost, LostRequest::NoPendingMutation);
    let still_canceled =
        request::reconnect(&mut runtime, end(), &mut || input.poll(), &mut || Ok(())).unwrap_err();
    assert_eq!(still_canceled.kind, FailureKind::Canceled);
    let lost =
        request::reconnect(&mut runtime, end(), &mut || input.poll(), &mut || Ok(())).unwrap_err();
    assert_eq!(lost.kind, FailureKind::Integrity);
    assert_eq!(runtime.identity(), &identity);
    runtime.check_child().unwrap();
    assert!(runtime.shutdown(end()).reaped);
}

#[test]
fn input_changed_during_authority_check_refuses_before_effect_reservation() {
    let fixture = Fixture::new();
    let mut runtime = fixture.start("reconnect", Duration::from_secs(5)).unwrap();
    let changed = std::cell::Cell::new(false);
    let page = PageKey {
        document: nonce(),
        page: nonce(),
    };
    let error = request::execute(
        &mut runtime,
        OperationId(nonce()),
        Request::Insert {
            conversation: StoreUuid::new_v4(),
            insertion: InsertAfter {
                source: ReaderFacts::new(page, vec![page.page]).unwrap(),
                target: nonce(),
            },
        },
        end(),
        &mut || {
            Ok(if changed.get() {
                InputProgress::Canceled
            } else {
                InputProgress::Quiet
            })
        },
        &mut || {
            changed.set(true);
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, FailureKind::Canceled);
    assert_eq!(error.lost, LostRequest::NoPendingMutation);
    assert!(resident_client::attempts(&runtime.store)
        .unwrap()
        .is_empty());
    runtime.check_child().unwrap();
    assert!(runtime.shutdown(end()).reaped);
}

#[test]
fn final_admission_after_real_outcome_publication_preserves_history_without_delivering_stale_reply()
{
    for scenario in ["canceled", "deadline", "input-error"] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start("reconnect-selection", Duration::from_secs(5))
            .unwrap();
        let published = std::rc::Rc::new(std::cell::Cell::new(false));
        let hook = published.clone();
        let deadline = Instant::now() + Duration::from_millis(700);
        resident_client::after_next_outcome(move || {
            hook.set(true);
            if scenario == "deadline" {
                thread::sleep(
                    deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
                );
            }
        });
        let page = PageKey {
            document: nonce(),
            page: nonce(),
        };
        let original = OperationId(nonce());
        let error = request::execute(
            &mut runtime,
            original,
            Request::Select {
                conversation: StoreUuid::new_v4(),
                selection: SelectAdjacent {
                    expected: ReaderFacts::new(page, vec![page.page]).unwrap(),
                    target: page.page,
                },
            },
            deadline,
            &mut || {
                ensure!(
                    !(published.get() && scenario == "input-error"),
                    "observer lost during retention"
                );
                Ok(if published.get() && scenario == "canceled" {
                    InputProgress::Canceled
                } else {
                    InputProgress::Quiet
                })
            },
            &mut || Ok(()),
        )
        .unwrap_err();
        assert!(published.get());
        assert_eq!(
            error.kind,
            match scenario {
                "canceled" => FailureKind::Canceled,
                "deadline" => FailureKind::Deadline,
                _ => FailureKind::Integrity,
            }
        );
        assert_eq!(error.lost, LostRequest::NoPendingMutation);
        let history = resident_client::attempts(&runtime.store).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].request.frame().operation, original);
        assert!(matches!(history[0].reply, Some(Reply::TargetObserved(_))));
        assert!(!history[0].known_not_dispatched);
        assert!(runtime.shutdown(end()).reaped);
    }
}

#[test]
fn ordinary_request_retirement_keeps_process_and_allows_explicit_reconnect() {
    for scenario in ["canceled", "deadline", "pending-deadline", "eof"] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start(
                if scenario == "eof" {
                    "reconnect-idle"
                } else if scenario == "pending-deadline" {
                    "reconnect-observe-timeout"
                } else {
                    "reconnect"
                },
                Duration::from_secs(5),
            )
            .unwrap();
        let identity = runtime.identity().clone();
        if scenario == "eof" {
            let deadline = end();
            while !fixture.0.join("run/idle-retired").exists() {
                before(deadline).unwrap();
                thread::sleep(POLL);
            }
        }
        let error = request::execute(
            &mut runtime,
            OperationId(nonce()),
            Request::Observe,
            if scenario == "deadline" {
                Instant::now()
            } else if scenario == "pending-deadline" {
                Instant::now() + Duration::from_millis(300)
            } else {
                end()
            },
            &mut || {
                Ok(if scenario == "canceled" {
                    InputProgress::Canceled
                } else {
                    InputProgress::Quiet
                })
            },
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            match scenario {
                "canceled" => FailureKind::Canceled,
                "deadline" | "pending-deadline" => FailureKind::Deadline,
                _ => FailureKind::Connection,
            }
        );
        assert_eq!(error.lost, LostRequest::NoPendingMutation);
        if scenario == "pending-deadline" {
            assert!(fixture.0.join("run/effect-received").exists());
        }
        assert!(runtime.connection.is_none());
        runtime.check_child().unwrap();
        request::reconnect(&mut runtime, end(), &mut quiet, &mut || Ok(())).unwrap();
        assert_eq!(runtime.identity(), &identity);
        assert_eq!(
            request::execute(
                &mut runtime,
                OperationId(nonce()),
                Request::Observe,
                end(),
                &mut quiet,
                &mut || Ok(())
            )
            .unwrap(),
            Reply::Rejected(Rejection::Unsupported)
        );
        assert!(runtime.shutdown(end()).reaped);
    }
}

#[test]
fn dispatcher_reports_input_authority_runtime_and_peer_integrity_failure() {
    for scenario in ["input", "authority", "artifact", "peer"] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start(
                if scenario == "peer" {
                    "reconnect-malformed"
                } else {
                    "reconnect"
                },
                Duration::from_secs(5),
            )
            .unwrap();
        let error = request::execute(
            &mut runtime,
            OperationId(nonce()),
            Request::Observe,
            end(),
            &mut || {
                ensure!(scenario != "input", "input observer lost");
                quiet()
            },
            &mut || {
                ensure!(scenario != "authority", "installed lease changed");
                if scenario == "artifact" {
                    let file = fixture.0.join("run/adapter.so");
                    fs::rename(&file, file.with_extension("original")).unwrap();
                    fs::write(file, b"changed").unwrap();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::Integrity, "{scenario}: {error:?}");
        assert_eq!(error.lost, LostRequest::NoPendingMutation);
        assert!(runtime.connection.is_none());
        assert!(runtime.shutdown(end()).reaped);
    }
}

#[test]
fn dispatched_cancellation_remains_original_through_dispatcher_reconnect_and_reconciliation() {
    for select in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start("reconnect-pending", Duration::from_secs(5))
            .unwrap();
        let identity = runtime.identity().clone();
        let page = PageKey {
            document: nonce(),
            page: nonce(),
        };
        let facts = ReaderFacts::new(page, vec![page.page]).unwrap();
        let conversation = StoreUuid::new_v4();
        let original = OperationId(nonce());
        let make = |conversation| {
            if select {
                Request::Select {
                    conversation,
                    selection: SelectAdjacent {
                        expected: facts.clone(),
                        target: page.page,
                    },
                }
            } else {
                Request::Insert {
                    conversation,
                    insertion: InsertAfter {
                        source: facts.clone(),
                        target: nonce(),
                    },
                }
            }
        };
        let error = request::execute(
            &mut runtime,
            original,
            make(conversation),
            end(),
            &mut || {
                Ok(if fixture.0.join("run/effect-received").exists() {
                    InputProgress::Canceled
                } else {
                    InputProgress::Quiet
                })
            },
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::Canceled, "{error:?}");
        assert!(
            matches!(error.lost, LostRequest::Indeterminate { operation, .. } if operation == original)
        );
        assert!(runtime.connection.is_none());
        request::reconnect(&mut runtime, end(), &mut quiet, &mut || Ok(())).unwrap();
        assert_eq!(runtime.identity(), &identity);
        // Read-only pixels do not reconcile the uncertain insertion/selection.
        let journal = runtime
            .store
            .paths
            .data
            .join("native-insertion-attempts-v1.json");
        let before_capture = fs::read(&journal).unwrap();
        assert!(matches!(
            request::execute(
                &mut runtime,
                OperationId(nonce()),
                Request::CaptureSource(facts.clone()),
                end(),
                &mut quiet,
                &mut || Ok(())
            )
            .unwrap(),
            Reply::Captured(_)
        ));
        assert_eq!(fs::read(&journal).unwrap(), before_capture);
        #[cfg(feature = "development-resident-bootstrap")]
        {
            use crate::workflow::resident_supervisor::service::bootstrap::setup::visual::capture_on_runtime;
            let loss = runtime.retired_loss.clone();
            capture_on_runtime(&mut runtime, end(), &mut || Ok(())).unwrap();
            assert_eq!(runtime.identity(), &identity);
            assert_eq!(runtime.retired_loss, loss);
            assert_eq!(fs::read(&journal).unwrap(), before_capture);
        }
        let refused = request::execute(
            &mut runtime,
            OperationId(nonce()),
            make(StoreUuid::new_v4()),
            end(),
            &mut quiet,
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(refused.kind, FailureKind::Unresolved);
        assert_eq!(refused.lost, error.lost);
        request::reconnect(&mut runtime, end(), &mut quiet, &mut || Ok(())).unwrap();
        assert_eq!(
            request::execute(
                &mut runtime,
                OperationId(nonce()),
                Request::Reconcile {
                    conversation,
                    original
                },
                end(),
                &mut quiet,
                &mut || Ok(())
            )
            .unwrap(),
            Reply::Rejected(Rejection::Unsupported)
        );
        let history = resident_client::attempts(&runtime.store).unwrap();
        assert_eq!(history.len(), 1);
        assert!(!history[0].known_not_dispatched && history[0].reply.is_none());
        let report = runtime.shutdown(end());
        assert!(report.reaped && report.error.is_none(), "{report:?}");
        assert_eq!(report.lost, error.lost);
    }
}

struct ReadOnlyProbe {
    boundaries: usize,
    pulses: usize,
    inflate: usize,
    fail: &'static str,
    prepared: bool,
    received: bool,
    closing: Vec<(bool, bool)>,
}
impl ReadOnlyAdmission for ReadOnlyProbe {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        for _ in 0..self.inflate {
            self.pulses += 1;
            before(deadline)?;
        }
        if self.received && self.fail == "closing" {
            anyhow::bail!("closing ownership loss");
        }
        Ok(())
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.boundaries += 1;
        self.pulse(deadline)?;
        if self.fail == "policy" {
            return Err(resident_request::authority_error(anyhow::anyhow!(
                "changed loaded policy"
            )));
        }
        if self.fail == "cancel" {
            return resident_request::admission(
                deadline,
                &mut || Ok(crate::workflow::resident_request::InputProgress::Canceled),
                &mut || Ok(()),
            );
        }
        Ok(())
    }
    fn before_api(&mut self, _: Instant) -> Result<()> {
        self.prepared = true;
        if self.fail == "entry-expiry" {
            return Err(crate::workflow::resident_request::DeadlineExpired(
                "entry retention expired",
            )
            .into());
        }
        Ok(())
    }
    fn reply_observed(&mut self) {
        self.received = true;
    }
    fn after_api(
        &mut self,
        _: Instant,
        invoked: bool,
        result: &Result<remarkable_open_sdk::native_adapter::Reply>,
    ) -> Result<()> {
        self.closing.push((invoked, result.is_ok()));
        Ok(())
    }
}
#[test]
fn closed_read_only_actual_peer_has_fixed_boundaries_and_preserves_api_retention() {
    for fault in [
        "none",
        "policy",
        "cancel",
        "entry-expiry",
        "closing",
        "missing",
    ] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start("reconnect-source-handoff", Duration::from_secs(5))
            .unwrap();
        let mut guard = ReadOnlyProbe {
            boundaries: 0,
            pulses: 0,
            inflate: 1,
            fail: fault,
            prepared: false,
            received: false,
            closing: vec![],
        };
        if fault == "missing" {
            runtime.retire_connection();
        }
        let result = runtime.request_read_only(
            OperationId(nonce()),
            ReadOnlyRequest::Observe,
            Instant::now() + Duration::from_secs(3),
            &mut guard,
        );
        match fault {
            "none" => {
                assert!(matches!(result.unwrap(), Reply::Observed(_)));
                assert_eq!(guard.boundaries, 3);
                assert_eq!(guard.closing, [(true, true)]);
            }
            "entry-expiry" => {
                assert!(result.is_err());
                assert_eq!(guard.closing, [(false, false)]);
            }
            "closing" => {
                assert!(result.is_err());
                assert_eq!(guard.closing, [(true, false)]);
                assert_eq!(guard.boundaries, 2);
            }
            "missing" => {
                assert!(result.is_err());
                assert!(!guard.prepared);
                assert!(guard.closing.is_empty());
            }
            "policy" | "cancel" => {
                let failure = resident_request::finish(&mut runtime, result).unwrap_err();
                assert_eq!(
                    failure.kind,
                    if fault == "policy" {
                        resident_request::FailureKind::Integrity
                    } else {
                        resident_request::FailureKind::Canceled
                    }
                );
                assert!(!guard.prepared);
                assert!(guard.closing.is_empty());
            }
            _ => unreachable!(),
        }
        assert!(
            runtime
                .shutdown(Instant::now() + Duration::from_secs(2))
                .reaped
        );
    }
}
#[test]
fn read_only_actual_peer_poll_growth_never_multiplies_full_admission() {
    for inflate in [1, 1000] {
        let fixture = Fixture::new();
        let mut runtime = fixture
            .start("reconnect-source-handoff", Duration::from_secs(5))
            .unwrap();
        let mut guard = ReadOnlyProbe {
            boundaries: 0,
            pulses: 0,
            inflate,
            fail: "none",
            prepared: false,
            received: false,
            closing: vec![],
        };
        let first = runtime
            .request_read_only(
                OperationId(nonce()),
                ReadOnlyRequest::Observe,
                Instant::now() + Duration::from_secs(3),
                &mut guard,
            )
            .unwrap();
        let Reply::Observed(facts) = first else {
            panic!("wrong response")
        };
        runtime
            .request_read_only(
                OperationId(nonce()),
                ReadOnlyRequest::CaptureSource(facts),
                Instant::now() + Duration::from_secs(3),
                &mut guard,
            )
            .unwrap();
        assert_eq!(guard.boundaries, 6);
        assert!(guard.pulses >= inflate * 6);
        assert_eq!(guard.closing, [(true, true), (true, true)]);
        assert!(resident_client::attempts(&runtime.store)
            .unwrap()
            .is_empty());
        assert!(
            runtime
                .shutdown(Instant::now() + Duration::from_secs(2))
                .reaped
        );
    }
}
