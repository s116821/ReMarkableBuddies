use super::*;

fn select(operation: u32, expected: ReaderFacts, target: u32) -> Frame {
    Frame {
        message: Message::Request(Command::SelectAdjacent(SelectAdjacent {
            expected,
            target: id(target),
        })),
        ..request(operation, 13)
    }
}
fn selected(target: u32, order: Vec<SdkUuid>) -> ReaderFacts {
    ReaderFacts::new(
        PageKey {
            document: id(10),
            page: id(target),
        },
        order,
    )
    .unwrap()
}
fn retain(store: &Store, frame: &Frame, reply: Reply) -> Result<()> {
    finish(
        store,
        frame,
        Outcome::HistoricalReply {
            frame: encode(&response(frame, reply))?,
        },
    )
}

#[test]
fn selection_current_and_adjacent_success_pin_only_exact_document_target() {
    for target in [11, 12] {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let original = select(50, facts(), target);
        claim(&store, conversation, &original).unwrap();
        retain(
            &store,
            &original,
            Reply::TargetObserved(selected(target, facts().order().to_vec())),
        )
        .unwrap();
        drop(store);
        let store = f.open();
        assert!(claim(&store, conversation, &request(51, 13)).is_err());
        assert!(claim(
            &store,
            conversation,
            &select(52, facts(), if target == 11 { 12 } else { 11 })
        )
        .is_err());
        let foreign = ReaderFacts::new(
            PageKey {
                document: id(20),
                page: id(target),
            },
            vec![id(target)],
        )
        .unwrap();
        assert!(claim(&store, conversation, &select(53, foreign, target)).is_err());
        // Fresh observed order may change; only the native executor can validate it.
        let next = select(54, selected(target, vec![id(target), id(99)]), target);
        claim(&store, conversation, &next).unwrap();
        assert_eq!(attempts(&store).unwrap().len(), 2);
        assert!(claim(&store, conversation, &select(55, facts(), target)).is_err());
    }
}

#[test]
fn observed_insertion_target_survives_later_known_selection_refusal() {
    for no_effect in [true, false] {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let insertion = request(50, 13);
        claim(&store, conversation, &insertion).unwrap();
        let order = vec![id(11), id(13), id(12)];
        retain(
            &store,
            &insertion,
            Reply::InsertionObserved {
                target: PageKey {
                    document: id(10),
                    page: id(13),
                },
                after_order: order.clone(),
            },
        )
        .unwrap();
        let selection = select(51, selected(11, order.clone()), 13);
        claim(&store, conversation, &selection).unwrap();
        if no_effect {
            finish(&store, &selection, Outcome::NotDispatched {}).unwrap();
        } else {
            retain(
                &store,
                &selection,
                Reply::Rejected(Rejection::BeforeDispatch),
            )
            .unwrap();
        }
        drop(store);
        let store = f.open();
        assert!(claim(&store, conversation, &request(52, 14)).is_err());
        assert!(claim(&store, conversation, &select(53, facts(), 12)).is_err());
        claim(&store, conversation, &select(54, selected(13, order), 13)).unwrap();
    }
}

#[test]
fn unknown_selection_and_cross_kind_operation_reuse_refuse_after_restart() {
    for reply in [
        None,
        Some(Reply::Indeterminate),
        Some(Reply::Rejected(Rejection::BeforeDispatch)),
    ] {
        let f = Fixture::new();
        let store = f.open();
        let conversation = Uuid::new_v4();
        let original = select(50, facts(), 12);
        claim(&store, conversation, &original).unwrap();
        if let Some(value) = reply.clone() {
            retain(&store, &original, value).unwrap();
        }
        drop(store);
        let store = f.open();
        assert!(claim(&store, Uuid::new_v4(), &request(50, 13)).is_err());
        assert!(claim(&store, Uuid::new_v4(), &original).is_err());
        if matches!(reply, Some(Reply::Rejected(_))) {
            claim(&store, conversation, &request(51, 13)).unwrap();
        } else {
            assert!(claim(&store, conversation, &request(51, 13)).is_err());
            assert!(claim(&store, conversation, &select(52, facts(), 12)).is_err());
        }
    }
    let f = Fixture::new();
    let store = f.open();
    claim(&store, Uuid::new_v4(), &request(60, 13)).unwrap();
    assert!(claim(&store, Uuid::new_v4(), &select(60, facts(), 12)).is_err());
}

