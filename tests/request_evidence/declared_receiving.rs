//! Composition fixtures use the same real Store/Index helpers as the legacy suite.
use super::*;
use std::cell::Cell;

fn frame(
    l: &Ledger,
    r: &Turn,
    d: SourceDirective,
    p: Option<&dyn Provider>,
) -> Result<FrozenRequestContext> {
    l.source_request_context_with_directive(r.conversation, r.id, None, d, budget(), p, counter)
}
fn prepare(
    l: &Ledger,
    r: &Turn,
    d: SourceDirective,
    p: Option<&dyn Provider>,
) -> Result<PreparedRequestEvidence> {
    l.prepare_request_evidence_with_directive(r.conversation, r.id, None, d, budget(), p, counter)
}
fn resolution_refusal<T>(result: Result<T>, expected: SourceResolutionRefusal) {
    let error = result.err().expect("expected resolution refusal");
    assert_eq!(
        error.downcast_ref::<SourceResolutionRefusal>(),
        Some(&expected)
    );
    assert_eq!(error.chain().count(), 1);
}
fn receiving_refusal<T>(result: Result<T>, expected: ReceivingRefusal) {
    let error = result.err().expect("expected receiving refusal");
    assert_eq!(error.downcast_ref::<ReceivingRefusal>(), Some(&expected));
    assert_eq!(error.chain().count(), 1);
}
fn revise_source(l: &Ledger, s: &ImageUse, change_content: bool) -> Uuid {
    let mut e = l.store().value(Namespace::Source, s.id).unwrap().unwrap();
    e.parents = [e.revision_id].into();
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    if change_content {
        let mut source = s.clone();
        source.purpose = "callback changed source".into();
        e.payload = serde_json::to_value(Record::Source(source)).unwrap();
    }
    let revision = e.revision_id;
    l.store().commit(vec![e], BTreeMap::new()).unwrap();
    revision
}
fn retire_anchor(l: &Ledger, operation: Uuid) {
    let mut e = l
        .store()
        .value(
            Namespace::Conversation,
            RequestEvidence::identity(operation),
        )
        .unwrap()
        .unwrap();
    e.parents = [e.revision_id].into();
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.kind = Kind::Tombstone;
    e.payload = serde_json::Value::Null;
    e.media_descriptors.clear();
    l.store().commit(vec![e], BTreeMap::new()).unwrap();
}

