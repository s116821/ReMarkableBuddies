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