#[test]
fn selection_correlation_and_response_kind_mismatches_preserve_exact_journal() {
    let f = Fixture::new();
    let store = f.open();
    let original = select(50, facts(), 12);
    claim(&store, Uuid::new_v4(), &original).unwrap();
    let path = store.paths.data.join(JOURNAL_FILE);
    let before = fs::read(&path).unwrap();
    let good = response(
        &original,
        Reply::TargetObserved(selected(12, facts().order().to_vec())),
    );
    let mut bad_digest = good.clone();
    if let Message::Response { request_sha256, .. } = &mut bad_digest.message {
        *request_sha256 = [0; 32];
    }
    let mut bad_scope = good.clone();
    bad_scope.scope.connection = id(90);
    let mut bad_sequence = good.clone();
    bad_sequence.sequence += 1;
    let mut bad_operation = good.clone();
    bad_operation.operation = OperationId(id(90));
    for wrong in [
        response(&original, Reply::TargetObserved(facts())),
        response(
            &original,
            Reply::TargetObserved(selected(12, vec![id(12), id(11)])),
        ),
        response(
            &original,
            Reply::TargetObserved(
                ReaderFacts::new(
                    PageKey {
                        document: id(90),
                        page: id(12),
                    },
                    facts().order().to_vec(),
                )
                .unwrap(),
            ),
        ),
        response(
            &original,
            Reply::InsertionObserved {
                target: PageKey {
                    document: id(10),
                    page: id(12),
                },
                after_order: facts().order().to_vec(),
            },
        ),
        bad_digest,
        bad_scope,
        bad_sequence,
        bad_operation,
    ] {
        assert!(finish(
            &store,
            &original,
            Outcome::HistoricalReply {
                frame: encode(&wrong).unwrap()
            }
        )
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let insertion = request(51, 13);
    claim(&store, Uuid::new_v4(), &insertion).unwrap();
    assert!(retain(
        &store,
        &insertion,
        Reply::TargetObserved(selected(13, vec![id(11), id(13), id(12)]))
    )
    .is_err());
}

#[test]
fn format_upgrade_preserves_records_and_rejects_invalid_chronology() {
    let f = Fixture::new();
    let store = f.open();
    let conversation = Uuid::new_v4();
    let path = store.paths.data.join(JOURNAL_FILE);
    let insertion = request(50, 13);
    let state = State {
        format: 1,
        actor: store.actor_id,
        attempts: vec![Attempt {
            conversation,
            frame: encode(&insertion).unwrap(),
            sha256: digest(&insertion).unwrap(),
            outcome: Outcome::NotDispatched {},
        }],
    };
    persist(&path, &state).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    claim(&store, conversation, &select(51, facts(), 12)).unwrap();
    let valid: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(valid["format"], 2);
    assert_eq!(valid["attempts"][0], before["attempts"][0]);
    let mut format1 = valid.clone();
    format1["format"] = 1.into();
    let mut future = valid.clone();
    future["format"] = 3.into();
    let mut chronology = valid.clone();
    chronology["attempts"][0]["outcome"] = serde_json::json!({"state":"reserved"});
    let mut hidden = valid.clone();
    // The original target restriction must survive a later no-effect record.
    hidden["attempts"][0]["outcome"] = serde_json::to_value(Outcome::HistoricalReply {
        frame: encode(&response(
            &insertion,
            Reply::InsertionObserved {
                target: PageKey {
                    document: id(10),
                    page: id(13),
                },
                after_order: vec![id(11), id(13), id(12)],
            },
        ))
        .unwrap(),
    })
    .unwrap();
    hidden["attempts"][1]["outcome"] = serde_json::json!({"state":"not-dispatched"});
    for invalid in [format1, future, chronology, hidden] {
        let bytes = serde_json::to_vec(&invalid).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(attempts(&store).is_err());
        assert!(claim(&store, Uuid::new_v4(), &request(99, 13)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn observed_v1_insertion_allows_new_selection_without_rewriting_old_evidence() {
    let f = Fixture::new();
    let store = f.open();
    let conversation = Uuid::new_v4();
    let path = store.paths.data.join(JOURNAL_FILE);
    let state = legacy_state(
        &store,
        conversation,
        Outcome::HistoricalReply {
            frame: STANDARD.encode(V1_RESPONSE),
        },
    );
    persist(&path, &state).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let Reply::InsertionObserved {
        target,
        after_order,
    } = attempts(&store).unwrap()[0].reply.clone().unwrap()
    else {
        panic!()
    };
    let expected = ReaderFacts::new(target, after_order).unwrap();
    let frame = Frame {
        message: Message::Request(Command::SelectAdjacent(SelectAdjacent {
            expected,
            target: target.page,
        })),
        ..request(800, 13)
    };
    claim(&store, conversation, &frame).unwrap();
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["format"], 2);
    assert_eq!(before["attempts"][0], after["attempts"][0]);
    assert_eq!(attempts(&store).unwrap()[0].request.encoded(), V1_REQUEST);
}

#[test]
fn generation_restore_preserves_selection_reservation_and_original_identity() {
    let f = Fixture::new();
    let store = f.open();
    let backup = f.0.join("before-selection");
    store.export(&backup).unwrap();
    let conversation = Uuid::new_v4();
    let original = select(50, facts(), 12);
    claim(&store, conversation, &original).unwrap();
    let before = fs::read(store.paths.data.join(JOURNAL_FILE)).unwrap();
    store.restore(&backup).unwrap();
    drop(store);
    let store = f.open();
    assert_eq!(
        fs::read(store.paths.data.join(JOURNAL_FILE)).unwrap(),
        before
    );
    assert_eq!(*attempts(&store).unwrap()[0].request.frame(), original);
    assert!(claim(&store, conversation, &request(51, 13)).is_err());
    assert!(claim(&store, conversation, &select(52, facts(), 12)).is_err());
}

#[test]
fn concurrent_mixed_effects_share_one_conversation_reservation() {
    let f = Fixture::new();
    let store = f.open();
    let conversation = Uuid::new_v4();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|n| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let frame = if n % 2 == 0 {
                    request(50 + n, 13)
                } else {
                    select(50 + n, facts(), 12)
                };
                barrier.wait();
                claim(&store, conversation, &frame).is_ok()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .filter_map(|w| w.join().unwrap().then_some(()))
            .count(),
        1
    );
    assert_eq!(attempts(&store).unwrap().len(), 1);
}