#[test]
fn explicit_inherited_and_narrowed_frames_match_legacy_without_writes() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
    let index = Index::build(document(source.observation.document, "v1")).unwrap();
    let bound = record_bound(&l, &prior, &source, &index, 1);
    let blank = Uuid::new_v4();
    l.persist_request_evidence(blank, 9999, &no_document(&l, &prior))
        .unwrap();
    let current = append(&l, user(c, "orbit\nwhy? "), vec![]);
    let before = serde_json::to_value(l.store().manifests().unwrap()).unwrap();
    let old_counts = Cell::new(0);
    let old = l
        .source_request_context(
            c,
            current.id,
            None,
            association(&prior, &source, index.key()),
            budget(),
            Some(&index),
            |text| {
                old_counts.set(old_counts.get() + 1);
                counter(text)
            },
        )
        .unwrap();
    let spy = Spy::new(&index);
    let new_counts = Cell::new(0);
    let explicit = l
        .source_request_context_with_directive(
            c,
            current.id,
            None,
            SourceDirective::Explicit(association(&prior, &source, index.key())),
            budget(),
            Some(&spy),
            |text| {
                new_counts.set(new_counts.get() + 1);
                counter(text)
            },
        )
        .unwrap();
    assert_eq!(explicit.encoded_json(), old.encoded_json());
    assert_eq!(new_counts.get(), old_counts.get());
    let inherited = frame(
        &l,
        &current,
        inherit(&prior, Some(bound), Some(index.key().clone())),
        Some(&spy),
    )
    .unwrap();
    assert_eq!(inherited.encoded_json(), old.encoded_json());
    assert_eq!(inherited.request().text.as_deref(), Some("orbit\nwhy? "));
    assert_eq!(inherited.retrieval().unwrap().passages[0].page, Some(9));
    let range = TurnRange {
        start: current.sequence,
        end_exclusive: current.sequence + 1,
    };
    let narrowed = l
        .source_request_context_with_directive(
            c,
            current.id,
            Some(range),
            inherit(&prior, Some(bound), Some(index.key().clone())),
            budget(),
            Some(&spy),
            counter,
        )
        .unwrap();
    assert!(narrowed.history().is_empty());
    assert!(
        matches!(narrowed.source(), SourceAssociation::Original { origin_turn, .. } if *origin_turn == prior.id)
    );
    let prepared = prepare(
        &l,
        &current,
        inherit(&prior, Some(bound), Some(index.key().clone())),
        Some(&spy),
    )
    .unwrap();
    assert_eq!(prepared.context().encoded_json(), old.encoded_json());
    assert_eq!(spy.calls.borrow().len(), 4);
    assert!(spy
        .calls
        .borrow()
        .iter()
        .all(|(_, q, _)| q == "orbit\nwhy? "));
    let old_blank = l
        .source_request_context(
            c,
            current.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap();
    let inherited_blank =
        frame(&l, &current, inherit(&prior, Some(blank), None), Some(&spy)).unwrap();
    assert_eq!(inherited_blank.encoded_json(), old_blank.encoded_json());
    let clear = frame(
        &l,
        &current,
        SourceDirective::Explicit(RequestSource::NoDocument),
        Some(&spy),
    )
    .unwrap();
    assert_eq!(clear.encoded_json(), old_blank.encoded_json());
    assert_eq!(spy.calls.borrow().len(), 4);
    assert_eq!(
        serde_json::to_value(l.store().manifests().unwrap()).unwrap(),
        before
    );
}

#[test]
fn counter_changes_keep_captured_frames_but_reject_fresh_publication() {
    for preparation in [false, true] {
        for change in 0..4 {
            let f = Fixture::new();
            let l = f.ledger();
            let c = create(&l);
            let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
            let index = Index::build(document(source.observation.document, "v1")).unwrap();
            let no_source = change == 3;
            let op = if no_source {
                let op = Uuid::new_v4();
                l.persist_request_evidence(op, 1, &no_document(&l, &prior))
                    .unwrap();
                op
            } else {
                record_bound(&l, &prior, &source, &index, 1)
            };
            let current = append(&l, user(c, "orbit"), vec![]);
            let choice = if no_source {
                RequestSource::NoDocument
            } else {
                association(&prior, &source, index.key())
            };
            let baseline = l
                .source_request_context(
                    c,
                    current.id,
                    None,
                    choice,
                    budget(),
                    Some(&index),
                    counter,
                )
                .unwrap();
            let before = l.store().manifests().unwrap().len();
            let changed = Cell::new(false);
            let calls = Cell::new(0);
            let callback = |text: &str| {
                calls.set(calls.get() + 1);
                if !changed.replace(true) {
                    match change {
                        0 => {
                            revise_source(&l, &source, true);
                        }
                        1 | 3 => retire_anchor(&l, op),
                        2 => {
                            append(&l, user(c, "callback changed history"), vec![]);
                        }
                        _ => unreachable!(),
                    }
                }
                counter(text)
            };
            let spy = Spy::new(&index);
            let directive = inherit(&prior, Some(op), (!no_source).then(|| index.key().clone()));
            if preparation {
                let prepared = l
                    .prepare_request_evidence_with_directive(
                        c,
                        current.id,
                        None,
                        directive,
                        budget(),
                        Some(&spy),
                        callback,
                    )
                    .unwrap();
                assert_eq!(prepared.context().encoded_json(), baseline.encoded_json());
                let write = Uuid::new_v4();
                assert!(l.persist_request_evidence(write, 2, &prepared).is_err());
                assert!(l
                    .store()
                    .value(Namespace::Conversation, RequestEvidence::identity(write))
                    .unwrap()
                    .is_none());
            } else {
                let context = l
                    .source_request_context_with_directive(
                        c,
                        current.id,
                        None,
                        directive,
                        budget(),
                        Some(&spy),
                        callback,
                    )
                    .unwrap();
                assert_eq!(context.encoded_json(), baseline.encoded_json());
            }
            assert!(changed.get());
            assert_eq!(calls.get(), 2);
            assert_eq!(spy.calls.borrow().len(), usize::from(!no_source));
            assert_eq!(l.store().manifests().unwrap().len(), before + 1);
        }
    }
}

