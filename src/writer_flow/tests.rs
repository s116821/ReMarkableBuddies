use super::*;
use crate::{
    config::Personalization,
    conversation::TextScope,
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::{Fault, Store, StorePaths},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct Fixture {
    root: std::path::PathBuf,
    ledger: Ledger,
    conversation: Uuid,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rmb-writer-flow-{}", Uuid::new_v4()));
        let ledger = Ledger::new(Arc::new(
            Store::open(StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            })
            .unwrap(),
        ));
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        Self {
            root,
            ledger,
            conversation,
        }
    }
    fn reopen(self) -> Self {
        let Self {
            root,
            ledger,
            conversation,
        } = self;
        drop(ledger);
        let ledger = Ledger::new(Arc::new(
            Store::open(StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            })
            .unwrap(),
        ));
        Self {
            root,
            ledger,
            conversation,
        }
    }
    fn request(&self) -> Request {
        Request::GenerateAndExport {
            outer_operation: Uuid::new_v4(),
            correlation: Uuid::new_v4(),
            effect_operation: Uuid::new_v4(),
            generation: writer::Request::Generate {
                conversation: self.conversation,
                operation: Uuid::new_v4(),
                request: "Preserve x = 7 and page references.".into(),
                document: NormalizedDocument {
                    document: Uuid::new_v4(),
                    format: SourceFormat::Notebook,
                    source_sha256: "a".repeat(64),
                    extractor_revision: "fixture-v1".into(),
                    units: vec![SourceUnit {
                        page: Some(1),
                        section: None,
                        text: "x = 7".into(),
                    }],
                },
                selection: TextScope::SuppliedNotebook,
                guidance: None,
            },
        }
    }
}
struct Model {
    calls: usize,
    hook: Option<Box<dyn FnOnce() + Send>>,
    response: Option<std::result::Result<String, String>>,
}
impl LLMEngine for Model {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, _: &str) {}
    fn clear_content(&mut self) {}
    fn execute(&mut self) -> Result<String> {
        self.calls += 1;
        if let Some(h) = self.hook.take() {
            h()
        };
        if let Some(response) = self.response.take() {
            return response.map_err(anyhow::Error::msg);
        }
        Ok(if self.calls == 1 {
            "# Study\n\nx = 7 (page 1)."
        } else {
            "# Study\n\nx = 7 (page 1). Refined."
        }
        .into())
    }
}
#[derive(Default)]
struct Remote {
    notes: BTreeMap<String, note_export::RemoteNote>,
    reads: usize,
    discovers: usize,
    writes: usize,
    lose_ack: bool,
    discovery_hook: Option<Box<dyn FnOnce() + Send>>,
}
#[derive(Default)]
struct Destination(Mutex<Remote>);
impl NoteDestination for Destination {
    fn backend(&self) -> &str {
        "fixture"
    }
    fn correlation_location(&self) -> note_export::CorrelationLocation {
        note_export::CorrelationLocation::Title
    }
    async fn discover(
        &self,
        _: Uuid,
        _: note_export::Budget,
    ) -> note_export::Result<note_export::Discovery> {
        let hook = {
            let mut s = self.0.lock().unwrap();
            s.discovers += 1;
            s.discovery_hook.take()
        };
        if let Some(h) = hook {
            h()
        };
        tokio::task::yield_now().await;
        Ok(note_export::Discovery::Missing)
    }
    async fn read(
        &self,
        id: &str,
        _: note_export::Budget,
    ) -> note_export::Result<Option<note_export::RemoteNote>> {
        let mut s = self.0.lock().unwrap();
        s.reads += 1;
        Ok(s.notes.get(id).cloned())
    }
    async fn create(
        &self,
        id: &str,
        content: &str,
        _: Uuid,
        _: note_export::Budget,
    ) -> note_export::Result<note_export::RemoteNote> {
        self.write(id, content, 1)
    }
    async fn update(
        &self,
        id: &str,
        content: &str,
        version: u64,
        _: Uuid,
        _: note_export::Budget,
    ) -> note_export::Result<note_export::RemoteNote> {
        self.write(id, content, version + 1)
    }
}
impl Destination {
    fn write(
        &self,
        id: &str,
        content: &str,
        version: u64,
    ) -> note_export::Result<note_export::RemoteNote> {
        let mut s = self.0.lock().unwrap();
        s.writes += 1;
        let note = note_export::RemoteNote {
            id: id.into(),
            title: content
                .lines()
                .next()
                .unwrap()
                .trim_start_matches("# ")
                .into(),
            content: content.into(),
            version,
            trashed: false,
            metadata: BTreeMap::new(),
        };
        s.notes.insert(id.into(), note.clone());
        if s.lose_ack {
            s.lose_ack = false;
            Err(note_export::Failure::Transport)
        } else {
            Ok(note)
        }
    }
    fn counts(&self) -> (usize, usize, usize) {
        let s = self.0.lock().unwrap();
        (s.discovers, s.reads, s.writes)
    }
}
fn model() -> Model {
    Model {
        calls: 0,
        hook: None,
        response: None,
    }
}
fn pref() -> PersonalizationSnapshot {
    Personalization::default().snapshot().unwrap()
}
fn execute(f: &Fixture, m: &mut Model, d: &Destination, r: &Request) -> Report {
    let (_tx, rx) = watch::channel(false);
    run(&f.ledger, m, d, r, &pref(), rx)
}
#[test]
fn direct_create_reopen_refine_update_and_replay() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let r = f.request();
    let first = execute(&f, &mut m, &d, &r);
    assert_eq!(first.status, Status::Exported);
    assert!(first.observation.write_polled);
    assert_eq!(d.counts(), (1, 1, 1));
    let again = execute(&f, &mut m, &d, &r);
    assert_eq!(again.status, Status::Historical);
    assert_eq!(m.calls, 1);
    assert_eq!(d.counts(), (1, 1, 1));
    let f = f.reopen();
    let (_, correlation, _) = r.identities();
    let refinement = Request::RefineAndExport {
        outer_operation: Uuid::new_v4(),
        correlation,
        effect_operation: Uuid::new_v4(),
        refinement: writer::Request::Refine {
            conversation: f.conversation,
            operation: Uuid::new_v4(),
            request: "Keep the equation.".into(),
            draft: first.draft.unwrap(),
        },
    };
    let (_tx, rx) = watch::channel(false);
    let second = run(&f.ledger, &mut m, &d, &refinement, &pref(), rx);
    assert_eq!(second.status, Status::Exported);
    assert_eq!(m.calls, 2);
    assert_eq!(d.counts(), (1, 2, 2));
    assert_eq!(d.0.lock().unwrap().notes.len(), 1);
}
#[test]
fn producer_finish_lost_ack_never_authorizes_export_even_when_public_generated() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let store = f.ledger.store().clone();
    m.hook = Some(Box::new(move || {
        store.set_fault(Fault::AfterCommit).unwrap()
    }));
    let r = f.request();
    let report = execute(&f, &mut m, &d, &r);
    assert_eq!(report.generation.unwrap().status, writer::Status::Generated);
    assert_eq!(report.status, Status::Interrupted);
    assert_eq!(d.counts(), (0, 0, 0));
    let (_tx, rx) = watch::channel(false);
    let replay = run(&f.reopen().ledger, &mut m, &d, &r, &pref(), rx);
    assert_eq!(replay.status, Status::Historical);
    assert_eq!(m.calls, 1);
    assert_eq!(d.counts(), (0, 0, 0));
}
#[test]
fn outer_and_export_admission_lost_ack_never_dispatch() {
    for phase in [Phase::BeforeAdmission, Phase::BeforeExportAdmission] {
        let f = Fixture::new();
        let d = Destination::default();
        let mut m = model();
        let r = f.request();
        let (_tx, rx) = watch::channel(false);
        let result = run_with_events(&f.ledger, &mut m, &d, &r, &pref(), rx, &mut |p, l| {
            if p == phase {
                l.store().set_fault(Fault::AfterCommit).unwrap()
            }
        });
        assert!(!result.observation.destination_polled);
        assert_eq!(d.counts(), (0, 0, 0));
        assert_eq!(m.calls, usize::from(phase == Phase::BeforeExportAdmission));
        let (_tx, rx) = watch::channel(false);
        let replay = run(&f.reopen().ledger, &mut m, &d, &r, &pref(), rx);
        assert!(matches!(
            replay.status,
            Status::Historical | Status::ReconcileRequired
        ));
        assert_eq!(d.counts(), (0, 0, 0));
    }
}
#[test]
fn cancellation_at_final_write_hook_is_unpolled_and_durably_no_send() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let r = f.request();
    let (tx, rx) = watch::channel(false);
    let report = run_with_events(&f.ledger, &mut m, &d, &r, &pref(), rx, &mut |p, _| {
        if p == Phase::BeforeWritePoll {
            tx.send(true).unwrap()
        }
    });
    assert_eq!(report.status, Status::Canceled);
    assert_eq!(d.counts(), (1, 1, 0));
    assert!(!report.observation.write_polled);
    assert_eq!(report.observation.no_send_recorded, Some(true));
}
#[test]
fn root_deleted_during_discovery_has_zero_writes_but_no_durable_no_send() {
    let f = Fixture::new();
    let d = Destination::default();
    let ledger = Ledger::new(f.ledger.store().clone());
    let id = f.conversation;
    d.0.lock().unwrap().discovery_hook = Some(Box::new(move || {
        let heads = ledger.expected(Namespace::Conversation, id).unwrap();
        ledger.delete(id, Uuid::new_v4(), heads, None).unwrap();
    }));
    let mut m = model();
    let report = execute(&f, &mut m, &d, &f.request());
    assert_eq!(report.status, Status::Refused);
    assert_eq!(d.counts().2, 0);
    assert!(report.observation.selection_changed);
    assert_eq!(report.observation.no_send_recorded, Some(false));
}
#[test]
fn uncertain_write_reconciles_known_id_read_only_with_truthful_observation() {
    let f = Fixture::new();
    let d = Destination::default();
    d.0.lock().unwrap().lose_ack = true;
    let mut m = model();
    let r = f.request();
    let first = execute(&f, &mut m, &d, &r);
    assert_eq!(first.status, Status::ReconcileRequired);
    assert!(first.observation.write_polled);
    let before = d.counts();
    let (_, correlation, _) = r.identities();
    let reconciliation = Request::ReconcileExport {
        conversation: f.conversation,
        correlation,
    };
    let second = execute(&f, &mut m, &d, &reconciliation);
    assert_eq!(second.status, Status::Exported);
    assert!(second.observation.destination_polled);
    assert!(!second.observation.write_polled);
    assert_eq!(d.counts(), (before.0, before.1 + 1, before.2));
    assert_eq!(m.calls, 1);
}

