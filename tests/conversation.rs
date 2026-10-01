use remarkable_reader_buddy::{conversation::*, storage::*};
use std::{fs, path::PathBuf, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-conversation-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("secrets"),
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
fn turn(conversation: Uuid, mode: Mode, text: &str) -> Turn {
    Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange: Uuid::new_v4(),
        sequence: 0,
        role: Role::User,
        mode,
        outcome: Outcome::Interpreted,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 9,
        updated_ms: 9,
        completion: None,
    }
}
fn observation() -> SourceObservation {
    SourceObservation {
        document: Uuid::new_v4(),
        page: Uuid::new_v4(),
        session: "synthetic-session".into(),
        visit: "synthetic-visit".into(),
        content_revision: Some("r1".into()),
        order_revision: Some("o1".into()),
        capability_revision: "fixture-v1".into(),
        evidence_procedure: "synthetic".into(),
    }
}
fn expected(ledger: &Ledger, id: Uuid) -> ExpectedHeads {
    ledger.expected(Namespace::Conversation, id).unwrap()
}
fn create(ledger: &Ledger) -> Uuid {
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 10).unwrap();
    id
}

#[test]
fn reopen_mixed_modes_preserves_exact_chronology_and_budget_refuses() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let id = create(&ledger);
    for (mode, text) in [
        (Mode::Reader, "first exact request"),
        (Mode::Writer, "writer\ntext"),
        (Mode::Reader, "reader again"),
    ] {
        ledger
            .append(
                Uuid::new_v4(),
                expected(&ledger, id),
                turn(id, mode, text),
                vec![],
            )
            .unwrap();
    }
    drop(ledger);
    drop(store);
    let ledger = Ledger::new(fixture.open());
    let budget = ContextBudget {
        max_text_bytes: 1000,
        max_turns: 3,
        provider_token_limit: Some(100),
    };
    let context = ledger
        .context(id, &budget, |turns| Ok(turns.len()))
        .unwrap();
    assert_eq!(
        context.turns.iter().map(|t| t.sequence).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(context.turns[1].text.as_deref(), Some("writer\ntext"));
    assert_eq!(context.turns[1].mode, Mode::Writer);
    assert!(ledger
        .context(
            id,
            &ContextBudget {
                max_turns: 2,
                ..budget.clone()
            },
            |_| Ok(1)
        )
        .is_err());
    assert!(ledger
        .context(
            id,
            &ContextBudget {
                provider_token_limit: None,
                ..budget
            },
            |_| Ok(1)
        )
        .is_err());
    assert_eq!(
        ledger
            .inspect(id, false)
            .unwrap()
            .iter()
            .filter(|r| matches!(r, Record::Turn(_)))
            .count(),
        3
    );
}

#[test]
fn concurrent_identical_operation_and_lost_ack_commit_once() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let id = create(&ledger);
    let operation = Uuid::new_v4();
    let request = turn(id, Mode::Reader, "one request");
    let parent = expected(&ledger, id);
    store.set_fault(Fault::AfterCommit).unwrap();
    let mut threads = vec![];
    for _ in 0..2 {
        let (store, request, parent) = (store.clone(), request.clone(), parent.clone());
        threads.push(std::thread::spawn(move || {
            Ledger::new(store)
                .append(operation, parent, request, vec![])
                .unwrap()
                .historical
        }));
    }
    let first = threads.remove(0).join().unwrap();
    let second = threads.remove(0).join().unwrap();
    assert_eq!(first, second);
    assert_eq!(
        ledger
            .inspect(id, false)
            .unwrap()
            .iter()
            .filter(|r| matches!(r, Record::Turn(_)))
            .count(),
        1
    );
    let mut changed = request;
    changed.text = Some("changed".into());
    assert!(ledger.append(operation, parent, changed, vec![]).is_err());
}

#[test]
fn full_head_cas_refuses_stale_append_without_partial_turn() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let old = expected(&ledger, id);
    ledger
        .append(
            Uuid::new_v4(),
            old.clone(),
            turn(id, Mode::Reader, "winner"),
            vec![],
        )
        .unwrap();
    let loser = turn(id, Mode::Writer, "loser");
    assert!(ledger
        .append(Uuid::new_v4(), old, loser.clone(), vec![])
        .is_err());
    assert!(ledger
        .store()
        .value(Namespace::Conversation, loser.id)
        .unwrap()
        .is_none());
}

