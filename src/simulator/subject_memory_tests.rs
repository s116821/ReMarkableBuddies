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
            replies.push_back("ANSWER: Non-ASCII α".into());
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
        assert!(envelopes.iter().any(|e| matches!(serde_json::from_value::<Record>(e.payload.clone()), Ok(Record::Turn(t)) if t.role == Role::User && t.text.as_deref() == Some("G unc.?") && matches!(t.outcome, crate::conversation::Outcome::Failed | crate::conversation::Outcome::Canceled))));
        drop(inspect);
    }
}
