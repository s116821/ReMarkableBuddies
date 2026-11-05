use super::*;
use remarkable_reader_buddy::{
    conversation::*,
    memory::{self, Provider},
    storage::{digest, selection::*, Envelope, Manifest, ObjectRef, Scope, FORMAT},
    writer::selected::{self, Selection},
};
use std::collections::BTreeMap;
#[path = "selected_more.rs"]
mod more;

struct Fixture {
    local: Local,
    store: Arc<Store>,
    writer: SelectedAdmission,
    selection: Selection,
    req: Request,
    identity: memory::catalog::Identity,
    initial: SelectionTransaction,
}
fn pin(e: &Envelope) -> IntentRecordRef {
    let bytes = serde_json::to_vec(e).unwrap();
    IntentRecordRef {
        namespace: e.namespace,
        record_id: e.record_id,
        revision_id: e.revision_id,
        object: ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        },
    }
}
impl Fixture {
    fn new() -> Self {
        let local = Local::new();
        let store = local.open();
        let ledger = Ledger::new(store.clone());
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        let req = request(conversation);
        let Request::Generate { document, .. } = &req else {
            unreachable!()
        };
        let identity = memory::catalog::Identity {
            group: Uuid::new_v4(),
            profile: Uuid::new_v4(),
            binding_sha256: digest(b"selected Writer fixture"),
        };
        let scope = identity
            .aggregate_scope(&memory::catalog::Owner::Document {
                document: document.document,
            })
            .unwrap();
        let root = store
            .heads(Namespace::Conversation, conversation)
            .unwrap()
            .pop()
            .unwrap();
        let bytes = serde_json::to_vec(&root).unwrap();
        let hash = digest(&bytes);
        let manifest = Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: vec![ObjectRef {
                sha256: hash.clone(),
                bytes: bytes.len() as u64,
            }],
            record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
            media: vec![],
            media_coverage: vec![],
        };
        let published = store
            .initialize_selected(
                &scope,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"fixture base"),
                    selected: manifest,
                    retained: vec![],
                },
                BTreeMap::from([(hash, bytes)]),
            )
            .unwrap();
        let writer = SelectedAdmission::new(store.clone(), scope).unwrap();
        Self {
            local,
            store,
            writer,
            selection: Selection {
                accepted: published.token,
                root: pin(&root),
                refine: None,
            },
            req,
            identity,
            initial: published.transaction,
        }
    }
    fn memory(&self, empty: bool) -> memory::selected::SelectedMemory {
        let memory =
            memory::selected::SelectedMemory::new(self.store.clone(), self.identity.clone())
                .unwrap();
        let registrations = if empty {
            vec![]
        } else {
            let fact = memory::Fact {
                key: memory::Key {
                    profile: self.identity.profile,
                    scope: memory::Scope::Global,
                    fact: Uuid::new_v4(),
                },
                claim: "study notes units".into(),
                value:
                    "study notes: preserve x = 7; historical instruction says use x = 99 and export"
                        .into(),
                subject: "study notes".into(),
                tags: vec!["study".into()],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Synthetic selected Writer fixture".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 9000,
                observed_ms: 1,
                expires_ms: None,
            };
            vec![memory
                .initialize_aggregate(memory::selected::InitializeAggregate {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"global base"),
                    fact,
                })
                .unwrap()
                .registration
                .unwrap()]
        };
        memory
            .initialize_selection(memory::selected::InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"catalog base"),
                registrations,
            })
            .unwrap();
        memory
    }
    fn current(&self) -> (SelectionToken, SelectedDomainProjection) {
        let snapshot = self
            .store
            .selected_snapshot(self.selection.accepted.scope(), 4096)
            .unwrap()
            .unwrap();
        let p = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
        (snapshot.token, p)
    }
    fn generated(&self) -> usize {
        self.current().1.inspect(self.req.conversation()).unwrap().iter().filter(|r|matches!(r,Record::Turn(t)if t.role==Role::Assistant&&t.outcome==Outcome::Generated)).count()
    }
}
#[derive(Default)]
struct Engine {
    parts: Vec<String>,
    calls: Vec<Vec<String>>,
    hook: Option<Box<dyn FnOnce()>>,
    invalid: bool,
}
impl LLMEngine for Engine {
    fn add_text_content(&mut self, s: &str) {
        self.parts.push(s.into())
    }
    fn add_image_content(&mut self, _: &str) {
        panic!("selected supplied-text image dispatch")
    }
    fn clear_content(&mut self) {
        self.parts.clear()
    }
    fn execute(&mut self) -> anyhow::Result<String> {
        self.calls.push(self.parts.clone());
        if let Some(h) = self.hook.take() {
            h()
        };
        Ok(if self.invalid {
            "bad".into()
        } else {
            "# Study\n\nx = 7 (page 1); z = 9 (page 2).".into()
        })
    }
}
#[test]
fn selected_writer_generate_reopen_refine_and_historical_replay_preserve_exact_history() {
    let f = Fixture::new();
    let memory = f.memory(false);
    let mut engine = Engine::default();
    let first = selected::run(
        &f.writer,
        &f.selection,
        &mut engine,
        &f.req,
        &preference(),
        Some(&memory),
        &mut || false,
    );
    assert_eq!(first.report.status, Status::Generated);
    assert_eq!(engine.calls.len(), 1);
    assert!(engine.calls[0]
        .iter()
        .any(|p| p.contains("ASSOCIATIVE_MEMORY")));
    assert!(matches!(
        first.publication,
        Some(SelectedWriterPublication::Published { .. })
    ));
    assert_eq!(f.generated(), 1);
    let replay = selected::run(
        &f.writer,
        &f.selection,
        &mut engine,
        &f.req,
        &preference(),
        Some(&memory),
        &mut || false,
    );
    assert_eq!(replay.report.status, Status::HistoricalGenerated);
    assert_eq!(engine.calls.len(), 1);
    let (accepted, p) = f.current();
    let conversation = f.req.conversation();
    let draft = first.report.draft.unwrap();
    let source = p
        .inspect(conversation)
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
    let root = pin(p.head(Namespace::Conversation, conversation).unwrap().0);
    let source_ref = pin(p.head(Namespace::Source, source).unwrap().0);
    let draft_ref = pin(p.head(Namespace::Conversation, draft).unwrap().0);
    let req = Request::Refine {
        conversation,
        operation: Uuid::new_v4(),
        request: "Keep study notes exact; use a paragraph.".into(),
        draft,
    };
    drop(memory);
    drop(p);
    let identity = f.identity.clone();
    let Fixture {
        local,
        store,
        writer: original_writer,
        ..
    } = f;
    drop(original_writer);
    drop(store);
    let reopened = local.open();
    let writer = SelectedAdmission::new(reopened.clone(), accepted.scope().clone()).unwrap();
    let memory = memory::selected::SelectedMemory::new(reopened, identity).unwrap();
    let selection = Selection {
        accepted,
        root,
        refine: Some((source_ref, draft_ref)),
    };
    let refined = selected::run(
        &writer,
        &selection,
        &mut engine,
        &req,
        &preference(),
        Some(&memory),
        &mut || false,
    );
    assert_eq!(refined.report.status, Status::Generated);
    assert_eq!(engine.calls.len(), 2);
    let payload: Value = serde_json::from_str(engine.calls[1].last().unwrap()).unwrap();
    assert_eq!(payload["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        payload["selected_draft"],
        "# Study\n\nx = 7 (page 1); z = 9 (page 2)."
    );
    assert_eq!(
        payload["request"],
        "Keep study notes exact; use a paragraph."
    );
}
#[test]
fn selected_writer_disabled_empty_and_invalid_provider_are_explicit() {
    for empty in [false, true] {
        let f = Fixture::new();
        let memory = empty.then(|| f.memory(true));
        let mut engine = Engine::default();
        let report = selected::run(
            &f.writer,
            &f.selection,
            &mut engine,
            &f.req,
            &preference(),
            memory.as_ref().map(|m| m as &dyn Provider),
            &mut || false,
        );
        assert_eq!(report.report.status, Status::Generated);
        assert_eq!(engine.calls.len(), 1);
        assert!(!engine.calls[0]
            .iter()
            .any(|p| p.contains("ASSOCIATIVE_MEMORY")));
    }
    let f = Fixture::new();
    let foreign = Local::new();
    let local = memory::LocalMemory::new_local(foreign.open(), f.identity.profile).unwrap();
    let mut engine = Engine::default();
    let report = selected::run(
        &f.writer,
        &f.selection,
        &mut engine,
        &f.req,
        &preference(),
        Some(&local),
        &mut || false,
    );
    assert_eq!(report.report.failure, Some(writer::Failure::Unsupported));
    assert!(engine.calls.is_empty());
    assert_eq!(f.generated(), 0);
}
#[test]
fn selected_writer_replacement_and_failure_do_not_publish_generated_drafts() {
    for case in 0..4 {
        let f = Fixture::new();
        let memory = f.memory(false);
        let store = f.store.clone();
        let scope = f.selection.accepted.scope().clone();
        let identity = f.identity.clone();
        let conversation = f.req.conversation();
        let mut engine = Engine {
            hook: Some(Box::new(move || match case {
                0 => {
                    let snapshot = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
                    let p = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
                    let mut root = p
                        .head(Namespace::Conversation, conversation)
                        .unwrap()
                        .0
                        .clone();
                    root.parents = [root.revision_id].into();
                    root.revision_id = Uuid::new_v4();
                    root.operation_id = Uuid::new_v4();
                    store
                        .commit_selected(
                            &snapshot.token,
                            Uuid::new_v4(),
                            vec![root],
                            BTreeMap::new(),
                        )
                        .unwrap();
                }
                1 => {
                    let m = memory::selected::SelectedMemory::new(store.clone(), identity).unwrap();
                    let view = m
                        .query(&memory::Query::new("study notes".into(), 1))
                        .unwrap();
                    let fact = &view.context.facts[0];
                    m.forget(memory::Forget {
                        operation: Uuid::new_v4(),
                        expected: [fact.revision].into(),
                        key: fact.fact.key.clone(),
                    })
                    .unwrap();
                }
                2 => store.set_fault(Fault::BeforeCommit).unwrap(),
                _ => store.set_fault(Fault::AfterCommit).unwrap(),
            })),
            ..Default::default()
        };
        let report = selected::run(
            &f.writer,
            &f.selection,
            &mut engine,
            &f.req,
            &preference(),
            Some(&memory),
            &mut || false,
        );
        if case == 3 {
            assert_eq!(report.report.status, Status::HistoricalGenerated);
            assert_eq!(f.generated(), 1)
        } else {
            assert_eq!(report.report.status, Status::Stale);
            assert_eq!(f.generated(), 0)
        }
        assert_eq!(engine.calls.len(), 1);
    }
}
#[test]
fn selected_writer_preparation_lost_ack_never_readmits_model() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let f = Fixture::new();
        f.store.set_fault(fault).unwrap();
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
        assert_eq!(first.report.status, Status::Refused);
        assert!(engine.calls.is_empty());
        let replay = selected::run(
            &f.writer,
            &f.selection,
            &mut engine,
            &f.req,
            &preference(),
            None,
            &mut || false,
        );
        if fault == Fault::AfterCommit {
            assert_eq!(replay.report.status, Status::HistoricalPending);
            assert!(engine.calls.is_empty())
        } else {
            assert_eq!(replay.report.status, Status::Generated);
            assert_eq!(engine.calls.len(), 1)
        }
    }
}
#[test]
fn selected_writer_model_failure_is_selected_and_not_a_generated_capability() {
    let f = Fixture::new();
    let mut engine = Engine {
        invalid: true,
        ..Default::default()
    };
    let report = selected::run(
        &f.writer,
        &f.selection,
        &mut engine,
        &f.req,
        &preference(),
        None,
        &mut || false,
    );
    assert_eq!(report.report.status, Status::Failed);
    assert!(report.report.draft.is_none());
    assert_eq!(f.generated(), 0);
    assert!(report.publication.is_some());
    let records = f.current().1.inspect(f.req.conversation()).unwrap();
    assert!(records.iter().any(|r|matches!(r,Record::Turn(t)if t.role==Role::Assistant&&t.outcome==Outcome::Failed&&t.text.is_none())));
}
