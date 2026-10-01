use remarkable_reader_buddy::{conversation::*, storage::*};
use std::collections::{BTreeMap, BTreeSet};
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
fn replicate(source: &Store, target: &Store) {
    let mut pending = source.manifests().unwrap();
    while !pending.is_empty() {
        let count = pending.len();
        pending.retain(|manifest| {
            let objects = manifest
                .records
                .iter()
                .chain(&manifest.media)
                .map(|object| (object.sha256.clone(), source.read_object(object).unwrap()))
                .collect();
            target.import(manifest.clone(), objects).is_err()
        });
        assert!(
            pending.len() < count,
            "replica dependencies did not resolve"
        );
    }
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
    let dimensions = image::ImageReader::new(std::io::Cursor::new(&bytes[..]))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
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

#[test]
fn replica_conflict_exposes_every_root_and_never_selects_latest() {
    let first_fixture = Fixture::new();
    let second_fixture = Fixture::new();
    let first = Ledger::new(first_fixture.open());
    let second = Ledger::new(second_fixture.open());
    let id = create(&first);
    replicate(first.store(), second.store());
    let old = expected(&first, id);
    first
        .append(
            Uuid::new_v4(),
            old.clone(),
            turn(id, Mode::Reader, "first branch"),
            vec![],
        )
        .unwrap();
    second
        .append(
            Uuid::new_v4(),
            old,
            turn(id, Mode::Writer, "second branch"),
            vec![],
        )
        .unwrap();
    replicate(second.store(), first.store());
    let all = expected(&first, id);
    assert_eq!(all.0.len(), 2);
    assert!(first.inspect(id, false).is_err());
    assert!(first
        .append(
            Uuid::new_v4(),
            all.clone(),
            turn(id, Mode::Reader, "not LWW"),
            vec![]
        )
        .is_err());
    assert!(first.delete(id, Uuid::new_v4(), all.clone(), None).is_err());
    assert_eq!(expected(&first, id), all);
}

#[test]
fn explicit_range_refusals_and_corrections_preserve_full_ledger() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let original = turn(id, Mode::Reader, "original request");
    ledger
        .append(
            Uuid::new_v4(),
            expected(&ledger, id),
            original.clone(),
            vec![],
        )
        .unwrap();
    let mut correction = turn(id, Mode::Writer, "explicit correction");
    correction.correction_of = Some(original.id);
    ledger
        .append(
            Uuid::new_v4(),
            expected(&ledger, id),
            correction.clone(),
            vec![],
        )
        .unwrap();
    let budget = ContextBudget {
        max_text_bytes: 100,
        max_turns: 1,
        provider_token_limit: Some(10),
    };
    let refusal = ledger.context(id, &budget, |_| Ok(1)).err().unwrap();
    assert!(matches!(
        refusal.downcast_ref::<ContextRefusal>(),
        Some(ContextRefusal::SelectionRequired { turns: 2, .. })
    ));
    let range = TurnRange {
        start: 1,
        end_exclusive: 2,
    };
    let selected = ledger
        .context_range(id, Some(range), &budget, |_| Ok(1))
        .unwrap();
    assert_eq!(selected.selection, Some(range));
    assert_eq!(selected.turns[0].id, correction.id);
    assert_eq!(selected.turns[0].correction_of, Some(original.id));
    let unknown = ledger
        .context(
            id,
            &ContextBudget {
                provider_token_limit: None,
                ..budget
            },
            |_| Ok(1),
        )
        .err()
        .unwrap();
    assert_eq!(
        unknown.downcast_ref::<ContextRefusal>(),
        Some(&ContextRefusal::UnknownProviderBudget)
    );
    assert_eq!(
        ledger
            .inspect(id, false)
            .unwrap()
            .iter()
            .filter(|record| matches!(record, Record::Turn(_)))
            .count(),
        2
    );
}

#[test]
fn unknown_relevant_payload_refuses_while_unrelated_legacy_coexists() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let mut opaque = Envelope {
        envelope_version: FORMAT,
        namespace: Namespace::Source,
        domain_schema_version: SCHEMA,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: ledger.store().actor_id,
        kind: Kind::Value,
        payload: serde_json::json!({"legacy":"opaque"}),
        media_descriptors: vec![],
    };
    ledger
        .store()
        .commit(vec![opaque.clone()], BTreeMap::new())
        .unwrap();
    assert!(ledger.inspect(id, false).is_ok());
    opaque.record_id = Uuid::new_v4();
    opaque.revision_id = Uuid::new_v4();
    opaque.operation_id = Uuid::new_v4();
    opaque.payload =
        serde_json::json!({"record_kind":"future-source","record":{"conversation":id}});
    ledger
        .store()
        .commit(vec![opaque], BTreeMap::new())
        .unwrap();
    assert!(ledger.inspect(id, false).is_err());
}

