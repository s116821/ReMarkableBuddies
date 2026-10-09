use super::*;
use crate::storage::selection::{SelectionChange, SelectionScope};
use crate::storage::{Coverage, Manifest, Scope, StorePaths};
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("selected-context-{}", Uuid::new_v4())))
    }
    fn store(&self) -> Arc<Store> {
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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn budget() -> ContextBudget {
    ContextBudget {
        max_text_bytes: 1024,
        max_turns: 10,
        provider_token_limit: Some(100),
    }
}
fn scope() -> SelectionScope {
    SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: digest(b"document"),
        binding_sha256: digest(b"binding"),
    }
}
fn envelope(store: &Store, record: Record) -> Envelope {
    let (id, namespace) = match &record {
        Record::Root(r) => (r.id, Namespace::Conversation),
        Record::Turn(t) => (t.id, Namespace::Conversation),
        Record::LegacyCapture(c) => (c.id, Namespace::Source),
        _ => panic!("fixture variant"),
    };
    Envelope {
        envelope_version: FORMAT,
        namespace,
        domain_schema_version: SCHEMA,
        record_id: id,
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(record).unwrap(),
        media_descriptors: vec![],
    }
}
fn records(store: &Store, texts: &[&str]) -> Vec<Envelope> {
    let conversation = Uuid::new_v4();
    let mut result = vec![envelope(
        store,
        Record::Root(Root {
            id: conversation,
            next_sequence: texts.len() as u64,
            binding: None,
            created_ms: 1,
            updated_ms: 1,
        }),
    )];
    for (sequence, text) in texts.iter().enumerate() {
        result.push(envelope(
            store,
            Record::Turn(Turn {
                id: Uuid::new_v4(),
                conversation,
                exchange: Uuid::new_v4(),
                sequence: sequence as u64,
                role: Role::User,
                mode: Mode::Reader,
                outcome: Outcome::Interpreted,
                text: Some((*text).into()),
                sources: vec![],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            }),
        ));
    }
    result
}
fn manifest(records: &[Envelope]) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut objects = BTreeMap::new();
    let mut refs = vec![];
    let mut namespaces = BTreeMap::new();
    for record in records {
        let bytes = serde_json::to_vec(record).unwrap();
        let hash = digest(&bytes);
        refs.push(ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(hash.clone(), record.namespace);
        objects.insert(hash, bytes);
    }
    (
        Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: refs,
            record_namespaces: namespaces,
            media: vec![],
            media_coverage: vec![],
        },
        objects,
    )
}
fn change(records: &[Envelope]) -> (SelectionChange, BTreeMap<String, Vec<u8>>) {
    let (selected, objects) = manifest(records);
    (
        SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"base"),
            selected,
            retained: vec![],
        },
        objects,
    )
}
fn edited(source: &Envelope, text: &str) -> Envelope {
    let mut result = source.clone();
    result.revision_id = Uuid::new_v4();
    result.operation_id = Uuid::new_v4();
    result.parents = BTreeSet::from([source.revision_id]);
    result.payload["record"]["text"] = serde_json::json!(text);
    result
}