#[test]
fn provider_mutation_is_once_outside_locks_and_cannot_publish_stale_capture() {
    struct Writing<'a> {
        ledger: &'a Ledger,
        index: &'a Index,
        conversation: Uuid,
        calls: Cell<usize>,
    }
    impl Provider for Writing<'_> {
        fn query(
            &self,
            key: &IndexKey,
            text: &str,
            n: usize,
            bytes: usize,
            neighbors: bool,
        ) -> Result<Retrieved> {
            self.calls.set(self.calls.get() + 1);
            append(
                self.ledger,
                user(self.conversation, "provider changed history"),
                vec![],
            );
            self.index.query(key, text, n, bytes, neighbors)
        }
    }
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
    let index = Index::build(document(source.observation.document, "v1")).unwrap();
    let op = record_bound(&l, &prior, &source, &index, 1);
    let current = append(&l, user(c, "orbit"), vec![]);
    let provider = Writing {
        ledger: &l,
        index: &index,
        conversation: c,
        calls: Cell::new(0),
    };
    let read = frame(
        &l,
        &current,
        inherit(&prior, Some(op), Some(index.key().clone())),
        Some(&provider),
    )
    .unwrap();
    assert_eq!(provider.calls.get(), 1);
    assert!(!read
        .history()
        .iter()
        .any(|t| t.text.as_deref() == Some("provider changed history")));
    let prepared = prepare(
        &l,
        &current,
        inherit(&prior, Some(op), Some(index.key().clone())),
        Some(&provider),
    )
    .unwrap();
    assert_eq!(prepared.context().encoded_json(), read.encoded_json());
    let before = l.store().manifests().unwrap().len();
    assert!(l
        .persist_request_evidence(Uuid::new_v4(), 2, &prepared)
        .is_err());
    assert_eq!(provider.calls.get(), 2);
    assert_eq!(l.store().manifests().unwrap().len(), before);
}

#[test]
fn captured_heads_generation_and_unrelated_scope_keep_existing_guards() {
    for change in 0..5 {
        let f = Fixture::new();
        let l = f.ledger();
        let c = create(&l);
        let other = create(&l);
        let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
        let index = Index::build(document(source.observation.document, "v1")).unwrap();
        let op = record_bound(&l, &prior, &source, &index, 1);
        let current = append(&l, user(c, "orbit"), vec![]);
        let spy = Spy::new(&index);
        let prepared = prepare(
            &l,
            &current,
            inherit(&prior, Some(op), Some(index.key().clone())),
            Some(&spy),
        )
        .unwrap();
        match change {
            0 => retire_anchor(&l, op),
            1 => {
                revise_source(&l, &source, false);
            }
            2 => {
                append(&l, user(c, "later"), vec![]);
            }
            3 => {
                let backup = f.0.join("backup");
                l.store().export(&backup).unwrap();
                l.store().restore(&backup).unwrap();
            }
            4 => {
                append(&l, user(other, "unrelated"), vec![]);
            }
            _ => unreachable!(),
        }
        let before = l.store().manifests().unwrap().len();
        let result = l.persist_request_evidence(Uuid::new_v4(), 2, &prepared);
        assert_eq!(result.is_ok(), change == 4);
        assert_eq!(spy.calls.borrow().len(), 1);
        assert_eq!(
            l.store().manifests().unwrap().len(),
            before + usize::from(change == 4)
        );
    }
}