#[test]
fn failed_stage_and_commit_boundaries_preserve_retryable_request_identity() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::AfterCommit,
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let ledger = Ledger::new(store.clone());
        let id = create(&ledger);
        let (request, input) = prepared_input(id);
        let source = input.source.clone();
        let bytes = input.bytes.clone();
        let operation = Uuid::new_v4();
        let head = expected(&ledger, id);
        store.set_fault(fault).unwrap();
        let first = ledger.prepare_request(operation, head.clone(), request.clone(), vec![input]);
        assert_eq!(first.is_ok(), fault == Fault::AfterCommit);
        let second = ledger
            .prepare_request(
                operation,
                head,
                request,
                vec![ImageInput {
                    source,
                    bytes,
                    parent_bytes: None,
                }],
            )
            .unwrap();
        assert_eq!(
            ledger.prepared_images(second.turn).unwrap().images,
            second.images
        );
        assert_eq!(
            ledger
                .inspect(id, false)
                .unwrap()
                .iter()
                .filter(|record| matches!(record, Record::Turn(_)))
                .count(),
            1
        );
    }
}

#[test]
fn later_target_and_parent_crop_keep_both_exact_images() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let (first, input) = prepared_input(id);
    let first_source = input.source.id;
    let first_page = input.source.observation.page;
    let first_bytes = input.bytes.clone();
    ledger
        .prepare_request(Uuid::new_v4(), expected(&ledger, id), first, vec![input])
        .unwrap();
    let (second, mut input) = prepared_input(id);
    let second_source = input.source.id;
    assert_ne!(input.source.observation.page, first_page);
    let parent = input.bytes.clone();
    let parent_dimensions = [input.source.width, input.source.height];
    let crop = image::load_from_memory(&parent)
        .unwrap()
        .crop_imm(0, 0, 2, 2);
    let mut output = std::io::Cursor::new(Vec::new());
    crop.write_to(&mut output, image::ImageFormat::Png).unwrap();
    input.source.parent = Some(input.source.image.clone());
    input.source.parent_dimensions = Some(parent_dimensions);
    input.source.crop = Some([0, 0, 2, 2]);
    input.source.width = 2;
    input.source.height = 2;
    input.source.image.sha256 = digest(output.get_ref());
    input.source.image.bytes = output.get_ref().len() as u64;
    input.parent_bytes = Some(parent);
    input.bytes = output.into_inner();
    let cropped = input.bytes.clone();
    ledger
        .prepare_request(Uuid::new_v4(), expected(&ledger, id), second, vec![input])
        .unwrap();
    assert_eq!(ledger.image(first_source).unwrap(), first_bytes);
    assert_eq!(ledger.image(second_source).unwrap(), cropped);
    assert_eq!(ledger.retained_media(id).unwrap().len(), 2);
}

#[test]
fn competing_initial_pages_commit_exactly_one_claim_without_stranding() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let ledger = Ledger::new(store.clone());
    let id = create(&ledger);
    let head = expected(&ledger, id);
    let source = observation();
    let mut attempts = vec![];
    let mut targets = vec![];
    for _ in 0..2 {
        let operation = Uuid::new_v4();
        let target = Uuid::new_v4();
        targets.push(Ledger::binding_id(source.document, target));
        let receipt = receipt(id, operation, source.clone(), target);
        let (store, head) = (store.clone(), head.clone());
        attempts.push(std::thread::spawn(move || {
            Ledger::new(store)
                .bind(operation, head, ExpectedHeads::default(), receipt)
                .is_ok()
        }));
    }
    assert_eq!(
        attempts
            .into_iter()
            .map(|attempt| attempt.join().unwrap())
            .filter(|success| *success)
            .count(),
        1
    );
    assert_eq!(
        targets
            .into_iter()
            .filter(|target| store
                .value(Namespace::Conversation, *target)
                .unwrap()
                .is_some())
            .count(),
        1
    );
}

