//! Synthetic device/provider checks through the real Reader; no recognition or native claim.
use super::*;
use crate::{
    conversation::{Ledger, Record, Role},
    memory::{self, Provider},
    storage::{Namespace, Uuid},
};
use std::sync::Arc;

#[derive(Default)]
struct Request {
    text: Vec<String>,
    images: Vec<String>,
}
struct Model {
    current: Request,
    calls: Rc<RefCell<Vec<Request>>>,
    replies: VecDeque<String>,
    cancel: Option<Shared>,
}

struct ReplacingModel {
    inner: Model,
    replace: Option<Box<dyn FnOnce()>>,
}
impl LLMEngine for ReplacingModel {
    fn add_text_content(&mut self, text: &str) {
        self.inner.add_text_content(text);
    }
    fn add_image_content(&mut self, image: &str) {
        self.inner.add_image_content(image);
    }
    fn clear_content(&mut self) {
        self.inner.clear_content();
    }
    fn execute(&mut self) -> Result<String> {
        if self.inner.calls.borrow().len() == 2 {
            if let Some(replace) = self.replace.take() {
                replace();
            }
        }
        self.inner.execute()
    }
}

struct BoundaryReplacement {
    memory: Arc<memory::selected::SelectedMemory>,
    replacement: std::sync::Mutex<Option<memory::Mutation>>,
    forge_query: bool,
}
impl Provider for BoundaryReplacement {
    fn put(&self, value: memory::Mutation) -> Result<Uuid> {
        self.memory.put(value)
    }
    fn forget(&self, value: memory::Forget) -> Result<Uuid> {
        self.memory.forget(value)
    }
    fn inspect(&self, key: &memory::Key) -> Result<Vec<crate::storage::Envelope>> {
        self.memory.inspect(key)
    }
    fn query(&self, query: &memory::Query) -> Result<memory::QueryResult> {
        let mut view = self.memory.query(query)?;
        if self.forge_query {
            view.context.facts[0].fact.value = "forged selected answer context".into();
        }
        Ok(view)
    }
    fn with_current(
        &self,
        view: &memory::QueryResult,
        consume: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        if let Some(replacement) = self.replacement.lock().unwrap().take() {
            self.memory.put(replacement)?;
        }
        self.memory.with_current(view, consume)
    }
}