#[test]
fn equivalent_legacy_frames_keep_historical_retries_after_reopen_and_delete() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
    let index = Index::build(document(source.observation.document, "v1")).unwrap();
    let original = record_bound(&l, &prior, &source, &index, 1);
    let current = append(&l, user(c, "orbit"), vec![]);
    let legacy = l
        .prepare_request_evidence(
            c,
            current.id,
            None,
            association(&prior, &source, index.key()),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    let spy = Spy::new(&index);
    let inherited = prepare(
        &l,
        &current,
        inherit(&prior, Some(original), Some(index.key().clone())),
        Some(&spy),
    )
    .unwrap();
    assert_eq!(
        legacy.context().encoded_json(),
        inherited.context().encoded_json()
    );
    let operation = Uuid::new_v4();
    let ack = l
        .persist_request_evidence(operation, 2, &legacy)
        .unwrap()
        .historical;
    let before = l.store().manifests().unwrap().len();
    assert_eq!(
        l.persist_request_evidence(operation, 2, &inherited)
            .unwrap()
            .historical,
        ack
    );
    let old_record = serde_json::to_value(
        l.request_evidence(c, prior.id, original)
            .unwrap()
            .unwrap()
            .record,
    )
    .unwrap();
    let new_record = serde_json::to_value(
        l.request_evidence(c, current.id, operation)
            .unwrap()
            .unwrap()
            .record,
    )
    .unwrap();
    assert_eq!(
        old_record.as_object().unwrap().keys().collect::<Vec<_>>(),
        new_record.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(l.store().manifests().unwrap().len(), before);
    drop(l);
    let l = f.ledger();
    assert_eq!(
        l.persist_request_evidence(operation, 2, &inherited)
            .unwrap()
            .historical,
        ack
    );
    l.delete(c, Uuid::new_v4(), heads(&l, c), None).unwrap();
    let deleted = l.store().manifests().unwrap().len();
    assert_eq!(
        l.persist_request_evidence(operation, 2, &inherited)
            .unwrap()
            .historical,
        ack
    );
    assert_eq!(
        l.request_evidence(c, current.id, operation)
            .unwrap()
            .unwrap()
            .acknowledgment,
        ack
    );
    assert_eq!(l.store().manifests().unwrap().len(), deleted);
    assert_eq!(spy.calls.borrow().len(), 1);
}

#[test]
fn anchors_keys_changed_content_and_complete_budgets_refuse_without_fallback() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
    let index = Index::build(document(source.observation.document, "v1")).unwrap();
    let op = record_bound(&l, &prior, &source, &index, 1);
    let blank = Uuid::new_v4();
    l.persist_request_evidence(blank, 99, &no_document(&l, &prior))
        .unwrap();
    let current = append(&l, user(c, "orbit"), vec![]);
    let spy = Spy::new(&index);
    for (directive, refusal) in [
        (
            inherit(&prior, None, Some(index.key().clone())),
            SourceResolutionRefusal::AnchorRequired,
        ),
        (
            inherit(&prior, Some(Uuid::new_v4()), None),
            SourceResolutionRefusal::AnchorUnavailable,
        ),
        (
            inherit(&prior, Some(op), None),
            SourceResolutionRefusal::IndependentKeyRequired,
        ),
        (
            inherit(&prior, Some(blank), Some(index.key().clone())),
            SourceResolutionRefusal::UnexpectedKey,
        ),
    ] {
        resolution_refusal(frame(&l, &current, directive.clone(), Some(&spy)), refusal);
        resolution_refusal(prepare(&l, &current, directive, Some(&spy)), refusal);
    }
    resolution_refusal(
        frame(
            &l,
            &prior,
            inherit(&prior, Some(op), Some(index.key().clone())),
            Some(&spy),
        ),
        SourceResolutionRefusal::NotEarlier,
    );
    let foreign = create(&l);
    let foreign_request = append(&l, user(foreign, "foreign"), vec![]);
    resolution_refusal(
        frame(
            &l,
            &foreign_request,
            inherit(&prior, Some(op), Some(index.key().clone())),
            Some(&spy),
        ),
        SourceResolutionRefusal::AnchorUnavailable,
    );
    let mut bad_key = index.key().clone();
    bad_key.document = Uuid::new_v4();
    resolution_refusal(
        frame(
            &l,
            &current,
            inherit(&prior, Some(op), Some(bad_key)),
            Some(&spy),
        ),
        SourceResolutionRefusal::InvalidSource,
    );
    assert!(spy.calls.borrow().is_empty());
    let fresh = Index::build(document(source.observation.document, "v2")).unwrap();
    let refreshed = frame(
        &l,
        &current,
        inherit(&prior, Some(op), Some(fresh.key().clone())),
        Some(&fresh),
    )
    .unwrap();
    assert_eq!(&refreshed.retrieval().unwrap().source, fresh.key());
    receiving_refusal(
        frame(
            &l,
            &current,
            inherit(&prior, Some(op), Some(index.key().clone())),
            Some(&fresh),
        ),
        ReceivingRefusal::RetrievalUnavailable,
    );
    let no_source = frame(&l, &current, inherit(&prior, Some(blank), None), None).unwrap();
    let mut exact = budget();
    exact.max_encoded_bytes = no_source.encoded_json().len();
    assert!(l
        .source_request_context_with_directive(
            c,
            current.id,
            None,
            inherit(&prior, Some(blank), None),
            exact.clone(),
            None,
            counter
        )
        .is_ok());
    exact.max_encoded_bytes -= 1;
    receiving_refusal(
        l.source_request_context_with_directive(
            c,
            current.id,
            None,
            inherit(&prior, Some(blank), None),
            exact,
            None,
            counter,
        ),
        ReceivingRefusal::EncodedLimit,
    );
    let mut tokens = budget();
    tokens.context.provider_token_limit = Some(1);
    assert!(l
        .source_request_context_with_directive(
            c,
            current.id,
            None,
            inherit(&prior, Some(blank), None),
            tokens,
            None,
            counter
        )
        .unwrap_err()
        .downcast_ref::<ContextRefusal>()
        .is_some_and(|e| matches!(e, ContextRefusal::SelectionRequired { .. })));
    let mut frame_tokens = budget();
    let limit = no_source.counted_tokens() - 1;
    frame_tokens.context.provider_token_limit = Some(limit);
    receiving_refusal(
        l.source_request_context_with_directive(
            c,
            current.id,
            None,
            inherit(&prior, Some(blank), None),
            frame_tokens,
            None,
            counter,
        ),
        ReceivingRefusal::TokenLimit {
            tokens: no_source.counted_tokens(),
            limit,
        },
    );
    revise_source(&l, &source, true);
    resolution_refusal(
        frame(
            &l,
            &current,
            inherit(&prior, Some(op), Some(index.key().clone())),
            Some(&spy),
        ),
        SourceResolutionRefusal::ChangedSource,
    );
    assert!(spy.calls.borrow().is_empty());
}