#[test]
fn malformed_imported_source_identity_and_singular_geometry_fail_closed() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let (request, mut invalid) = prepared_input(id);
    invalid.source.transform = [0.0; 6];
    assert!(ledger
        .prepare_request(
            Uuid::new_v4(),
            expected(&ledger, id),
            request,
            vec![invalid]
        )
        .is_err());
    let (request, input) = prepared_input(id);
    let source_id = input.source.id;
    ledger
        .prepare_request(Uuid::new_v4(), expected(&ledger, id), request, vec![input])
        .unwrap();
    let mut envelope = ledger
        .store()
        .value(Namespace::Source, source_id)
        .unwrap()
        .unwrap();
    envelope.parents = BTreeSet::from([envelope.revision_id]);
    envelope.revision_id = Uuid::new_v4();
    envelope.operation_id = Uuid::new_v4();
    envelope.payload["record"]["id"] = serde_json::to_value(Uuid::new_v4()).unwrap();
    ledger
        .store()
        .commit(vec![envelope], BTreeMap::new())
        .unwrap();
    assert!(ledger.image(source_id).is_err());
    assert!(ledger.inspect(id, false).is_err());
}

#[test]
fn unrelated_large_history_does_not_consume_selected_conversation_bound() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let foreign = Uuid::new_v4();
    for _ in 0..10 {
        let envelope = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: SCHEMA,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: ledger.store().actor_id,
            kind: Kind::Value,
            payload: serde_json::json!({"record_kind":"foreign-fixture","record":{"conversation":foreign,"bytes":"x".repeat(900_000)}}),
            media_descriptors: vec![],
        };
        ledger
            .store()
            .commit(vec![envelope], BTreeMap::new())
            .unwrap();
    }
    let budget = ContextBudget {
        max_text_bytes: 1,
        max_turns: 1,
        provider_token_limit: Some(1),
    };
    assert!(ledger
        .context(id, &budget, |_| Ok(0))
        .unwrap()
        .turns
        .is_empty());
    assert_eq!(ledger.inspect(id, false).unwrap().len(), 2);
}

#[test]
fn imported_duplicate_or_unallocated_chronology_is_refused() {
    for invalid_sequence in [0, 2] {
        let fixture = Fixture::new();
        let ledger = Ledger::new(fixture.open());
        let id = create(&ledger);
        ledger
            .append(
                Uuid::new_v4(),
                expected(&ledger, id),
                turn(id, Mode::Reader, "first"),
                vec![],
            )
            .unwrap();
        let second = turn(id, Mode::Writer, "second");
        let second_id = second.id;
        ledger
            .append(Uuid::new_v4(), expected(&ledger, id), second, vec![])
            .unwrap();
        let mut envelope = ledger
            .store()
            .value(Namespace::Conversation, second_id)
            .unwrap()
            .unwrap();
        envelope.parents = BTreeSet::from([envelope.revision_id]);
        envelope.revision_id = Uuid::new_v4();
        envelope.operation_id = Uuid::new_v4();
        envelope.payload["record"]["sequence"] = serde_json::json!(invalid_sequence);
        ledger
            .store()
            .commit(vec![envelope], BTreeMap::new())
            .unwrap();
        assert!(ledger.inspect(id, false).is_err());
        assert!(ledger.inspect(id, true).is_err());
    }
}

#[test]
fn retained_media_includes_source_revisions_after_source_tombstone() {
    let fixture = Fixture::new();
    let ledger = Ledger::new(fixture.open());
    let id = create(&ledger);
    let (request, input) = prepared_input(id);
    let source_id = input.source.id;
    let media = input.source.image.clone();
    ledger
        .prepare_request(Uuid::new_v4(), expected(&ledger, id), request, vec![input])
        .unwrap();
    let mut source = ledger
        .store()
        .value(Namespace::Source, source_id)
        .unwrap()
        .unwrap();
    source.parents = BTreeSet::from([source.revision_id]);
    source.revision_id = Uuid::new_v4();
    source.operation_id = Uuid::new_v4();
    source.kind = Kind::Tombstone;
    source.payload = serde_json::Value::Null;
    source.media_descriptors.clear();
    ledger
        .store()
        .commit(vec![source], BTreeMap::new())
        .unwrap();
    assert!(ledger.image(source_id).is_err());
    let retained = ledger.retained_media(id).unwrap();
    let item = retained.iter().find(|item| item.media == media).unwrap();
    assert!(item.available);
    assert_eq!(item.retained_revision_references, 1);
}