#[test]
fn selected_reader_revalidates_after_model_and_at_actual_journal_boundary_without_native_output() {
    for mode in 0..4 {
        let (state, workflow) = setup();
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let profile = Uuid::new_v4();
        let identity = memory::catalog::Identity {
            group: Uuid::new_v4(),
            profile,
            binding_sha256: crate::storage::digest(
                b"synthetic local binding, not native/cloud authority",
            ),
        };
        let memory =
            Arc::new(memory::selected::SelectedMemory::new(store.clone(), identity).unwrap());
        let fact = memory::Fact {
            key: memory::Key {
                profile,
                scope: memory::Scope::Global,
                fact: Uuid::new_v4(),
            },
            claim: "gravity convention".into(),
            value: "G unc. uses SI gravity units".into(),
            subject: "gravity".into(),
            tags: vec!["unc".into()],
            provenance: memory::Provenance {
                basis: memory::Basis::UserConfirmed,
                description: "Explicit host fixture".into(),
                conversation: None,
                turn: None,
                source: None,
            },
            confidence_basis_points: 9000,
            observed_ms: 1,
            expires_ms: None,
        };
        let created = memory
            .initialize_aggregate(memory::selected::InitializeAggregate {
                operation: Uuid::new_v4(),
                accepted_base_sha256: crate::storage::digest(b"aggregate"),
                fact: fact.clone(),
            })
            .unwrap();
        memory
            .initialize_selection(memory::selected::InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: crate::storage::digest(b"catalog"),
                registrations: vec![created.registration.unwrap()],
            })
            .unwrap();
        let expected = memory
            .inspect(&fact.key)
            .unwrap()
            .into_iter()
            .map(|e| e.revision_id)
            .collect();
        let mut changed = fact.clone();
        changed.value = "G unc. uses a corrected convention".into();
        let mutation = memory::Mutation {
            operation: Uuid::new_v4(),
            expected,
            fact: changed,
        };
        let calls = Rc::new(RefCell::new(vec![]));
        let inner = Model {current:Request::default(),calls:calls.clone(),replies:VecDeque::from([
            "QUESTION: G unc.?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\n---\nANSWER: Original proposal.".into(),
            "TRANSCRIPTION: G unc.?".into(),"ANSWER: Enriched gravity answer.".into(),
        ]),cancel:None};
        let replace_memory = memory.clone();
        let replacement = mutation.clone();
        let model = ReplacingModel {
            inner,
            replace: (mode == 0).then(|| {
                Box::new(move || {
                    replace_memory.put(replacement).unwrap();
                }) as Box<dyn FnOnce()>
            }),
        };
        let provider: Arc<dyn Provider> = if mode == 1 || mode == 3 {
            Arc::new(BoundaryReplacement {
                memory: memory.clone(),
                replacement: std::sync::Mutex::new((mode == 1).then_some(mutation)),
                forge_query: mode == 3,
            })
        } else {
            memory.clone()
        };
        let mut reader = Orchestrator::new(workflow, model, ledger);
        reader.set_trigger_enabled(false);
        reader.set_subject_memory(Some(provider));
        let result = reader.run_iteration();
        assert_eq!(result.is_ok(), mode == 2);
        assert_eq!(calls.borrow().len(), if mode == 3 { 2 } else { 3 });
        if mode == 3 {
            assert!(format!("{:#}", result.unwrap_err())
                .contains("selected memory result payload changed"));
        }
        let envelopes = store
            .snapshot_heads_matching(&[Namespace::Conversation], 4096, |_| true)
            .unwrap();
        let turns: Vec<_> = envelopes
            .iter()
            .filter_map(|e| serde_json::from_value::<Record>(e.payload.clone()).ok())
            .filter_map(|r| {
                if let Record::Turn(t) = r {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert!(turns
            .iter()
            .any(|t| t.role == Role::User && t.text.as_deref() == Some("G unc.?")));
        if mode != 2 {
            assert!(turns.iter().all(|t| t.role != Role::Assistant));
            assert!(
                turns
                    .iter()
                    .any(|t| t.role == Role::User
                        && t.outcome == crate::conversation::Outcome::Failed)
            );
        } else {
            assert!(turns
                .iter()
                .any(|t| t.role == Role::Assistant
                    && t.outcome == crate::conversation::Outcome::Failed));
        }
        let state = state.borrow();
        assert!(state.pages.iter().all(|page| page.text.is_empty()));
        assert!(!state
            .events
            .iter()
            .any(|event| matches!(event.action.as_str(), "text" | "next")));
    }
}
impl LLMEngine for Model {
    fn add_text_content(&mut self, text: &str) {
        self.current.text.push(text.into());
    }
    fn add_image_content(&mut self, image: &str) {
        self.current.images.push(image.into());
    }
    fn clear_content(&mut self) {
        self.current = Request::default();
    }
    fn execute(&mut self) -> Result<String> {
        self.calls
            .borrow_mut()
            .push(std::mem::take(&mut self.current));
        if self.calls.borrow().len() == 3 {
            if let Some(state) = &self.cancel {
                state.borrow_mut().visit += 1;
            }
        }
        self.replies
            .pop_front()
            .context("synthetic provider failure")
    }
}
fn setup() -> (Shared, Workflow) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    let scenario: Scenario =
        serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap()).unwrap();
    let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
    let workflow = Workflow::with_device(Box::new(SimDevice(state.clone())), false);
    (state, workflow)
}