fn revise(ledger: &Ledger, namespace: Namespace, id: Uuid) {
    let mut e = ledger.store().heads(namespace, id).unwrap().pop().unwrap();
    e.parents = std::collections::BTreeSet::from([e.revision_id]);
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    ledger.store().commit(vec![e], BTreeMap::new()).unwrap();
}
#[test]
fn exact_selection_rejects_root_source_and_draft_changes_before_and_after_admission() {
    for phase in [
        Phase::BeforeExportAdmission,
        Phase::AfterExportAdmission,
        Phase::BeforeWritePoll,
    ] {
        for target in ["root", "source", "draft"] {
            let f = Fixture::new();
            let d = Destination::default();
            let mut m = model();
            let r = f.request();
            let (_tx, rx) = watch::channel(false);
            let report = run_with_events(&f.ledger, &mut m, &d, &r, &pref(), rx, &mut |p, l| {
                if p == phase {
                    let records = l.inspect(f.conversation, false).unwrap();
                    let (namespace, id) = match target {
                        "root" => (Namespace::Conversation, f.conversation),
                        "source" => (
                            Namespace::Source,
                            records
                                .iter()
                                .find_map(|r| {
                                    if let crate::conversation::Record::NormalizedText(s) = r {
                                        Some(s.id)
                                    } else {
                                        None
                                    }
                                })
                                .unwrap(),
                        ),
                        _ => (
                            Namespace::Conversation,
                            records
                                .iter()
                                .find_map(|r| {
                                    if let crate::conversation::Record::Turn(t) = r {
                                        if t.role == crate::conversation::Role::Assistant
                                            && t.outcome == Outcome::Generated
                                        {
                                            Some(t.id)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                })
                                .unwrap(),
                        ),
                    };
                    revise(l, namespace, id);
                }
            });
            assert_eq!(report.status, Status::Refused, "{phase:?} {target}");
            assert_eq!(d.counts().2, 0);
            assert!(!report.observation.write_polled);
            if phase != Phase::BeforeExportAdmission {
                assert!(report.observation.selection_changed);
            }
        }
    }
}
#[test]
fn discovery_cancellation_is_observed_before_write() {
    let f = Fixture::new();
    let d = Destination::default();
    let (tx, rx) = watch::channel(false);
    d.0.lock().unwrap().discovery_hook = Some(Box::new(move || {
        tx.send(true).unwrap();
    }));
    let mut m = model();
    let report = run(&f.ledger, &mut m, &d, &f.request(), &pref(), rx);
    assert_eq!(report.status, Status::Canceled);
    assert_eq!(d.counts().2, 0);
    assert_eq!(report.observation.no_send_recorded, Some(true));
}
#[test]
fn joined_owners_exclude_standalone_writer_and_export_for_the_whole_invocation() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let r = f.request();
    let (_tx, rx) = watch::channel(false);
    let mut checked = 0;
    let report = run_with_events(
        &f.ledger,
        &mut m,
        &d,
        &r,
        &pref(),
        rx,
        &mut |phase, ledger| {
            if [
                Phase::BeforeModel,
                Phase::AfterModel,
                Phase::BeforeWritePoll,
            ]
            .contains(&phase)
            {
                let mut other = model();
                let legacy =
                    writer::run(ledger, &mut other, r.model().unwrap(), &pref(), &mut || {
                        false
                    });
                assert_eq!(legacy.status, writer::Status::Busy);
                assert_eq!(other.calls, 0);
                std::thread::scope(|scope| {
                    scope
                        .spawn(|| {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .unwrap();
                            let note = note_export::PreparedNote {
                                conversation: f.conversation,
                                correlation: Uuid::new_v4(),
                                effect_operation: Uuid::new_v4(),
                                source_scope: "fixture".into(),
                                source_revision: "fixture".into(),
                                title: "Other".into(),
                                markdown: "Other".into(),
                            };
                            let (_tx, rx) = watch::channel(false);
                            assert_eq!(
                                runtime
                                    .block_on(note_export::export(ledger, &d, &note, rx))
                                    .status,
                                ExportStatus::Busy
                            );
                            let (_tx, rx) = watch::channel(false);
                            assert_eq!(
                                runtime
                                    .block_on(note_export::reconcile(
                                        ledger,
                                        &d,
                                        f.conversation,
                                        note.correlation,
                                        rx
                                    ))
                                    .status,
                                ExportStatus::Busy
                            );
                        })
                        .join()
                        .unwrap()
                });
                checked += 1;
            }
        },
    );
    assert_eq!(report.status, Status::Exported);
    assert_eq!(checked, 3);
}
#[test]
fn explicit_review_first_export_does_not_generate_again() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let generate = f.request();
    let draft = writer::run(
        &f.ledger,
        &mut m,
        generate.model().unwrap(),
        &pref(),
        &mut || false,
    )
    .draft
    .unwrap();
    let request = Request::ExportDraft {
        outer_operation: Uuid::new_v4(),
        conversation: f.conversation,
        draft,
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    let report = execute(&f, &mut m, &d, &request);
    assert_eq!(report.status, Status::Exported);
    assert!(report.generation.is_none());
    assert_eq!(m.calls, 1);
    assert_eq!(d.counts().2, 1);
}

#[test]
fn changed_content_cannot_reuse_outer_operation() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let mut request = f.request();
    let first = execute(&f, &mut m, &d, &request);
    assert_eq!(first.status, Status::Exported);
    let before = d.counts();
    if let Request::GenerateAndExport {
        generation: writer::Request::Generate { request, .. },
        ..
    } = &mut request
    {
        request.push_str(" Changed intent.");
    }
    let replay = execute(&f, &mut m, &d, &request);
    assert_eq!(replay.status, Status::Refused);
    assert_eq!(m.calls, 1);
    assert_eq!(d.counts(), before);
}
#[test]
fn failed_invalid_oversized_and_canceled_model_results_never_reach_destination() {
    for response in [
        Err("synthetic failure".into()),
        Ok("invalid without heading".into()),
        Ok(format!("# Too large\n\n{}", "x".repeat(25 * 1024))),
    ] {
        let f = Fixture::new();
        let d = Destination::default();
        let mut m = model();
        m.response = Some(response);
        let report = execute(&f, &mut m, &d, &f.request());
        assert_eq!(report.status, Status::GenerationFailed);
        assert_eq!(d.counts(), (0, 0, 0));
    }
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let (tx, rx) = watch::channel(false);
    m.hook = Some(Box::new(move || {
        tx.send(true).unwrap();
    }));
    let report = run(&f.ledger, &mut m, &d, &f.request(), &pref(), rx);
    assert_eq!(report.status, Status::Canceled);
    assert_eq!(d.counts(), (0, 0, 0));
}
#[test]
fn selected_source_changed_while_discovery_is_pending_never_writes() {
    let f = Fixture::new();
    let d = Destination::default();
    let ledger = Ledger::new(f.ledger.store().clone());
    let id = f.conversation;
    d.0.lock().unwrap().discovery_hook = Some(Box::new(move || {
        let source = ledger
            .inspect(id, false)
            .unwrap()
            .into_iter()
            .find_map(|r| {
                if let crate::conversation::Record::NormalizedText(s) = r {
                    Some(s.id)
                } else {
                    None
                }
            })
            .unwrap();
        revise(&ledger, Namespace::Source, source);
    }));
    let mut m = model();
    let report = execute(&f, &mut m, &d, &f.request());
    assert_eq!(report.status, Status::Refused);
    assert!(report.observation.selection_changed);
    assert_eq!(d.counts().2, 0);
    assert_eq!(report.observation.no_send_recorded, Some(true));
}

#[test]
fn interruption_after_saved_generation_requires_new_explicit_export_intent() {
    let f = Fixture::new();
    let d = Destination::default();
    let mut m = model();
    let request = f.request();
    let (tx, rx) = watch::channel(false);
    let stopped = run_with_events(&f.ledger, &mut m, &d, &request, &pref(), rx, &mut |p, _| {
        if p == Phase::AfterModel {
            tx.send(true).unwrap()
        }
    });
    assert_eq!(stopped.status, Status::Canceled);
    assert!(stopped.draft.is_some());
    assert_eq!(d.counts(), (0, 0, 0));
    let replay = execute(&f, &mut m, &d, &request);
    assert_eq!(replay.status, Status::Historical);
    assert_eq!(d.counts(), (0, 0, 0));
    let (_, correlation, _) = request.identities();
    let explicit = Request::ExportDraft {
        outer_operation: Uuid::new_v4(),
        conversation: f.conversation,
        draft: stopped.draft.unwrap(),
        correlation,
        effect_operation: Uuid::new_v4(),
    };
    let exported = execute(&f, &mut m, &d, &explicit);
    assert_eq!(exported.status, Status::Exported);
    assert_eq!(m.calls, 1);
    assert_eq!(d.counts().2, 1);
}
