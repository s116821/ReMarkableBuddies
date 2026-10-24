use super::*;
use crate::storage::StorePaths;
use remarkable_open_sdk::{native_adapter::*, OperationId, PageKey, Uuid as SdkUuid};
use std::{fs, path::PathBuf, sync::Arc};

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
    assert_eq!(saved[0].request, original);
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
    assert_eq!(attempts(&store).unwrap()[0].request, original);
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
                        request_sha256: saved[0].request.fingerprint().unwrap(),
                    }
                );
                assert_eq!(saved[0].request.operation, OperationId(id(51)));
                assert_eq!(
                    saved[0].reply.is_some(),
                    point == PublicationPoint::AfterOutcome
                );
                assert!(claim(&store, conversation, &request(52, 14)).is_err());
            }
            peer.join().unwrap();
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
