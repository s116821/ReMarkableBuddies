use super::*;
use crate::note_export::capacities::CapacitiesDestination;
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

// Exactly the two read-only preflight requests. The flow must never send POST.
fn preflight_server() -> (CapacitiesDestination, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let space = Uuid::new_v4();
    let destination = CapacitiesDestination::loopback_fixture(
        &format!("http://{}/", listener.local_addr().unwrap()),
        "fixture",
        space,
        BTreeSet::new(),
    )
    .unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        for (path, body) in [
            ("/space", serde_json::json!({"id": space})),
            (
                "/space/structures",
                serde_json::json!({"structures":[{"id":"RootPage",
                "propertyDefinitions":[{"id":"title","type":"title","writable":true},
                {"id":"content","type":"richText","writable":true}]}]}),
            ),
        ] {
            let end = std::time::Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < end, "preflight not polled");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                assert!(header.len() < 8192);
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            assert!(String::from_utf8(header)
                .unwrap()
                .starts_with(&format!("GET {path} HTTP/1.1\r\n")));
            let body = body.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    (destination, worker)
}

#[test]
fn selected_capacities_draft_and_cancel_are_checked_at_actual_write_poll() {
    for target in ["root", "source", "draft", "cancel"] {
        let f = Fixture::new();
        let (destination, worker) = preflight_server();
        let mut m = model();
        let request = f.request();
        let (tx, rx) = watch::channel(false);
        let report = run_destination_with_events(
            &f.ledger,
            &mut m,
            &destination,
            &request,
            &pref(),
            rx,
            &mut |phase, ledger| {
                if phase != Phase::BeforeWritePoll {
                    return;
                }
                if target == "cancel" {
                    tx.send(true).unwrap();
                    return;
                }
                let records = ledger.inspect(f.conversation, false).unwrap();
                let (namespace, id) = match target {
                    "root" => (Namespace::Conversation, f.conversation),
                    "source" => (
                        Namespace::Source,
                        records
                            .iter()
                            .find_map(|r| match r {
                                crate::conversation::Record::NormalizedText(s) => Some(s.id),
                                _ => None,
                            })
                            .unwrap(),
                    ),
                    _ => (
                        Namespace::Conversation,
                        records
                            .iter()
                            .find_map(|r| match r {
                                crate::conversation::Record::Turn(t)
                                    if t.role == crate::conversation::Role::Assistant
                                        && t.outcome == Outcome::Generated =>
                                {
                                    Some(t.id)
                                }
                                _ => None,
                            })
                            .unwrap(),
                    ),
                };
                revise(ledger, namespace, id);
            },
        );
        worker.join().unwrap();
        assert_eq!(m.calls, 1);
        assert!(report.observation.destination_polled);
        assert!(!report.observation.write_polled);
        assert_eq!(report.observation.no_send_recorded, Some(true));
        assert_eq!(report.observation.selection_changed, target != "cancel");
        assert_eq!(
            report.status,
            if target == "cancel" {
                Status::Canceled
            } else {
                Status::Refused
            }
        );
    }
}

#[test]
fn capacities_admission_cancellation_never_polls_destination() {
    for phase in [Phase::BeforeExportAdmission, Phase::AfterExportAdmission] {
        let f = Fixture::new();
        let mut m = model();
        let request = f.request();
        let destination = CapacitiesDestination::loopback_fixture(
            "http://127.0.0.1:1/",
            "fixture",
            Uuid::new_v4(),
            BTreeSet::new(),
        )
        .unwrap();
        let (tx, rx) = watch::channel(false);
        let report = run_destination_with_events(
            &f.ledger,
            &mut m,
            &destination,
            &request,
            &pref(),
            rx,
            &mut |p, _| {
                if p == phase {
                    tx.send(true).unwrap();
                }
            },
        );
        assert_eq!(report.status, Status::Canceled);
        assert!(!report.observation.destination_polled);
        assert!(!report.observation.write_polled);
        assert_eq!(
            report.observation.no_send_recorded,
            if phase == Phase::AfterExportAdmission {
                Some(true)
            } else {
                None
            }
        );
    }
}

#[test]
fn interrupted_guarded_preparation_direct_retry_is_unresolved_without_http() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let space = Uuid::new_v4();
    let destination = CapacitiesDestination::loopback_fixture(
        &format!("http://{}/", listener.local_addr().unwrap()),
        "fixture",
        space,
        BTreeSet::new(),
    )
    .unwrap();
    let request = f.request();
    let mut m = model();
    let (_tx, rx) = watch::channel(false);
    let first = run_destination_with_events(
        &f.ledger,
        &mut m,
        &destination,
        &request,
        &pref(),
        rx.clone(),
        &mut |phase, ledger| {
            if phase == Phase::BeforeExportAdmission {
                ledger.store().set_fault(Fault::AfterCommit).unwrap();
            }
        },
    );
    assert_eq!(first.status, Status::ReconcileRequired);
    assert!(!first.observation.destination_polled && !first.observation.write_polled);
    let f = f.reopen();
    let (_, correlation, effect) = request.identities();
    let key =
        crate::conversation::CapacitiesAttempt::association_id(f.conversation, space, correlation);
    let stored = f
        .ledger
        .export_association(f.conversation, key)
        .unwrap()
        .unwrap();
    assert_eq!(stored.outcome, Outcome::Prepared);
    // This is the actual incompatible receipt, not a manufactured association.
    assert!(f
        .ledger
        .historical_export_attempt(f.conversation, key, effect)
        .is_err());
    let draft = f
        .ledger
        .writer_turn(first.draft.unwrap(), f.conversation)
        .unwrap();
    let source = f
        .ledger
        .inspect(f.conversation, false)
        .unwrap()
        .into_iter()
        .find_map(|record| match record {
            crate::conversation::Record::NormalizedText(source) => Some(source),
            _ => None,
        })
        .unwrap();
    let note = writer::note_from_draft(&draft, &source, correlation, effect).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let retry = runtime.block_on(destination.export(&f.ledger, &note, rx.clone()));
    assert_eq!(retry.status, note_export::ExportStatus::ReconcileRequired);
    assert_eq!(retry.failure, None);
    assert_eq!(retry.effect_operation, effect);
    let mut changed = note.clone();
    changed.markdown.push_str(" different content");
    assert_eq!(
        runtime
            .block_on(destination.export(&f.ledger, &changed, rx.clone()))
            .status,
        note_export::ExportStatus::Conflict
    );
    let mut foreign_source = note;
    foreign_source.source_revision.push_str("-changed");
    assert_eq!(
        runtime
            .block_on(destination.export(&f.ledger, &foreign_source, rx.clone()))
            .status,
        note_export::ExportStatus::Conflict
    );
    assert_eq!(
        run_capacities(&f.ledger, &mut m, &destination, &request, &pref(), rx).status,
        Status::ReconcileRequired
    );
    assert_eq!(m.calls, 1);
    assert_eq!(
        f.ledger
            .export_association(f.conversation, key)
            .unwrap()
            .unwrap(),
        stored
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