#[test]
fn reader_personalization_is_once_after_verification_with_optional_memory() {
    let text = "Use short explanations.\r\nPreserve units: Âµ and Î±.";
    for (enabled, value, memory_mode, agrees) in [
        (true, text, 0, true),
        (true, text, 1, true),
        (true, text, 2, true),
        (false, text, 0, true),
        (true, "", 0, true),
        (true, text, 0, false),
    ] {
        let (state, workflow) = setup();
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let calls = Rc::new(RefCell::new(vec![]));
        let model = Model {
            current: Request::default(), calls: calls.clone(), cancel: None,
            replies: VecDeque::from([
                "QUESTION: G unc.?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\n---\nANSWER: Original answer.".into(),
                if agrees { "TRANSCRIPTION: G unc.?".into() } else { "TRANSCRIPTION: different question?".into() },
                "ANSWER: Personalized gravity answer.".into(),
            ]),
        };
        let mut reader = Orchestrator::new(workflow, model, ledger);
        reader.set_trigger_enabled(false);
        let mut saved_preference = crate::config::Personalization {
            enabled,
            text: value.into(),
            ..Default::default()
        };
        reader.set_personalization(saved_preference.snapshot().unwrap());
        // A later saved value cannot mutate the already selected startup value.
        saved_preference.text = "A later edit must not appear in this request.".into();
        saved_preference.enabled = false;
        assert!(saved_preference
            .snapshot()
            .unwrap()
            .effective_text()
            .is_none());
        if memory_mode > 0 {
            let profile = Uuid::new_v4();
            let memory = Arc::new(memory::LocalMemory::new_local(store.clone(), profile).unwrap());
            if memory_mode == 2 {
                remember(
                    &memory,
                    profile,
                    "G unc. uses SI gravity units",
                    Uuid::new_v4(),
                );
            }
            reader.set_subject_memory(Some(memory));
        }
        reader.run_iteration().unwrap();
        let calls = calls.borrow();
        let personalized = enabled && !value.is_empty() && agrees;
        assert_eq!(calls.len(), if personalized { 3 } else { 2 });
        assert!(calls[..2].iter().all(|call| call
            .text
            .iter()
            .all(|part| !part.contains("PERSONALIZATION"))));
        if personalized {
            let expected = format!("PERSONALIZATION: {}", serde_json::to_string(value).unwrap());
            assert_eq!(
                calls[2]
                    .text
                    .iter()
                    .filter(|part| **part == expected)
                    .count(),
                1
            );
            assert!(calls[2]
                .text
                .iter()
                .any(|part| part == "CURRENT_REQUEST: \"G unc.?\""));
            assert_eq!(calls[2].images, calls[0].images);
            assert_eq!(
                calls[2]
                    .text
                    .iter()
                    .any(|part| part.starts_with("ASSOCIATIVE_MEMORY: ")),
                memory_mode == 2
            );
        }
        let turns: Vec<crate::conversation::Turn> = store
            .snapshot_heads_matching(&[Namespace::Conversation], 4096, |_| true)
            .unwrap()
            .into_iter()
            .filter_map(|entry| serde_json::from_value::<Record>(entry.payload).ok())
            .filter_map(|record| match record {
                Record::Turn(turn) => Some(turn),
                _ => None,
            })
            .collect();
        let users: Vec<_> = turns
            .iter()
            .filter(|turn| turn.role == Role::User)
            .collect();
        assert_eq!(users.len(), 1);
        assert!(users.iter().all(|turn| turn
            .text
            .as_deref()
            .is_none_or(|question| question == "G unc.?")));
        assert!(turns
            .iter()
            .all(|turn| turn.text.as_ref().is_none_or(|body| !body.contains(text))));
        if !agrees {
            assert!(state.borrow().pages.iter().all(|page| page.text.is_empty()));
        }
    }
}
fn remember(provider: &memory::LocalMemory, profile: Uuid, value: &str, key: Uuid) {
    provider
        .put(memory::Mutation {
            operation: Uuid::new_v4(),
            expected: Default::default(),
            fact: memory::Fact {
                key: memory::Key {
                    profile,
                    scope: memory::Scope::Global,
                    fact: key,
                },
                claim: "gravity convention".into(),
                value: value.into(),
                subject: "gravity".into(),
                tags: vec!["unc".into()],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Explicit synthetic fixture author assertion".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 9000,
                observed_ms: 1,
                expires_ms: None,
            },
        })
        .unwrap();
}
#[test]
fn reader_memory_disabled_empty_and_enriched_preserve_verification_and_source() {
    for mode in 0..3 {
        let (state, workflow) = setup();
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let profile = Uuid::new_v4();
        let memory = Arc::new(memory::LocalMemory::new_local(store.clone(), profile).unwrap());
        if mode != 1 {
            remember(
                &memory,
                profile,
                "G unc. uses SI gravity units",
                Uuid::new_v4(),
            );
        }
        let calls = Rc::new(RefCell::new(vec![]));
        let model = Model { current: Request::default(), calls: calls.clone(), replies: VecDeque::from([
            "QUESTION: G unc.?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\n---\nANSWER: Original proposal.".into(),
            "TRANSCRIPTION: G unc.?".into(), "ANSWER: Enriched gravity answer.".into(),
        ]), cancel: None };
        let mut reader = Orchestrator::new(workflow, model, ledger);
        reader.set_trigger_enabled(false);
        if mode > 0 {
            reader.set_subject_memory(Some(memory));
        }
        reader.run_iteration().unwrap();
        let calls = calls.borrow();
        assert_eq!(calls.len(), if mode == 2 { 3 } else { 2 });
        assert!(calls[..2]
            .iter()
            .all(|call| call.text.iter().all(|t| !t.contains("ASSOCIATIVE_MEMORY"))));
        assert_eq!(calls[0].images, calls[1].images);
        let records = store
            .snapshot_heads_matching(&[Namespace::Conversation], 4096, |_| true)
            .unwrap();
        let turns: Vec<_> = records
            .iter()
            .filter_map(|e| serde_json::from_value::<Record>(e.payload.clone()).ok())
            .filter_map(|r| {
                if let Record::Turn(t) = r {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(turns.iter().filter(|t| t.role == Role::User).count(), 1);
        assert_eq!(
            turns
                .iter()
                .find(|t| t.role == Role::User)
                .unwrap()
                .text
                .as_deref(),
            Some("G unc.?")
        );
        if mode == 2 {
            assert_eq!(calls[0].images, calls[2].images);
            assert!(calls[2]
                .text
                .iter()
                .any(|t| t == "CURRENT_REQUEST: \"G unc.?\""));
            assert!(calls[2]
                .text
                .iter()
                .any(|t| t.starts_with("ASSOCIATIVE_MEMORY: ")
                    && t.contains("author-asserted-not-source-verified")));
            assert!(turns
                .iter()
                .find(|t| t.role == Role::Assistant)
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .contains("Enriched gravity answer."));
        }
        assert_eq!(state.borrow().pages[0].text, "");
    }
}

#[test]
fn reader_withheld_only_abstention_provider_failure_and_cancel_never_publish_proposal() {
    for mode in 0..6 {
        let (state, workflow) = setup();
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let profile = Uuid::new_v4();
        let memory = Arc::new(memory::LocalMemory::new_local(store.clone(), profile).unwrap());
        remember(&memory, profile, "G unc. uses SI", Uuid::new_v4());
        if mode == 0 {
            remember(
                &memory,
                profile,
                "G unc. uses incompatible cgs",
                Uuid::new_v4(),
            );
        }
        let calls = Rc::new(RefCell::new(vec![]));
        let mut replies = VecDeque::from([
            "QUESTION: G unc.?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\n---\nANSWER: Must not publish proposal.".into(),
            "TRANSCRIPTION: G unc.?".into(),
        ]);
        if mode == 1 {
            replies.push_back("NONE".into());
        }
        if mode == 3 {
            replies.push_back("ANSWER: Enriched but stale source".into());
        }
        if mode == 4 {
            replies.push_back("Unframed malformed answer".into());
        }
        if mode == 5 {
            replies.push_back("ANSWER: Non-ASCII Î±".into());
        }
        let model = Model {
            current: Request::default(),
            calls: calls.clone(),
            replies,
            cancel: (mode == 3).then(|| state.clone()),
        };
        let mut reader = Orchestrator::new(workflow, model, ledger);
        reader.set_trigger_enabled(false);
        reader.set_subject_memory(Some(memory));
        assert!(reader.run_iteration().is_err());
        assert_eq!(calls.borrow().len(), if mode == 0 { 2 } else { 3 });
        let state = state.borrow();
        assert!(state.pages.iter().all(|p| p.text.is_empty()));
        assert!(!state
            .events
            .iter()
            .any(|event| matches!(event.action.as_str(), "text" | "next")));
        let inspect = Ledger::new(store.clone());
        let envelopes = store
            .snapshot_heads_matching(&[Namespace::Conversation], 4096, |_| true)
            .unwrap();
        assert!(!envelopes.iter().any(|e| matches!(serde_json::from_value::<Record>(e.payload.clone()), Ok(Record::Turn(t)) if t.role == Role::Assistant)));
        let expected_outcome = if mode == 3 {
            crate::conversation::Outcome::Canceled
        } else {
            crate::conversation::Outcome::Failed
        };
        assert!(envelopes.iter().any(|e| matches!(serde_json::from_value::<Record>(e.payload.clone()), Ok(Record::Turn(t)) if t.role == Role::User && t.text.as_deref() == Some("G unc.?") && t.outcome == expected_outcome)), "failure/cancellation case {mode} retained the wrong terminal turn");
        drop(inspect);
    }
}