#[test]
fn draft_is_excluded_until_verified_transition_and_exact_retry() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let mut draft = turn(id, Mode::Reader, "generated answer");
    draft.role = Role::Assistant;
    draft.outcome = Outcome::Generated;
    ledger
        .append(Uuid::new_v4(), expected(&ledger, id), draft.clone(), vec![])
        .unwrap();
    let budget = ContextBudget {
        max_text_bytes: 100,
        max_turns: 10,
        provider_token_limit: Some(100),
    };
    assert!(ledger
        .context(id, &budget, |_| Ok(0))
        .unwrap()
        .turns
        .is_empty());
    draft.outcome = Outcome::Completed;
    assert!(ledger
        .advance(
            Uuid::new_v4(),
            expected(&ledger, id),
            expected(&ledger, draft.id),
            draft.clone()
        )
        .is_err());
    draft.completion = Some(CompletionEvidence {
        native_operation: Uuid::new_v4(),
        exact_text: "generated answer".into(),
        observation: observation(),
        procedure: "synthetic output verification".into(),
    });
    let operation = Uuid::new_v4();
    let root = expected(&ledger, id);
    let head = expected(&ledger, draft.id);
    let result = ledger
        .advance(operation, root.clone(), head.clone(), draft.clone())
        .unwrap();
    assert_eq!(
        ledger
            .advance(operation, root, head, draft)
            .unwrap()
            .historical,
        result.historical
    );
    assert_eq!(
        ledger.context(id, &budget, |_| Ok(1)).unwrap().turns.len(),
        1
    );
}

fn receipt(
    conversation: Uuid,
    operation: Uuid,
    source: SourceObservation,
    target: Uuid,
) -> BindingReceipt {
    BindingReceipt {
        operation,
        conversation,
        request_fingerprint: "immutable synthetic request".into(),
        adapter_fingerprint: "synthetic adapter".into(),
        intended_target: Some(target),
        observed_document: source.document,
        observed_page: target,
        expected_order: vec![source.page],
        observed_order: vec![source.page, target],
        source,
        persisted_revision: "synthetic persisted revision".into(),
        observed_ms: 12,
        evidence_origin: "synthetic".into(),
    }
}

#[test]
fn atomic_page_claim_and_deleted_retry_never_resurrect() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let a = create(&ledger);
    let b = create(&ledger);
    let operation = Uuid::new_v4();
    let target = Uuid::new_v4();
    let source = observation();
    let binding = Ledger::binding_id(source.document, target);
    let claim = receipt(a, operation, source.clone(), target);
    let old = expected(&ledger, a);
    let applied = ledger
        .bind(
            operation,
            old.clone(),
            ExpectedHeads::default(),
            claim.clone(),
        )
        .unwrap();
    let other_operation = Uuid::new_v4();
    assert!(ledger
        .bind(
            other_operation,
            expected(&ledger, b),
            expected(&ledger, binding),
            receipt(b, other_operation, source.clone(), target)
        )
        .is_err());
    let competing_operation = Uuid::new_v4();
    assert!(ledger
        .bind(
            competing_operation,
            expected(&ledger, a),
            ExpectedHeads::default(),
            receipt(a, competing_operation, source, Uuid::new_v4())
        )
        .is_err());
    let deletion = Uuid::new_v4();
    let delete_root = expected(&ledger, a);
    let delete_binding = expected(&ledger, binding);
    ledger
        .delete(
            a,
            deletion,
            delete_root.clone(),
            Some(delete_binding.clone()),
        )
        .unwrap();
    let retry = ledger
        .bind(operation, old, ExpectedHeads::default(), claim)
        .unwrap();
    assert_eq!(retry.historical, applied.historical);
    assert_eq!(retry.current_root[0].kind, Kind::Tombstone);
    assert_eq!(retry.current_binding[0].kind, Kind::Tombstone);
    assert!(ledger
        .append(
            Uuid::new_v4(),
            expected(&ledger, a),
            turn(a, Mode::Reader, "no revival"),
            vec![]
        )
        .is_err());
    assert!(ledger.inspect(a, false).is_err());
    assert!(ledger.inspect(a, true).is_ok());
    assert_eq!(
        ledger
            .delete(a, deletion, delete_root, Some(delete_binding))
            .unwrap()
            .current_root[0]
            .kind,
        Kind::Tombstone
    );
}

#[test]
fn exact_shared_media_survives_logical_deletion_and_reports_absence() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let a = create(&ledger);
    let b = create(&ledger);
    let bytes = include_bytes!("fixtures/trigger-dismiss/open.png");
    let image = Media {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    };
    ledger.stage_image(&image, &bytes[..]).unwrap();
    let mut surviving_source = Uuid::nil();
    for conversation in [a, b] {
        let mut request = turn(conversation, Mode::Reader, "uses exact source");
        let id = Uuid::new_v4();
        request.sources.push(id);
        let source = ImageUse {
            id,
            conversation,
            turn: request.id,
            image: image.clone(),
            observation: observation(),
            width: 1404,
            height: 1872,
            purpose: "overview".into(),
            ordinal: 0,
            parent: None,
            parent_dimensions: None,
            crop: None,
            transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            target: None,
            captured_ms: 1,
        };
        ledger
            .append(
                Uuid::new_v4(),
                expected(&ledger, conversation),
                request,
                vec![source],
            )
            .unwrap();
        surviving_source = id;
    }
    ledger
        .delete(a, Uuid::new_v4(), expected(&ledger, a), None)
        .unwrap();
    let retained = ledger.retained_media(a).unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].retained_revision_references, 2);
    assert!(retained[0].available);
    assert_eq!(ledger.image(surviving_source).unwrap(), bytes);
    let budget = ContextBudget {
        max_text_bytes: 100,
        max_turns: 10,
        provider_token_limit: Some(100),
    };
    assert!(ledger
        .context(b, &budget, |_| Ok(1))
        .unwrap()
        .missing_media
        .is_empty());
    let generation: Uuid = files::json(&ledger.store().paths.data.join("CURRENT"), 128).unwrap();
    fs::remove_file(
        ledger
            .store()
            .paths
            .data
            .join("generations")
            .join(generation.to_string())
            .join("objects")
            .join(&image.sha256),
    )
    .unwrap();
    assert_eq!(
        ledger.context(b, &budget, |_| Ok(1)).unwrap().missing_media,
        vec![image]
    );
}

