use super::*;
use crate::storage::StorePaths;
use remarkable_open_sdk::{native_adapter::*, OperationId, PageKey, Uuid as SdkUuid};
use std::{fs, path::PathBuf, sync::Arc};

#[path = "tests_selection.rs"]
mod selection;

fn id(n: u32) -> SdkUuid {
    SdkUuid::parse(&format!("00000000-0000-0000-0000-{n:012x}")).unwrap()
}
fn facts() -> ReaderFacts {
    ReaderFacts::new(
        PageKey {
            document: id(10),
            page: id(11),
        },
        vec![id(11), id(12)],
    )
    .unwrap()
}
fn scope() -> ConnectionIdentity {
    ConnectionIdentity {
        runtime: RuntimeIdentity {
            profile_sha256: [1; 32],
            executable_sha256: [2; 32],
            adapter_sha256: [3; 32],
            boot: id(2),
            pid: 42,
            start_ticks: 43,
            activation: id(3),
        },
        connection: id(4),
        server_nonce: Some(id(5)),
    }
}
fn request(operation: u32, target: u32) -> Frame {
    Frame {
        scope: scope(),
        sequence: 1,
        operation: OperationId(id(operation)),
        message: Message::Request(Command::InsertAfter(InsertAfter {
            source: facts(),
            target: id(target),
        })),
    }
}
fn response(request: &Frame, reply: Reply) -> Frame {
    Frame {
        message: Message::Response {
            request_sha256: request.fingerprint().unwrap(),
            reply,
        },
        ..request.clone()
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-resident-journal-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn claim(store: &Store, conversation: Uuid, frame: &Frame) -> Result<()> {
    reserve(store, conversation, frame, frame.fingerprint().unwrap())
}

#[test]
fn exact_original_survives_restart_and_rejects_replacement_identity() {
    let f = Fixture::new();
    let store = f.open();
    let conversation = Uuid::new_v4();
    let original = request(50, 13);
    claim(&store, conversation, &original).unwrap();
    let bytes = fs::read(store.paths.data.join(JOURNAL_FILE)).unwrap();
    for frame in [&original, &request(50, 14), &request(51, 14)] {
        assert!(claim(&store, conversation, frame).is_err());
    }
    assert!(claim(&store, Uuid::new_v4(), &original).is_err());
    assert_eq!(
        fs::read(store.paths.data.join(JOURNAL_FILE)).unwrap(),
        bytes
    );
    drop(store);
    let store = f.open();
    let saved = attempts(&store).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(*saved[0].request.frame(), original);
    assert!(!saved[0].known_not_dispatched && saved[0].reply.is_none());
    assert!(claim(&store, conversation, &request(52, 15)).is_err());
}

#[test]
fn store_generation_restore_cannot_erase_local_reservation() {
    let f = Fixture::new();
    let store = f.open();
    let backup = f.0.join("backup");
    store.export(&backup).unwrap();
    let before = fs::read(store.paths.data.join("CURRENT")).unwrap();
    let conversation = Uuid::new_v4();
    let original = request(50, 13);
    claim(&store, conversation, &original).unwrap();
    store.restore(&backup).unwrap();
    assert_ne!(fs::read(store.paths.data.join("CURRENT")).unwrap(), before);
    assert_eq!(*attempts(&store).unwrap()[0].request.frame(), original);
    assert!(claim(&store, conversation, &request(51, 14)).is_err());
}

#[test]
fn only_retained_known_no_effect_allows_new_caller_operation() {
    for outcome in [
        Outcome::NotDispatched {},
        Outcome::HistoricalReply {
            frame: encode(&response(
                &request(50, 13),
                Reply::Rejected(Rejection::BeforeDispatch),
            ))
            .unwrap(),
        },
    ] {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let original = request(50, 13);
        claim(&store, conversation, &original).unwrap();
        finish(&store, &original, outcome).unwrap();
        assert!(claim(&store, conversation, &original).is_err()); // Operation is forever consumed.
        claim(&store, conversation, &request(51, 14)).unwrap();
        assert!(claim(&store, conversation, &request(52, 15)).is_err());
        assert_eq!(attempts(&store).unwrap().len(), 2);
    }
}

#[test]
fn observed_and_unknown_replies_never_release_conversation() {
    for reply in [
        Reply::Indeterminate,
        Reply::InsertionObserved {
            target: PageKey {
                document: id(10),
                page: id(13),
            },
            after_order: vec![id(11), id(13), id(12)],
        },
    ] {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let original = request(50, 13);
        claim(&store, conversation, &original).unwrap();
        let outcome = Outcome::HistoricalReply {
            frame: encode(&response(&original, reply.clone())).unwrap(),
        };
        finish(&store, &original, outcome.clone()).unwrap();
        finish(&store, &original, outcome).unwrap();
        assert_eq!(attempts(&store).unwrap()[0].reply, Some(reply));
        assert!(finish(&store, &original, Outcome::NotDispatched {}).is_err());
        assert!(claim(&store, conversation, &request(51, 14)).is_err());
    }
}

#[test]
fn wrong_reply_and_wrong_journal_digest_preserve_reservation() {
    let f = Fixture::new();
    let store = f.open();
    let original = request(50, 13);
    let conversation = Uuid::new_v4();
    assert!(reserve(&store, conversation, &original, [0; 32]).is_err());
    assert!(!store.paths.data.join(JOURNAL_FILE).exists());
    claim(&store, conversation, &original).unwrap();
    let bytes = fs::read(store.paths.data.join(JOURNAL_FILE)).unwrap();
    for reply in [
        response(&request(51, 13), Reply::Indeterminate),
        response(&original, Reply::Observed(facts())),
        response(
            &original,
            Reply::InsertionObserved {
                target: PageKey {
                    document: id(10),
                    page: id(14),
                },
                after_order: vec![id(11), id(14), id(12)],
            },
        ),
    ] {
        assert!(finish(
            &store,
            &original,
            Outcome::HistoricalReply {
                frame: encode(&reply).unwrap()
            }
        )
        .is_err());
        assert_eq!(
            fs::read(store.paths.data.join(JOURNAL_FILE)).unwrap(),
            bytes
        );
    }
}

#[test]
fn malformed_partial_unknown_and_oversized_journals_are_not_reset() {
    let f = Fixture::new();
    let store = f.open();
    let original = request(50, 13);
    let conversation = Uuid::new_v4();
    claim(&store, conversation, &original).unwrap();
    let path = store.paths.data.join(JOURNAL_FILE);
    let valid: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut foreign = valid.clone();
    foreign["actor"] = serde_json::json!(Uuid::new_v4());
    let mut unknown = valid.clone();
    unknown["attempts"][0]["outcome"]["injected"] = serde_json::json!(true);
    let mut bad_digest = valid.clone();
    bad_digest["attempts"][0]["sha256"] = serde_json::json!("0".repeat(64));
    for bytes in [
        b"{".to_vec(),
        vec![],
        serde_json::to_vec(&foreign).unwrap(),
        serde_json::to_vec(&unknown).unwrap(),
        serde_json::to_vec(&bad_digest).unwrap(),
        vec![b' '; MAX_JOURNAL + 1],
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(attempts(&store).is_err());
        assert!(claim(&store, Uuid::new_v4(), &request(51, 14)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn concurrent_calls_share_existing_store_lock() {
    let f = Fixture::new();
    let store = f.open();
    let conversation = Uuid::new_v4();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|n| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                claim(&store, conversation, &request(50 + n, 100 + n)).is_ok()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .filter_map(|worker| worker.join().unwrap().then_some(()))
            .count(),
        1
    );
    assert_eq!(attempts(&store).unwrap().len(), 1);
}

#[test]
fn retained_attempt_capacity_refuses_without_evicting_history() {
    let f = Fixture::new();
    let store = f.open();
    let path = store.paths.data.join(JOURNAL_FILE);
    let mut state = State {
        format: 1,
        actor: store.actor_id,
        attempts: Vec::new(),
    };
    for n in 0..MAX_ATTEMPTS {
        let request = request(1000 + n as u32, 13);
        state.attempts.push(Attempt {
            conversation: Uuid::new_v4(),
            frame: encode(&request).unwrap(),
            sha256: digest(&request).unwrap(),
            outcome: Outcome::NotDispatched {},
        });
    }
    persist(&path, &state).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(claim(&store, Uuid::new_v4(), &request(9000, 13)).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

const V1_REQUEST: &[u8] = include_bytes!("fixtures/v1-insertion.bin");
const V1_RESPONSE: &[u8] = include_bytes!("fixtures/v1-insertion-observed.bin");
fn legacy_state(store: &Store, conversation: Uuid, outcome: Outcome) -> State {
    State {
        format: 1,
        actor: store.actor_id,
        attempts: vec![Attempt {
            conversation,
            frame: STANDARD.encode(V1_REQUEST),
            sha256: crate::storage::digest(V1_REQUEST),
            outcome,
        }],
    }
}

#[test]
fn v1_bytes_and_reservations_survive_upgrade_restart_and_append() {
    for outcome in [
        Outcome::Reserved {},
        Outcome::HistoricalReply {
            frame: STANDARD.encode(V1_RESPONSE),
        },
        Outcome::NotDispatched {},
    ] {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let backup = f.0.join("before-journal");
        store.export(&backup).unwrap();
        let state = legacy_state(&store, conversation, outcome.clone());
        let path = store.paths.data.join(JOURNAL_FILE);
        persist(&path, &state).unwrap();
        let first: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        store.restore(&backup).unwrap();
        drop(store);
        let store = f.open();
        let saved = attempts(&store).unwrap();
        assert_eq!(saved[0].request.wire_revision(), 1);
        assert_eq!(saved[0].request.encoded(), V1_REQUEST);
        assert_ne!(
            saved[0].request.fingerprint(),
            saved[0].request.frame().fingerprint().unwrap()
        );
        // Re-encoding the old operation cannot complete or reserve it again.
        let original = saved[0].request.frame().clone();
        assert!(finish(&store, &original, Outcome::NotDispatched {}).is_err());
        assert!(claim(&store, Uuid::new_v4(), &original).is_err());
        let before = fs::read(&path).unwrap();
        if matches!(outcome, Outcome::NotDispatched {}) {
            claim(&store, conversation, &request(70, 14)).unwrap();
        } else {
            assert!(claim(&store, conversation, &request(70, 14)).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        claim(&store, Uuid::new_v4(), &request(71, 15)).unwrap();
        let appended: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(appended["format"], 2);
        assert_eq!(appended["attempts"][0], first["attempts"][0]);
        assert_eq!(
            attempts(&store)
                .unwrap()
                .last()
                .unwrap()
                .request
                .wire_revision(),
            2
        );
    }
}

#[test]
fn mixed_revision_and_reencoded_digest_refuse_without_rewriting_history() {
    let f = Fixture::new();
    let store = f.open();
    let conversation = Uuid::new_v4();
    let original = HistoricalFrame::decode(V1_REQUEST).unwrap();
    let response = HistoricalFrame::decode(V1_RESPONSE).unwrap();
    let mixed = legacy_state(
        &store,
        conversation,
        Outcome::HistoricalReply {
            frame: encode(response.frame()).unwrap(),
        },
    );
    let mut wrong_digest = legacy_state(&store, conversation, Outcome::Reserved {});
    wrong_digest.attempts[0].sha256 = digest(original.frame()).unwrap();
    let mut inverse = legacy_state(
        &store,
        conversation,
        Outcome::HistoricalReply {
            frame: STANDARD.encode(V1_RESPONSE),
        },
    );
    inverse.attempts[0].frame = encode(original.frame()).unwrap();
    inverse.attempts[0].sha256 = digest(original.frame()).unwrap();
    let mut wrong_response = response.frame().clone();
    let Message::Response { request_sha256, .. } = &mut wrong_response.message else {
        panic!()
    };
    *request_sha256 = original.frame().fingerprint().unwrap();
    let mut wrong_bytes = wrong_response.encode().unwrap();
    wrong_bytes[8..10].copy_from_slice(&1u16.to_be_bytes());
    let wrong_correlation = legacy_state(
        &store,
        conversation,
        Outcome::HistoricalReply {
            frame: STANDARD.encode(wrong_bytes),
        },
    );
    let path = store.paths.data.join(JOURNAL_FILE);
    for state in [&mixed, &wrong_digest, &inverse, &wrong_correlation] {
        let bytes = serde_json::to_vec(state).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(attempts(&store).is_err());
        assert!(claim(&store, Uuid::new_v4(), &request(72, 16)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
#[cfg(target_os = "linux")]
mod sockets {
    use super::*;
    use std::{
        fs::{File, Permissions},
        io::{Read, Write},
        os::unix::{
            fs::{MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        thread,
        time::{Duration, Instant},
    };
    fn end() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }
    fn runtime_scope() -> ConnectionIdentity {
        let mut scope = scope();
        scope.server_nonce = None;
        scope.runtime.pid = std::process::id();
        scope.runtime.boot = SdkUuid::parse(
            fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .unwrap()
                .trim(),
        )
        .unwrap();
        scope.runtime.start_ticks = fs::read_to_string("/proc/self/stat")
            .unwrap()
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse()
            .unwrap();
        scope.connection = SdkUuid::parse(&Uuid::new_v4().to_string()).unwrap();
        scope
    }
    fn read(stream: &mut UnixStream) -> Frame {
        let mut size = [0; 4];
        stream.read_exact(&mut size).unwrap();
        let size = u32::from_be_bytes(size) as usize;
        assert!(size <= MAX_FRAME);
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).unwrap();
        Frame::decode(&bytes).unwrap()
    }
    fn send(stream: &mut UnixStream, frame: &Frame) {
        stream.write_all(&frame.encode().unwrap()).unwrap();
    }
    fn endpoint(f: &Fixture) -> (UnixListener, File, u32) {
        let path = f.0.join("run");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
        let listener = UnixListener::bind(path.join("sdk.sock")).unwrap();
        fs::set_permissions(path.join("sdk.sock"), Permissions::from_mode(0o600)).unwrap();
        let directory = File::open(path).unwrap();
        let uid = directory.metadata().unwrap().uid();
        (listener, directory, uid)
    }
    fn accept(listener: &UnixListener) -> UnixStream {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut ready = read(&mut stream);
        ready.message = Message::Ready;
        ready.scope.server_nonce = Some(id(6));
        send(&mut stream, &ready);
        stream
    }

    #[test]
    fn caller_progress_preserves_predispatch_proof_or_original_pending_across_restart() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for select in [false, true] {
            for dispatched in [false, true] {
                let f = Fixture::new();
                let store = f.open();
                let conversation = Uuid::new_v4();
                let (listener, directory, uid) = endpoint(&f);
                let seen = Arc::new(AtomicBool::new(false));
                let peer_seen = seen.clone();
                let peer = thread::spawn(move || {
                    let mut stream = accept(&listener);
                    if dispatched {
                        let request = read(&mut stream);
                        assert_eq!(request.operation, OperationId(id(1400)));
                        assert!(matches!(
                            request.message,
                            Message::Request(Command::InsertAfter(_) | Command::SelectAdjacent(_))
                        ));
                        peer_seen.store(true, Ordering::Release);
                    }
                    assert_eq!(stream.read(&mut [0; 1]).unwrap(), 0);
                });
                let mut connection = ResidentConnection::connect(
                    store.clone(),
                    directory,
                    uid,
                    runtime_scope(),
                    OperationId(id(1399)),
                    end(),
                )
                .unwrap();
                let mut canceled = false;
                let mut progress = || {
                    let reserved = !attempts(&store)?.is_empty();
                    if reserved && (!dispatched || seen.load(Ordering::Acquire)) {
                        canceled = true;
                        anyhow::bail!("owned request canceled");
                    }
                    Ok(())
                };
                let failure = if select {
                    connection.select_adjacent_with_progress(
                        conversation,
                        OperationId(id(1400)),
                        SelectAdjacent {
                            expected: facts(),
                            target: id(12),
                        },
                        end(),
                        &mut progress,
                    )
                } else {
                    connection.insert_after_with_progress(
                        conversation,
                        OperationId(id(1400)),
                        InsertAfter {
                            source: facts(),
                            target: id(13),
                        },
                        end(),
                        &mut progress,
                    )
                }
                .unwrap_err();
                assert!(canceled && failure.detail.contains("owned request canceled"));
                assert!(matches!(
                    failure.cause,
                    super::super::EffectCause::Transport(
                        remarkable_open_sdk::native_adapter::transport::Failure::Progress(_)
                    )
                ));
                let history = attempts(&store).unwrap();
                assert_eq!(history.len(), 1);
                let original = history[0].request.clone();
                let expected = if dispatched {
                    LostRequest::Indeterminate {
                        operation: OperationId(id(1400)),
                        request_sha256: original.fingerprint(),
                    }
                } else {
                    LostRequest::NoPendingMutation
                };
                assert_eq!(failure.lost, expected);
                assert_eq!(connection.close(), expected);
                peer.join().unwrap();
                drop(connection);
                drop(store);
                let reopened = f.open();
                let after = attempts(&reopened).unwrap();
                assert_eq!(after[0].request.encoded(), original.encoded());
                assert_eq!(after[0].known_not_dispatched, !dispatched);
                assert!(after[0].reply.is_none());
                // A new caller request is possible only from explicit no-dispatch
                // proof; a lost effect blocks insertion/selection after restart.
                assert_eq!(
                    claim(&reopened, conversation, &request(1401, 14)).is_ok(),
                    !dispatched
                );
            }
        }
    }

    #[test]
    fn selection_is_durable_before_bytes_and_reconciles_specific_original_read_only() {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let (listener, directory, uid) = endpoint(&f);
        let path = store.paths.data.join(JOURNAL_FILE);
        let peer_path = path.clone();
        let peer = thread::spawn(move || {
            let mut stream = accept(&listener);
            for (operation, target) in [(51, 12), (52, 12)] {
                let call = read(&mut stream);
                let saved: State = serde_json::from_slice(&fs::read(&peer_path).unwrap()).unwrap();
                let last = saved.attempts.last().unwrap();
                assert_eq!(*decode(&last.frame).unwrap().frame(), call);
                assert!(matches!(last.outcome, Outcome::Reserved {}));
                assert_eq!(call.operation, OperationId(id(operation)));
                let Message::Request(Command::SelectAdjacent(selection)) = &call.message else {
                    panic!()
                };
                assert_eq!(selection.target, id(target));
                let current = ReaderFacts::new(
                    PageKey {
                        document: id(10),
                        page: id(target),
                    },
                    selection.expected.order().to_vec(),
                )
                .unwrap();
                send(
                    &mut stream,
                    &response(&call, Reply::TargetObserved(current)),
                );
            }
            // First reconcile an older adjacent request, then latest current request.
            for expected_page in [11, 12] {
                let call = read(&mut stream);
                let Message::Request(Command::ReconcileSelection(selection)) = &call.message else {
                    panic!()
                };
                assert_eq!(selection.expected.page().page, id(expected_page));
                assert_eq!(selection.target, id(12));
                send(
                    &mut stream,
                    &response(
                        &call,
                        Reply::SelectionReconciled {
                            current: facts(),
                            target_present: true,
                        },
                    ),
                );
            }
            let mut eof = [0];
            assert_eq!(stream.read(&mut eof).unwrap(), 0);
        });
        let mut connection = ResidentConnection::connect(
            store.clone(),
            directory,
            uid,
            runtime_scope(),
            OperationId(id(50)),
            end(),
        )
        .unwrap();
        for (operation, expected) in [
            (51, facts()),
            (
                52,
                ReaderFacts::new(
                    PageKey {
                        document: id(10),
                        page: id(12),
                    },
                    facts().order().to_vec(),
                )
                .unwrap(),
            ),
        ] {
            assert!(matches!(
                connection
                    .select_adjacent(
                        conversation,
                        OperationId(id(operation)),
                        SelectAdjacent {
                            expected,
                            target: id(12)
                        },
                        end()
                    )
                    .unwrap(),
                Reply::TargetObserved(_)
            ));
        }
        let before = fs::read(&path).unwrap();
        // Each invalid lookup must refuse before any socket request.
        for (conv, original, fresh) in [
            (Uuid::new_v4(), 51, 60),
            (conversation, 99, 60),
            (conversation, 51, 51),
        ] {
            assert!(connection
                .reconcile_attempt(
                    conv,
                    OperationId(id(original)),
                    OperationId(id(fresh)),
                    end()
                )
                .is_err());
        }
        assert!(matches!(
            connection
                .reconcile_attempt(
                    conversation,
                    OperationId(id(51)),
                    OperationId(id(60)),
                    end()
                )
                .unwrap(),
            Reply::SelectionReconciled { .. }
        ));
        assert!(matches!(
            connection
                .reconcile(conversation, OperationId(id(61)), end())
                .unwrap(),
            Reply::SelectionReconciled { .. }
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(store
            .heads(crate::storage::Namespace::Conversation, conversation)
            .unwrap()
            .is_empty());
        connection.close();
        peer.join().unwrap();
    }

    #[test]
    fn selection_loss_partial_wrong_kind_and_indeterminate_block_both_effects() {
        for mode in 0..4 {
            let f = Fixture::new();
            let store = f.open();
            let conversation = Uuid::new_v4();
            let (listener, directory, uid) = endpoint(&f);
            let peer = thread::spawn(move || {
                let mut stream = accept(&listener);
                let call = read(&mut stream);
                match mode {
                    0 => {} // Lost entire reply.
                    1 => stream.write_all(&[0, 0]).unwrap(),
                    2 => send(&mut stream, &response(&call, Reply::Observed(facts()))),
                    _ => send(&mut stream, &response(&call, Reply::Indeterminate)),
                }
            });
            let mut connection = ResidentConnection::connect(
                store.clone(),
                directory,
                uid,
                runtime_scope(),
                OperationId(id(50)),
                end(),
            )
            .unwrap();
            let result = connection.select_adjacent(
                conversation,
                OperationId(id(51)),
                SelectAdjacent {
                    expected: facts(),
                    target: id(12),
                },
                end(),
            );
            if mode == 3 {
                assert_eq!(result.unwrap(), Reply::Indeterminate);
            } else {
                assert!(
                    matches!(result.unwrap_err().lost, LostRequest::Indeterminate { operation, .. } if operation == OperationId(id(51)))
                );
            }
            peer.join().unwrap();
            drop(connection);
            drop(store);
            let store = f.open();
            assert!(claim(&store, conversation, &request(52, 13)).is_err());
            let selection = Frame {
                message: Message::Request(Command::SelectAdjacent(SelectAdjacent {
                    expected: facts(),
                    target: id(12),
                })),
                ..request(53, 13)
            };
            assert!(claim(&store, conversation, &selection).is_err());
            let saved = attempts(&store).unwrap();
            assert_eq!(saved.len(), 1);
            assert_eq!(
                saved[0].reply,
                if mode == 3 {
                    Some(Reply::Indeterminate)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn selection_publication_failure_reopen_uses_actual_stored_outcome() {
        for point in [
            PublicationPoint::Reservation,
            PublicationPoint::Outcome,
            PublicationPoint::AfterOutcome,
        ] {
            for reply in [
                Reply::TargetObserved(
                    ReaderFacts::new(
                        PageKey {
                            document: id(10),
                            page: id(12),
                        },
                        facts().order().to_vec(),
                    )
                    .unwrap(),
                ),
                Reply::Indeterminate,
                Reply::Rejected(Rejection::BeforeDispatch),
            ] {
                let f = Fixture::new();
                let store = f.open();
                let conversation = Uuid::new_v4();
                let (listener, directory, uid) = endpoint(&f);
                let observed_reply = reply.clone();
                let peer = thread::spawn(move || {
                    let mut stream = accept(&listener);
                    if point != PublicationPoint::Reservation {
                        let call = read(&mut stream);
                        send(&mut stream, &response(&call, observed_reply));
                    }
                    let mut eof = [0];
                    assert_eq!(stream.read(&mut eof).unwrap(), 0);
                });
                let mut connection = ResidentConnection::connect(
                    store.clone(),
                    directory,
                    uid,
                    runtime_scope(),
                    OperationId(id(50)),
                    end(),
                )
                .unwrap();
                PUBLICATION_FAULT.with(|fault| fault.set(Some(point)));
                let failure = connection
                    .select_adjacent(
                        conversation,
                        OperationId(id(51)),
                        SelectAdjacent {
                            expected: facts(),
                            target: id(12),
                        },
                        end(),
                    )
                    .unwrap_err();
                peer.join().unwrap();
                drop(connection);
                drop(store);
                let store = f.open();
                let saved = attempts(&store).unwrap();
                if point == PublicationPoint::Reservation {
                    assert_eq!(failure.lost, LostRequest::NoPendingMutation);
                    assert!(saved.is_empty());
                    claim(&store, conversation, &request(52, 13)).unwrap();
                } else {
                    assert_eq!(
                        failure.lost,
                        LostRequest::Indeterminate {
                            operation: OperationId(id(51)),
                            request_sha256: saved[0].request.fingerprint()
                        }
                    );
                    assert!(claim(&store, Uuid::new_v4(), &request(51, 13)).is_err());
                    let stored = point == PublicationPoint::AfterOutcome;
                    assert_eq!(saved[0].reply, stored.then_some(reply.clone()));
                    let selection = Frame {
                        message: Message::Request(Command::SelectAdjacent(SelectAdjacent {
                            expected: facts(),
                            target: id(12),
                        })),
                        ..request(53, 13)
                    };
                    if stored && matches!(reply, Reply::Rejected(_)) {
                        claim(&store, conversation, &request(52, 13)).unwrap();
                    } else {
                        assert!(claim(&store, conversation, &request(52, 13)).is_err());
                        if stored && matches!(reply, Reply::TargetObserved(_)) {
                            claim(&store, conversation, &selection).unwrap();
                        } else {
                            assert!(claim(&store, conversation, &selection).is_err());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn retained_v1_reconciles_read_only_over_v2_without_replay_or_rewrite() {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let state = legacy_state(&store, conversation, Outcome::Reserved {});
        let path = store.paths.data.join(JOURNAL_FILE);
        persist(&path, &state).unwrap();
        let before = fs::read(&path).unwrap();
        let old = HistoricalFrame::decode(V1_REQUEST).unwrap();
        let Message::Request(Command::InsertAfter(insertion)) = old.frame().message.clone() else {
            panic!()
        };
        let expected = insertion.clone();
        let (listener, directory, uid) = endpoint(&f);
        let peer = thread::spawn(move || {
            let mut stream = accept(&listener);
            let call = read(&mut stream); // Strict v2 decoder in read().
            assert_eq!(call.operation, OperationId(id(51)));
            assert_eq!(
                call.message,
                Message::Request(Command::Reconcile(expected.clone()))
            );
            send(
                &mut stream,
                &response(
                    &call,
                    Reply::Reconciled {
                        current: expected.source,
                        target_present: false,
                    },
                ),
            );
            let mut eof = [0];
            assert_eq!(stream.read(&mut eof).unwrap(), 0);
        });
        let mut connection = ResidentConnection::connect(
            store.clone(),
            directory,
            uid,
            runtime_scope(),
            OperationId(id(50)),
            end(),
        )
        .unwrap();
        assert!(matches!(
            connection
                .reconcile(conversation, OperationId(id(51)), end())
                .unwrap(),
            Reply::Reconciled {
                target_present: false,
                ..
            }
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        let failure = connection
            .insert_after(conversation, OperationId(id(52)), insertion, end())
            .unwrap_err();
        assert_eq!(failure.lost, LostRequest::NoPendingMutation);
        connection.close();
        peer.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(attempts(&store).unwrap()[0].request.encoded(), V1_REQUEST);
        assert!(attempts(&store).unwrap()[0].reply.is_none());
    }

    #[test]
    fn malformed_legacy_evidence_refuses_before_connect() {
        let f = Fixture::new();
        let store = f.open();
        let response = HistoricalFrame::decode(V1_RESPONSE).unwrap();
        let state = legacy_state(
            &store,
            Uuid::new_v4(),
            Outcome::HistoricalReply {
                frame: encode(response.frame()).unwrap(),
            },
        );
        let bytes = serde_json::to_vec(&state).unwrap();
        let path = store.paths.data.join(JOURNAL_FILE);
        fs::write(&path, &bytes).unwrap();
        let (listener, directory, uid) = endpoint(&f);
        listener.set_nonblocking(true).unwrap();
        assert!(ResidentConnection::connect(
            store,
            directory,
            uid,
            runtime_scope(),
            OperationId(id(50)),
            end()
        )
        .is_err());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn lost_native_reply_reopens_only_for_original_target_reconciliation() {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let (listener, directory, uid) = endpoint(&f);
        let second_dir = directory.try_clone().unwrap();
        let peer = thread::spawn(move || {
            let mut first = accept(&listener);
            let original = read(&mut first);
            assert!(matches!(
                original.message,
                Message::Request(Command::InsertAfter(_))
            ));
            drop(first);
            let mut next = accept(&listener);
            let reconciliation = read(&mut next);
            let Message::Request(Command::Reconcile(value)) = &reconciliation.message else {
                panic!("must reconcile, never replay")
            };
            let Message::Request(Command::InsertAfter(original_value)) = original.message else {
                panic!()
            };
            assert_eq!(value, &original_value);
            send(
                &mut next,
                &response(
                    &reconciliation,
                    Reply::Reconciled {
                        current: facts(),
                        target_present: false,
                    },
                ),
            );
            let mut eof = [0];
            assert_eq!(next.read(&mut eof).unwrap(), 0);
        });
        let mut connection = ResidentConnection::connect(
            store.clone(),
            directory,
            uid,
            runtime_scope(),
            OperationId(id(50)),
            end(),
        )
        .unwrap();
        let insertion = InsertAfter {
            source: facts(),
            target: id(13),
        };
        let failure = connection
            .insert_after(conversation, OperationId(id(51)), insertion.clone(), end())
            .unwrap_err();
        assert!(
            matches!(failure.lost,LostRequest::Indeterminate{operation,..} if operation==OperationId(id(51)))
        );
        drop(connection);
        drop(store);
        let store = f.open();
        let mut connection = ResidentConnection::connect(
            store.clone(),
            second_dir,
            uid,
            runtime_scope(),
            OperationId(id(52)),
            end(),
        )
        .unwrap();
        assert!(matches!(
            connection
                .reconcile(conversation, OperationId(id(53)), end())
                .unwrap(),
            Reply::Reconciled {
                target_present: false,
                ..
            }
        ));
        assert!(connection
            .insert_after(
                conversation,
                OperationId(id(54)),
                InsertAfter {
                    target: id(14),
                    ..insertion
                },
                end()
            )
            .is_err());
        assert_eq!(attempts(&store).unwrap().len(), 1);
        assert!(attempts(&store).unwrap()[0].reply.is_none());
        peer.join().unwrap();
    }

    #[test]
    fn publication_faults_keep_dispatch_and_retention_boundaries_distinct() {
        // Explicit owned IO-boundary fault injection; real socket/SDK/Store flow.
        // This is not a filesystem power-loss or native persistence test.
        for point in [
            PublicationPoint::Reservation,
            PublicationPoint::Outcome,
            PublicationPoint::AfterOutcome,
        ] {
            let f = Fixture::new();
            let store = f.open();
            let conversation = Uuid::new_v4();
            let (listener, directory, uid) = endpoint(&f);
            let peer = thread::spawn(move || {
                let mut stream = accept(&listener);
                if point != PublicationPoint::Reservation {
                    let request = read(&mut stream);
                    send(
                        &mut stream,
                        &response(
                            &request,
                            Reply::InsertionObserved {
                                target: PageKey {
                                    document: id(10),
                                    page: id(13),
                                },
                                after_order: vec![id(11), id(13), id(12)],
                            },
                        ),
                    );
                }
                let mut eof = [0];
                assert_eq!(stream.read(&mut eof).unwrap(), 0);
            });
            let mut connection = ResidentConnection::connect(
                store.clone(),
                directory,
                uid,
                runtime_scope(),
                OperationId(id(50)),
                end(),
            )
            .unwrap();
            PUBLICATION_FAULT.with(|fault| fault.set(Some(point)));
            let failure = connection
                .insert_after(
                    conversation,
                    OperationId(id(51)),
                    InsertAfter {
                        source: facts(),
                        target: id(13),
                    },
                    end(),
                )
                .unwrap_err();
            let saved = attempts(&store).unwrap();
            if point == PublicationPoint::Reservation {
                assert_eq!(failure.lost, LostRequest::NoPendingMutation);
                assert!(saved.is_empty());
            } else {
                assert_eq!(saved.len(), 1);
                assert_eq!(
                    failure.lost,
                    LostRequest::Indeterminate {
                        operation: OperationId(id(51)),
                        request_sha256: saved[0].request.fingerprint(),
                    }
                );
                assert_eq!(saved[0].request.frame().operation, OperationId(id(51)));
                assert_eq!(
                    saved[0].reply.is_some(),
                    point == PublicationPoint::AfterOutcome
                );
                assert!(claim(&store, conversation, &request(52, 14)).is_err());
            }
            peer.join().unwrap();
            drop(connection);
            drop(store);
            let store = f.open();
            if point != PublicationPoint::Reservation {
                assert!(claim(&store, conversation, &request(53, 14)).is_err());
                let selection = Frame {
                    message: Message::Request(Command::SelectAdjacent(SelectAdjacent {
                        expected: ReaderFacts::new(
                            PageKey {
                                document: id(10),
                                page: id(11),
                            },
                            vec![id(11), id(13), id(12)],
                        )
                        .unwrap(),
                        target: id(13),
                    })),
                    ..request(54, 13)
                };
                assert_eq!(
                    claim(&store, conversation, &selection).is_ok(),
                    point == PublicationPoint::AfterOutcome
                );
                assert!(claim(&store, Uuid::new_v4(), &request(51, 13)).is_err());
            }
        }
    }

    #[test]
    fn successful_reply_is_recorded_without_binding_or_replay() {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let (listener, directory, uid) = endpoint(&f);
        let peer = thread::spawn(move || {
            let mut stream = accept(&listener);
            let request = read(&mut stream);
            send(
                &mut stream,
                &response(
                    &request,
                    Reply::InsertionObserved {
                        target: PageKey {
                            document: id(10),
                            page: id(13),
                        },
                        after_order: vec![id(11), id(13), id(12)],
                    },
                ),
            );
            let mut eof = [0];
            assert_eq!(stream.read(&mut eof).unwrap(), 0);
        });
        let mut connection = ResidentConnection::connect(
            store.clone(),
            directory,
            uid,
            runtime_scope(),
            OperationId(id(50)),
            end(),
        )
        .unwrap();
        assert!(matches!(
            connection
                .insert_after(
                    conversation,
                    OperationId(id(51)),
                    InsertAfter {
                        source: facts(),
                        target: id(13)
                    },
                    end()
                )
                .unwrap(),
            Reply::InsertionObserved { .. }
        ));
        assert!(attempts(&store).unwrap()[0].reply.is_some());
        assert!(store
            .heads(crate::storage::Namespace::Conversation, conversation)
            .unwrap()
            .is_empty());
        assert!(connection
            .insert_after(
                conversation,
                OperationId(id(52)),
                InsertAfter {
                    source: facts(),
                    target: id(14)
                },
                end()
            )
            .is_err());
        peer.join().unwrap();
    }
}
