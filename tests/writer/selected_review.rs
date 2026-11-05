use super::*;

#[test]
fn selected_writer_refines_imported_noncanonical_source_and_replays_after_reopen() {
    let f = Fixture::new();
    let mut engine = Engine::default();
    let first = selected::run(
        &f.writer,
        &f.selection,
        &mut engine,
        &f.req,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(first.report.status, Status::Generated);
    let snapshot = f
        .store
        .selected_snapshot(f.selection.accepted.scope(), 4096)
        .unwrap()
        .unwrap();
    let mut manifest = snapshot.transaction.selected.clone();
    manifest.transaction_id = Uuid::new_v4();
    let mut objects = BTreeMap::new();
    let mut original_source = None;
    for (envelope, reference) in snapshot
        .selected_records
        .iter()
        .zip(&snapshot.transaction.selected.records)
    {
        if matches!(envelope.namespace, Namespace::Source) {
            // Sorted JSON object keys and indentation differ from authored struct encoding.
            let raw = serde_json::to_vec_pretty(&serde_json::to_value(envelope).unwrap()).unwrap();
            assert_ne!(raw, serde_json::to_vec(envelope).unwrap());
            let object = ObjectRef {
                sha256: digest(&raw),
                bytes: raw.len() as u64,
            };
            *manifest
                .records
                .iter_mut()
                .find(|r| *r == reference)
                .unwrap() = object.clone();
            manifest.record_namespaces.remove(&reference.sha256);
            manifest
                .record_namespaces
                .insert(object.sha256.clone(), envelope.namespace);
            original_source = Some(IntentRecordRef {
                namespace: envelope.namespace,
                record_id: envelope.record_id,
                revision_id: envelope.revision_id,
                object: object.clone(),
            });
            objects.insert(object.sha256, raw);
        } else {
            objects.insert(
                reference.sha256.clone(),
                f.store.read_object(reference).unwrap(),
            );
        }
    }
    let local = Local::new();
    let store = local.open();
    let imported = store
        .initialize_selected(
            f.selection.accepted.scope(),
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"imported source fixture"),
                selected: manifest,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    let snapshot = store
        .selected_snapshot(imported.token.scope(), 4096)
        .unwrap()
        .unwrap();
    let projection = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
    let conversation = f.req.conversation();
    let draft = first.report.draft.unwrap();
    let source = original_source.unwrap();
    assert!(projection.exact_reference(&source).is_ok());
    let selection = Selection {
        accepted: imported.token,
        root: pin(projection
            .head(Namespace::Conversation, conversation)
            .unwrap()
            .0),
        refine: Some((
            source.clone(),
            pin(projection.head(Namespace::Conversation, draft).unwrap().0),
        )),
    };
    let request = Request::Refine {
        conversation,
        operation: Uuid::new_v4(),
        request: "Keep the exact imported study notes.".into(),
        draft,
    };
    let writer = SelectedAdmission::new(store.clone(), selection.accepted.scope().clone()).unwrap();
    let result = selected::run(
        &writer,
        &selection,
        &mut engine,
        &request,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(result.report.status, Status::Generated);
    assert_eq!(engine.calls.len(), 2);
    let replay = selected::run(
        &writer,
        &selection,
        &mut engine,
        &request,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(replay.report.status, Status::HistoricalGenerated);
    assert_eq!(engine.calls.len(), 2);
    let original_bytes = store.read_object(&source.object).unwrap();
    drop(writer);
    drop(store);
    let reopened = local.open();
    let writer =
        SelectedAdmission::new(reopened.clone(), selection.accepted.scope().clone()).unwrap();
    let replay = selected::run(
        &writer,
        &selection,
        &mut engine,
        &request,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(replay.report.status, Status::HistoricalGenerated);
    assert_eq!(engine.calls.len(), 2);
    assert_eq!(
        reopened.read_object(&source.object).unwrap(),
        original_bytes
    );
    let snapshot = reopened
        .selected_snapshot(selection.accepted.scope(), 4096)
        .unwrap()
        .unwrap();
    let projection = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
    assert!(projection.exact_reference(&source).is_ok());
}

#[test]
fn selected_writer_bounds_all_sent_history_variants_without_truncation() {
    for excessive_turns in [true, false] {
        let f = Fixture::new();
        let mut engine = Engine::default();
        let first = selected::run(
            &f.writer,
            &f.selection,
            &mut engine,
            &f.req,
            &preference(),
            None,
            &mut || false,
        );
        assert_eq!(first.report.status, Status::Generated);
        let (accepted, p) = f.current();
        let conversation = f.req.conversation();
        let records = p.inspect(conversation).unwrap();
        let source = records
            .iter()
            .find_map(|r| {
                if let Record::NormalizedText(s) = r {
                    Some(s.id)
                } else {
                    None
                }
            })
            .unwrap();
        let mut root = p
            .head(Namespace::Conversation, conversation)
            .unwrap()
            .0
            .clone();
        let start = root.payload["record"]["next_sequence"].as_u64().unwrap();
        let ms = root.payload["record"]["updated_ms"].as_u64().unwrap();
        let count = if excessive_turns { 65 } else { 1 };
        let mut edits = Vec::new();
        let mut preserved = Vec::new();
        for index in 0..count {
            let outcome = match index % 3 {
                0 => Outcome::Generated,
                1 => Outcome::Failed,
                _ => Outcome::Canceled,
            };
            let text = if outcome == Outcome::Generated {
                Some(if excessive_turns {
                    "x".into()
                } else {
                    "x".repeat(16 * 1024 + 1)
                })
            } else {
                None
            };
            let turn = Turn {
                id: Uuid::new_v4(),
                conversation,
                exchange: Uuid::new_v4(),
                sequence: start + index,
                role: Role::Assistant,
                mode: Mode::Writer,
                outcome,
                text,
                sources: vec![source],
                correction_of: None,
                created_ms: ms,
                updated_ms: ms,
                completion: None,
            };
            preserved.push(turn.clone());
            let mut envelope = root.clone();
            envelope.record_id = turn.id;
            envelope.revision_id = Uuid::new_v4();
            envelope.operation_id = Uuid::new_v4();
            envelope.parents.clear();
            envelope.payload = serde_json::to_value(Record::Turn(turn)).unwrap();
            edits.push(envelope);
        }
        root.parents = [root.revision_id].into();
        root.revision_id = Uuid::new_v4();
        root.operation_id = Uuid::new_v4();
        root.payload["record"]["next_sequence"] = json!(start + count);
        edits.push(root);
        let published = f
            .store
            .commit_selected(&accepted, Uuid::new_v4(), edits, BTreeMap::new())
            .unwrap();
        let snapshot = f
            .store
            .selected_snapshot(published.token.scope(), 4096)
            .unwrap()
            .unwrap();
        let p = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
        let selection = Selection {
            accepted: published.token,
            root: pin(p.head(Namespace::Conversation, conversation).unwrap().0),
            refine: None,
        };
        let mut request = f.req.clone();
        let Request::Generate { operation, .. } = &mut request else {
            unreachable!()
        };
        *operation = Uuid::new_v4();
        let result = selected::run(
            &f.writer,
            &selection,
            &mut engine,
            &request,
            &preference(),
            None,
            &mut || false,
        );
        assert_eq!(result.report.status, Status::Failed);
        assert_eq!(result.report.failure, Some(writer::Failure::TooLarge));
        assert!(result.report.draft.is_none());
        assert_eq!(engine.calls.len(), 1);
        let (_, after) = f.current();
        let records = after.inspect(conversation).unwrap();
        for expected in preserved {
            assert!(records
                .iter()
                .any(|r| matches!(r,Record::Turn(actual)if *actual==expected)));
        }
    }
}