fn prepared_input(conversation: Uuid) -> (Turn, ImageInput) {
    let bytes = include_bytes!("fixtures/trigger-dismiss/open.png").to_vec();
    let dimensions = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    let mut request = turn(conversation, Mode::Reader, "");
    request.outcome = Outcome::Prepared;
    request.text = None;
    let id = Uuid::new_v4();
    request.sources.push(id);
    let source = ImageUse {
        id,
        conversation,
        turn: request.id,
        image: Media {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
            media_type: "image/png".into(),
        },
        observation: observation(),
        width: dimensions.0,
        height: dimensions.1,
        purpose: "overview".into(),
        ordinal: 0,
        parent: None,
        parent_dimensions: None,
        crop: None,
        transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        target: None,
        captured_ms: 1,
    };
    (
        request,
        ImageInput {
            source,
            bytes,
            parent_bytes: None,
        },
    )
}

#[test]
fn prepared_batch_is_exact_and_reused_without_recapture() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let (request, input) = prepared_input(id);
    let source = input.source.clone();
    let bytes = input.bytes.clone();
    let operation = Uuid::new_v4();
    let root = expected(&ledger, id);
    let prepared = ledger
        .prepare_request(operation, root.clone(), request.clone(), vec![input])
        .unwrap();
    assert_eq!(prepared.images, vec![(source.id, bytes.clone())]);
    let retry = ledger
        .prepare_request(
            operation,
            root,
            request,
            vec![ImageInput {
                source,
                bytes: vec![],
                parent_bytes: None,
            }],
        )
        .unwrap();
    assert_eq!(retry.images, prepared.images);
    assert_eq!(
        ledger.prepared_images(prepared.turn).unwrap().images,
        prepared.images
    );
}

#[test]
fn storage_failure_or_bad_evidence_prevents_dispatch() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let id = create(&ledger);
    let (request, input) = prepared_input(id);
    let turn_id = request.id;
    store.set_fault(Fault::BeforeCommit).unwrap();
    let mut provider_calls = 0;
    let mut mutations = 0;
    if ledger
        .prepare_request(Uuid::new_v4(), expected(&ledger, id), request, vec![input])
        .is_ok()
    {
        provider_calls += 1;
        mutations += 1;
    }
    assert_eq!((provider_calls, mutations), (0, 0));
    assert!(store
        .value(Namespace::Conversation, turn_id)
        .unwrap()
        .is_none());
    let (request, mut input) = prepared_input(id);
    input.source.width += 1;
    assert!(ledger
        .prepare_request(Uuid::new_v4(), expected(&ledger, id), request, vec![input])
        .is_err());
}

#[test]
fn uncertain_export_discovery_requires_exact_unique_marker() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let operation = Uuid::new_v4();
    let association = ExportAssociation {
        id: Uuid::new_v4(),
        conversation: id,
        backend: "fake-backend".into(),
        native_note: None,
        source_scope: "all-turns".into(),
        source_revision: "exact-root-r1".into(),
        operation,
        outcome: Outcome::ReconcileRequired,
    };
    assert_eq!(
        association.title_marker(),
        format!("[RMB:{}]", association.id)
    );
    ledger
        .associate_export(
            operation,
            expected(&ledger, id),
            ExpectedHeads::default(),
            association.clone(),
        )
        .unwrap();
    assert_eq!(
        ledger.reconcile_export(association.id, &[]).unwrap(),
        ExportReconciliation::Uncertain
    );
    let first = ExportMatch {
        marker: association.id,
        backend: association.backend.clone(),
        scope: association.source_scope.clone(),
        revision: association.source_revision.clone(),
        native_note: "note-1".into(),
    };
    assert_eq!(
        ledger
            .reconcile_export(association.id, std::slice::from_ref(&first))
            .unwrap(),
        ExportReconciliation::Unique("note-1".into())
    );
    let mut second = first.clone();
    second.native_note = "note-2".into();
    assert_eq!(
        ledger
            .reconcile_export(association.id, &[first.clone(), second])
            .unwrap(),
        ExportReconciliation::Conflict(vec!["note-1".into(), "note-2".into()])
    );
    let mut wrong = first;
    wrong.marker = Uuid::new_v4();
    assert!(ledger.reconcile_export(association.id, &[wrong]).is_err());
}