#[test]
fn new_history_errors_are_sanitized_and_legacy_validation_order_is_preserved() {
    for conflict in [false, true] {
        let f = Fixture::new();
        let l = f.ledger();
        let c = create(&l);
        let prior = append(&l, user(c, "prior"), vec![]);
        let op = Uuid::new_v4();
        l.persist_request_evidence(op, 1, &no_document(&l, &prior))
            .unwrap();
        let current = append(&l, user(c, "current"), vec![]);
        if conflict {
            let mut e = l
                .store()
                .value(Namespace::Conversation, RequestEvidence::identity(op))
                .unwrap()
                .unwrap();
            e.revision_id = Uuid::new_v4();
            e.operation_id = Uuid::new_v4();
            let bytes = serde_json::to_vec(&e).unwrap();
            let hash = digest(&bytes);
            l.store()
                .import(
                    Manifest {
                        format: FORMAT,
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::LocalTransaction,
                        records: vec![ObjectRef {
                            sha256: hash.clone(),
                            bytes: bytes.len() as u64,
                        }],
                        record_namespaces: BTreeMap::from([(
                            hash.clone(),
                            Namespace::Conversation,
                        )]),
                        media: vec![],
                        media_coverage: vec![],
                    },
                    BTreeMap::from([(hash, bytes)]),
                )
                .unwrap();
        } else {
            let record = l.request_evidence(c, prior.id, op).unwrap().unwrap().record;
            let generation: Uuid = files::json(&f.0.join("data/CURRENT"), 128).unwrap();
            let path =
                f.0.join("data/generations")
                    .join(generation.to_string())
                    .join("objects")
                    .join(&record.turns[0].object.sha256);
            assert!(path
                .canonicalize()
                .unwrap()
                .starts_with(f.0.canonicalize().unwrap()));
            fs::write(path, b"private history/path must not escape").unwrap();
        }
        let before = l.store().manifests().unwrap().len();
        receiving_refusal(
            frame(&l, &current, inherit(&prior, Some(op), None), None),
            ReceivingRefusal::InvalidHistory,
        );
        resolution_refusal(
            prepare(&l, &current, inherit(&prior, Some(op), None), None),
            SourceResolutionRefusal::HistoryUnavailable,
        );
        let mut invalid = budget();
        invalid.context.provider_token_limit = None;
        for error in [
            l.source_request_context(
                c,
                current.id,
                None,
                RequestSource::NoDocument,
                invalid.clone(),
                None,
                counter,
            )
            .unwrap_err(),
            l.prepare_request_evidence(
                c,
                current.id,
                None,
                RequestSource::NoDocument,
                invalid.clone(),
                None,
                counter,
            )
            .err()
            .unwrap(),
            l.source_request_context_with_directive(
                c,
                current.id,
                None,
                inherit(&prior, Some(op), None),
                invalid.clone(),
                None,
                counter,
            )
            .unwrap_err(),
            l.prepare_request_evidence_with_directive(
                c,
                current.id,
                None,
                inherit(&prior, Some(op), None),
                invalid,
                None,
                counter,
            )
            .err()
            .unwrap(),
        ] {
            assert!(matches!(
                error.downcast_ref::<ContextRefusal>(),
                Some(ContextRefusal::UnknownProviderBudget)
            ));
        }
        assert_eq!(l.store().manifests().unwrap().len(), before);
    }
}

