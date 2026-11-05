use super::*;
#[test]
fn selected_writer_document_memory_shares_writer_admission_without_deadlock() {
    let f = Fixture::new();
    let mut e = Engine::default();
    let first = selected::run(
        &f.writer,
        &f.selection,
        &mut e,
        &f.req,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(first.report.status, Status::Generated);
    let m = f.memory(true);
    let Request::Generate { document, .. } = &f.req else {
        unreachable!()
    };
    m.register_selection(memory::selected::RegisterSelection {
        operation: Uuid::new_v4(),
        registration: memory::catalog::Registration {
            format: 1,
            catalog: f.identity.record_id().unwrap(),
            key_sha256: f.selection.accepted.scope().key_sha256.clone(),
            owner: memory::catalog::Owner::Document {
                document: document.document,
            },
            genesis_operation: f.initial.operation,
            genesis_sha256: f.selection.accepted.selection_sha256().into(),
        },
    })
    .unwrap();
    let fact = memory::Fact {
        key: memory::Key {
            profile: f.identity.profile,
            scope: memory::Scope::Document {
                document: document.document,
            },
            fact: Uuid::new_v4(),
        },
        claim: "study notes document units".into(),
        value: "study notes document reminder: preserve exact units".into(),
        subject: "study notes".into(),
        tags: vec!["study".into()],
        provenance: memory::Provenance {
            basis: memory::Basis::UserConfirmed,
            description: "Synthetic joined document memory".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 1,
        expires_ms: None,
    };
    m.put(memory::Mutation {
        operation: Uuid::new_v4(),
        expected: Default::default(),
        fact,
    })
    .unwrap();
    let (accepted, p) = f.current();
    let draft = first.report.draft.unwrap();
    let source = p
        .inspect(f.req.conversation())
        .unwrap()
        .iter()
        .find_map(|r| {
            if let Record::NormalizedText(s) = r {
                Some(s.id)
            } else {
                None
            }
        })
        .unwrap();
    let selection = Selection {
        accepted,
        root: pin(p
            .head(Namespace::Conversation, f.req.conversation())
            .unwrap()
            .0),
        refine: Some((
            pin(p.head(Namespace::Source, source).unwrap().0),
            pin(p.head(Namespace::Conversation, draft).unwrap().0),
        )),
    };
    let req = Request::Refine {
        conversation: f.req.conversation(),
        operation: Uuid::new_v4(),
        request: "Preserve study notes units.".into(),
        draft,
    };
    let result = selected::run(
        &f.writer,
        &selection,
        &mut e,
        &req,
        &preference(),
        Some(&m),
        &mut || false,
    );
    assert_eq!(result.report.status, Status::Generated);
    assert_eq!(e.calls.len(), 2);
    assert!(e.calls[1].iter().any(|s| s.contains("document reminder")));
}
fn prepare(f: &Fixture) -> SelectedWriterPrepared {
    let Request::Generate {
        document,
        selection,
        request,
        guidance,
        ..
    } = &f.req
    else {
        unreachable!()
    };
    let req = SelectedWriterPrepare {
        operation: f.req.operation(),
        root: f.selection.root.clone(),
        request: request.clone(),
        source: SelectedWriterSource::Generate {
            normalized: NormalizedTextSource::new(
                f.req.conversation(),
                writer::stage(f.req.operation(), "user"),
                document.clone(),
                selection.clone(),
                guidance.clone(),
            )
            .unwrap(),
        },
    };
    let SelectedWriterPreparation::Fresh { prepared, .. } = f
        .writer
        .prepare_selected_writer(&f.selection.accepted, req, 2)
        .unwrap()
    else {
        panic!("fresh preparation expected")
    };
    *prepared
}
fn finish(f: &Fixture, p: SelectedWriterPrepared) -> SelectedWriterFinish {
    SelectedWriterFinish {
        operation: writer::stage(f.req.operation(), "finish"),
        prepared: p,
        assistant: writer::stage(f.req.operation(), "draft"),
        outcome: Outcome::Generated,
        text: Some("# Study\n\nx = 7".into()),
        updated_ms: 3,
    }
}
fn query(f: &Fixture, m: &memory::selected::SelectedMemory) -> memory::QueryResult {
    let Request::Generate { document, .. } = &f.req else {
        unreachable!()
    };
    let mut q = memory::Query::new("study notes".into(), 2);
    q.document = Some(document.document);
    m.query(&q).unwrap()
}
fn replace_head(store: &Store, scope: &SelectionScope) {
    let snapshot = store.selected_snapshot(scope, 4096).unwrap().unwrap();
    let parents: std::collections::BTreeSet<_> = snapshot
        .selected_records
        .iter()
        .flat_map(|e| e.parents.iter().copied())
        .collect();
    let mut head = snapshot
        .selected_records
        .iter()
        .find(|e| !parents.contains(&e.revision_id))
        .unwrap()
        .clone();
    head.parents = [head.revision_id].into();
    head.revision_id = Uuid::new_v4();
    head.operation_id = Uuid::new_v4();
    store
        .commit_selected(&snapshot.token, Uuid::new_v4(), vec![head], BTreeMap::new())
        .unwrap();
}
#[test]
fn selected_writer_exact_references_and_receipt_fingerprint_cannot_be_forged() {
    for case in 0..5 {
        let f = Fixture::new();
        let mut prepared = prepare(&f);
        match case {
            0 => prepared.preparation_fingerprint = "f".repeat(64),
            1 => prepared.source.revision_id = Uuid::new_v4(),
            2 => prepared.user.object.sha256 = "e".repeat(64),
            3 => prepared.root.record_id = Uuid::new_v4(),
            _ => prepared.refined_draft = Some(prepared.user.clone()),
        }
        assert!(f
            .writer
            .finish_selected_writer(finish(&f, prepared))
            .is_err());
        assert_eq!(f.generated(), 0);
    }
    let f = Fixture::new();
    let prepared = prepare(&f);
    let request = finish(&f, prepared);
    assert!(matches!(
        f.writer.finish_selected_writer(request.clone()).unwrap(),
        SelectedWriterPublication::Published { .. }
    ));
    assert!(matches!(
        f.writer.finish_selected_writer(request.clone()).unwrap(),
        SelectedWriterPublication::Historical { .. }
    ));
    let mut changed = request;
    changed.text = Some("# Changed\n\nx = 99".into());
    assert!(f.writer.finish_selected_writer(changed).is_err());
    assert_eq!(f.generated(), 1);
}
#[test]
fn selected_writer_memory_tamper_and_consumed_views_refuse_at_real_commit() {
    for tamper in [false, true] {
        let f = Fixture::new();
        let m = f.memory(false);
        let prepared = prepare(&f);
        let mut view = query(&f, &m);
        let request = finish(&f, prepared);
        if tamper {
            view.context.facts[0].fact.value = "tampered".into();
            assert!(m
                .publish_selected_writer(&view, &f.writer, request)
                .is_err());
        } else {
            f.store.set_fault(Fault::BeforeCommit).unwrap();
            assert!(m
                .publish_selected_writer(&view, &f.writer, request.clone())
                .is_err());
            assert!(m
                .publish_selected_writer(&view, &f.writer, request)
                .is_err());
        }
        assert_eq!(f.generated(), 0);
    }
}
#[test]
fn selected_writer_empty_memory_membership_is_still_guarded() {
    let f = Fixture::new();
    let m = f.memory(true);
    let prepared = prepare(&f);
    let view = query(&f, &m);
    assert_eq!(view.status, memory::Status::CompleteEmpty);
    replace_head(&f.store, &f.identity.scope().unwrap());
    assert!(m
        .publish_selected_writer(&view, &f.writer, finish(&f, prepared))
        .is_err());
    assert_eq!(f.generated(), 0);
}
#[test]
fn selected_writer_unavailable_memory_and_cancellation_publish_only_selected_failures() {
    let f = Fixture::new();
    let m = memory::selected::SelectedMemory::new(f.store.clone(), f.identity.clone()).unwrap();
    let mut e = Engine::default();
    let report = selected::run(
        &f.writer,
        &f.selection,
        &mut e,
        &f.req,
        &preference(),
        Some(&m),
        &mut || false,
    );
    assert_eq!(report.report.status, Status::Failed);
    assert!(report.report.draft.is_none());
    assert!(e.calls.is_empty());
    assert_eq!(f.generated(), 0);
    let f = Fixture::new();
    let flag = Arc::new(AtomicBool::new(false));
    let changed = flag.clone();
    let mut e = Engine {
        hook: Some(Box::new(move || changed.store(true, Ordering::Release))),
        ..Default::default()
    };
    let report = selected::run(
        &f.writer,
        &f.selection,
        &mut e,
        &f.req,
        &preference(),
        None,
        &mut || flag.load(Ordering::Acquire),
    );
    assert_eq!(report.report.status, Status::Canceled);
    assert!(report.report.draft.is_none());
    assert_eq!(e.calls.len(), 1);
    assert_eq!(f.generated(), 0);
}
#[test]
fn selected_writer_explicit_entry_rejects_foreign_selection_and_export() {
    let f = Fixture::new();
    let other = Fixture::new();
    let mut e = Engine::default();
    let report = selected::run(
        &other.writer,
        &f.selection,
        &mut e,
        &f.req,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(report.report.status, Status::Refused);
    assert!(e.calls.is_empty());
    let req = Request::PrepareExport {
        conversation: f.req.conversation(),
        draft: Uuid::new_v4(),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    let report = selected::run(
        &f.writer,
        &f.selection,
        &mut e,
        &req,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(report.report.failure, Some(writer::Failure::Unsupported));
    assert!(e.calls.is_empty());
}