#[test]
fn selected_context_reads_winner_not_retained_or_all_history_and_releases_callback_gate() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let admission = SelectedAdmission::new(store.clone(), s.clone()).unwrap();
    let winner = records(&store, &["winner"]);
    let conversation = winner[0].record_id;
    let mut loser = winner[1].clone();
    loser.revision_id = Uuid::new_v4();
    loser.operation_id = Uuid::new_v4();
    loser.payload["record"]["text"] = serde_json::json!("loser");
    store
        .commit(vec![winner[0].clone(), loser.clone()], BTreeMap::new())
        .unwrap();
    let (mut update, mut objects) = change(&winner);
    let (retained, more) = manifest(&[winner[0].clone(), loser]);
    update.retained.push(retained);
    objects.extend(more);
    let initial = admission.initialize(update, objects).unwrap();
    let ledger = Ledger::new(store.clone());
    assert!(ledger.context(conversation, &budget(), |_| Ok(1)).is_err());
    let next = vec![
        winner[0].clone(),
        winner[1].clone(),
        edited(&winner[1], "replacement"),
    ];
    let (update, objects) = change(&next);
    let other = SelectedAdmission::new(store.clone(), s.clone()).unwrap();
    let returned = std::cell::RefCell::new(None);
    let view = admission
        .context_range(&initial.token, conversation, None, &budget(), |turns| {
            assert_eq!(turns[0].text.as_deref(), Some("winner"));
            // Reentrant replacement proves callback runs outside the shared gate.
            *returned.borrow_mut() = Some(
                other
                    .activate(&initial.token, update.clone(), objects.clone())
                    .unwrap(),
            );
            Ok(1)
        })
        .unwrap();
    assert_eq!(view.turns.len(), 1);
    assert_eq!(view.turns[0].text.as_deref(), Some("winner"));
    assert!(admission
        .context_range(&initial.token, conversation, None, &budget(), |_| Ok(1))
        .is_err());
    let replacement = returned.borrow();
    let current = admission
        .context_range(
            &replacement.as_ref().unwrap().token,
            conversation,
            None,
            &budget(),
            |_| Ok(1),
        )
        .unwrap();
    assert_eq!(current.turns[0].text.as_deref(), Some("replacement"));
}
#[test]
fn selected_context_budget_range_and_complete_chronology_refuse_without_truncation() {
    let f = Fixture::new();
    let store = f.store();
    let s = scope();
    let admission = SelectedAdmission::new(store.clone(), s).unwrap();
    let values = records(&store, &["first", "second"]);
    let id = values[0].record_id;
    let (update, objects) = change(&values);
    let selected = admission.initialize(update, objects).unwrap();
    let mut limited = budget();
    limited.max_turns = 1;
    let error = admission
        .context_range(&selected.token, id, None, &limited, |_| {
            panic!("over-budget count must not be called")
        })
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ContextRefusal>(),
        Some(ContextRefusal::SelectionRequired { turns: 2, .. })
    ));
    limited.provider_token_limit = None;
    let error = admission
        .context_range(&selected.token, id, None, &limited, |_| {
            panic!("unknown budget")
        })
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ContextRefusal>(),
        Some(&ContextRefusal::UnknownProviderBudget)
    );
    let chosen = admission
        .context_range(
            &selected.token,
            id,
            Some(TurnRange {
                start: 1,
                end_exclusive: 2,
            }),
            &budget(),
            |_| Ok(1),
        )
        .unwrap();
    assert_eq!(chosen.turns[0].text.as_deref(), Some("second"));
    let all = admission
        .context_range(&selected.token, id, None, &budget(), |_| Ok(2))
        .unwrap();
    assert_eq!(all.turns.len(), 2);
    let (update, objects) = change(&[values[0].clone(), values[2].clone()]);
    let incomplete = admission
        .activate(&selected.token, update, objects)
        .unwrap();
    let error = admission
        .context_range(&incomplete.token, id, None, &budget(), |_| {
            panic!("incomplete chronology")
        })
        .unwrap_err();
    assert!(error.to_string().contains("chronology is incomplete"));
}
#[test]
fn selected_context_reports_omitted_winner_media_even_if_retained_bytes_exist() {
    let f = Fixture::new();
    let store = f.store();
    let admission = SelectedAdmission::new(store.clone(), scope()).unwrap();
    let mut values = records(&store, &["question"]);
    let conversation = values[0].record_id;
    let bytes = b"opaque image fixture".to_vec();
    let media = Media {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    };
    let capture_id = Uuid::new_v4();
    values[1].payload["record"]["sources"] = serde_json::json!([capture_id]);
    let mut capture = envelope(
        &store,
        Record::LegacyCapture(LegacyCapture {
            schema: 1,
            id: capture_id,
            conversation,
            turn: values[1].record_id,
            origin: LegacyOrigin::LegacyUnqualified,
            identity: (),
            qualification: (),
            parent: None,
            images: vec![LegacyImage {
                media: media.clone(),
                dimensions: [768, 1024],
                role: LegacyImageRole::Overview,
                provider_ordinal: Some(0),
            }],
        }),
    );
    capture.media_descriptors = vec![media.clone()];
    values.push(capture);
    let (mut update, mut objects) = change(&values);
    update.selected.media_coverage.push(Coverage::Omitted {
        sha256: media.sha256.clone(),
        reason: crate::storage::Omission::PolicyDisabled,
    });
    let mut historical = values[1].clone();
    historical.revision_id = Uuid::new_v4();
    historical.operation_id = Uuid::new_v4();
    historical.payload["record"]["text"] = serde_json::json!("retained");
    let (mut retained, more) = manifest(&[values[0].clone(), historical, values[2].clone()]);
    retained.media = vec![ObjectRef {
        sha256: media.sha256.clone(),
        bytes: media.bytes,
    }];
    retained.media_coverage = vec![Coverage::Included {
        sha256: media.sha256.clone(),
    }];
    update.retained.push(retained);
    objects.extend(more);
    objects.insert(media.sha256.clone(), bytes);
    let selected = admission.initialize(update, objects).unwrap();
    let view = admission
        .context_range(&selected.token, conversation, None, &budget(), |_| Ok(1))
        .unwrap();
    assert_eq!(view.missing_media, vec![media]);
    assert_eq!(view.legacy_captures.len(), 1);
    assert_eq!(view.turns[0].text.as_deref(), Some("question"));
}