#[test]
fn metadata_omission_and_equal_content_successors_keep_exact_captured_references() {
    let f = Fixture::new();
    let l = f.ledger();
    let c = create(&l);
    let (prior, source) = with_source(&l, user(c, "acceleration"), Uuid::new_v4());
    let index = Index::build(document(source.observation.document, "v1")).unwrap();
    let op = record_bound(&l, &prior, &source, &index, 1);
    let current = append(&l, user(c, "orbit"), vec![]);
    let revision = revise_source(&l, &source, false);
    let target_f = Fixture::new();
    let target = target_f.ledger();
    replicate(l.store(), target.store(), true, false);
    let range = TurnRange {
        start: current.sequence,
        end_exclusive: current.sequence + 1,
    };
    let prepared = target
        .prepare_request_evidence_with_directive(
            c,
            current.id,
            Some(range),
            inherit(&prior, Some(op), Some(index.key().clone())),
            budget(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert!(prepared.context().history().is_empty());
    assert!(prepared.context().missing_media().contains(&source.image));
    assert!(
        matches!(prepared.context().source(), SourceAssociation::Original { origin_turn, .. } if *origin_turn == prior.id)
    );
    let write = Uuid::new_v4();
    target
        .persist_request_evidence(write, 2, &prepared)
        .unwrap();
    let record = target
        .request_evidence(c, current.id, write)
        .unwrap()
        .unwrap()
        .record;
    let reference = record
        .sources
        .iter()
        .find(|r| r.record == source.id)
        .unwrap();
    assert_eq!(reference.revision, revision);
    let historical = target
        .request_evidence(c, prior.id, op)
        .unwrap()
        .unwrap()
        .record;
    assert_ne!(reference.revision, historical.sources[0].revision);
    drop(target);
    let target = target_f.ledger();
    let read = target
        .request_evidence(c, current.id, write)
        .unwrap()
        .unwrap();
    assert_eq!(read.record, record);
    assert!(read.unavailable_media.contains(&source.image));
}
