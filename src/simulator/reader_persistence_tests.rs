use super::*;
use crate::{
    conversation::*,
    storage::{Fault as StorageFault, Namespace, Store},
};
use std::sync::Arc;

type Calls = Rc<RefCell<Vec<Vec<Vec<u8>>>>>;
struct RecordingModel {
    images: Vec<String>,
    calls: Calls,
    store: Arc<Store>,
    replies: VecDeque<String>,
    fail: bool,
    fault_after_second: bool,
    cancel_after_first: Option<Shared>,
}
impl LLMEngine for RecordingModel {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.into());
    }
    fn clear_content(&mut self) {
        self.images.clear();
    }
    fn execute(&mut self) -> Result<String> {
        let ledger = Ledger::new(self.store.clone());
        let records = records(&ledger);
        let evidence = records
            .iter()
            .find_map(|r| match r {
                Record::LegacyCapture(e) => Some(e),
                _ => None,
            })
            .context("no prepared evidence before provider")?;
        let actual = self
            .images
            .iter()
            .map(|i| STANDARD.decode(i).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            actual == ledger.stored_legacy_images(evidence.id)?.images,
            "provider bytes differ from storage"
        );
        self.calls.borrow_mut().push(actual);
        if self.fault_after_second && self.calls.borrow().len() == 2 {
            self.store.set_fault(StorageFault::BeforeCommit)?;
        }
        ensure!(!self.fail, "provider failed");
        if self.calls.borrow().len() == 1 {
            if let Some(state) = &self.cancel_after_first {
                state.borrow_mut().request_lost = true;
            }
        }
        self.replies.pop_front().context("model replies exhausted")
    }
}
fn records(ledger: &Ledger) -> Vec<Record> {
    ledger
        .store()
        .snapshot_heads_matching(&[Namespace::Conversation, Namespace::Source], 100, |_| true)
        .unwrap()
        .into_iter()
        .filter_map(|e| serde_json::from_value(e.payload).ok())
        .collect()
}
fn setup() -> Shared {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    let scenario: Scenario =
        serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap()).unwrap();
    let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
    state.borrow_mut().refuse_repeated_details = true;
    state
}
fn model(store: Arc<Store>, calls: Calls) -> RecordingModel {
    RecordingModel {images:vec![],calls,store,replies:VecDeque::from([
        "QUESTION: Why?\nQUESTION_BOX: 20,20,30,30\nSELECTION_CENTER: 300,400\n---\nANSWER: Because.".into(),
        "TRANSCRIPTION: Why?".into()]),fail:false,fault_after_second:false,cancel_after_first:None}
}
#[test]
fn no_output_disagreement_cancellation_and_device_uncertainty_remain_explicit() {
    for case in 0..5 {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let state = setup();
        let calls = Rc::new(RefCell::new(vec![]));
        let mut model = model(store.clone(), calls.clone());
        let expected = match case {
            0 => {
                state.borrow_mut().pages.truncate(1);
                AttemptReason::NoSuccessor
            }
            1 => {
                state.borrow_mut().pages[1].background.fill(0);
                AttemptReason::InvalidSuccessor
            }
            2 => {
                state.borrow_mut().faults.push(scenario::Fault {
                    operation: Operation::Text,
                    call: 1,
                    effect: scenario::Effect::Error,
                });
                AttemptReason::DeviceUncertain
            }
            3 => {
                model.replies[1] = "TRANSCRIPTION: Different".into();
                AttemptReason::TranscriptionDisagreement
            }
            _ => {
                model.cancel_after_first = Some(state.clone());
                AttemptReason::Canceled
            }
        };
        let mut orchestrator = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model,
            ledger,
        );
        orchestrator.set_trigger_enabled(false);
        let result = orchestrator.run_iteration();
        assert_eq!(
            result.is_err(),
            matches!(case, 2 | 4),
            "case {case}: {result:?}"
        );
        let recorded = records(&Ledger::new(store.clone()));
        assert!(
            recorded
                .iter()
                .any(|r| matches!(r,Record::OutcomeFact(f) if f.reason==expected)),
            "case{case}"
        );
        assert!(!recorded
            .iter()
            .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Completed)));
        assert_eq!(state.borrow().detail_queries, 1);
        if case >= 3 {
            assert_eq!(
                state
                    .borrow()
                    .counts
                    .get(&Operation::Next)
                    .copied()
                    .unwrap_or(0),
                0
            );
            assert_eq!(
                state
                    .borrow()
                    .counts
                    .get(&Operation::Text)
                    .copied()
                    .unwrap_or(0),
                0
            );
        }
    }
}
#[test]
fn unsupported_or_failed_sdk_capture_never_downgrades_or_dispatches() {
    use crate::device::backend::AcquisitionKind;
    for kind in [AcquisitionKind::Unsupported, AcquisitionKind::Sdk] {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let state = setup();
        state.borrow_mut().acquisition_kind = kind;
        let calls = Rc::new(RefCell::new(vec![]));
        let mut orchestrator = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model(store.clone(), calls.clone()),
            ledger,
        );
        orchestrator.set_trigger_enabled(false);
        assert!(orchestrator.run_iteration().is_err());
        assert!(calls.borrow().is_empty());
        assert!(records(&Ledger::new(store)).is_empty());
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Capture)
                .copied()
                .unwrap_or(0),
            0
        );
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Text)
                .copied()
                .unwrap_or(0),
            0
        );
    }
}
#[test]
fn actual_orchestrator_uses_stored_batch_twice_and_records_unverified_output() {
    let fixture = SimulatorLedger::new();
    let ledger = fixture.open().unwrap();
    let store = ledger.store().clone();
    let state = setup();
    let initial = state.borrow().pages[0].image();
    let calls = Rc::new(RefCell::new(vec![]));
    let mut orchestrator = Orchestrator::new(
        Workflow::with_device(Box::new(SimDevice(state.clone())), false),
        model(store.clone(), calls.clone()),
        ledger,
    );
    orchestrator.set_trigger_enabled(false);
    orchestrator.run_iteration().unwrap();
    assert_eq!(calls.borrow().len(), 2);
    assert_eq!(calls.borrow()[0], calls.borrow()[1]);
    assert_eq!(state.borrow().detail_queries, 1);
    assert_eq!(state.borrow().pages[0].image(), initial);
    let recorded = records(&Ledger::new(store.clone()));
    assert!(recorded.iter().any(|r|matches!(r,Record::Turn(t) if t.role==Role::Assistant && t.outcome==Outcome::ReconcileRequired)));
    assert!(!recorded
        .iter()
        .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Completed)));
    drop(orchestrator);
    drop(store);
    let ledger = fixture.open().unwrap();
    let recorded = records(&ledger);
    assert!(recorded.iter().any(
        |r| matches!(r,Record::OutcomeFact(f) if f.reason==AttemptReason::SubmittedUnverified)
    ));
    // Opening/reconstructing the ledger makes no device/model effects.
    assert_eq!(calls.borrow().len(), 2);
}
#[test]
fn storage_failure_prevents_provider_or_output_and_provider_failure_is_durable() {
    for case in 0..3 {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let state = setup();
        let calls = Rc::new(RefCell::new(vec![]));
        let mut model = model(store.clone(), calls.clone());
        if case == 0 {
            store.set_fault(StorageFault::BeforeCommit).unwrap();
        }
        if case == 1 {
            model.fault_after_second = true;
        }
        if case == 2 {
            model.fail = true;
        }
        let mut orchestrator = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model,
            ledger,
        );
        orchestrator.set_trigger_enabled(false);
        assert!(orchestrator.run_iteration().is_err());
        assert_eq!(calls.borrow().len(), [0, 2, 1][case]);
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Next)
                .copied()
                .unwrap_or(0),
            0
        );
        assert_eq!(
            state
                .borrow()
                .counts
                .get(&Operation::Text)
                .copied()
                .unwrap_or(0),
            0
        );
        if case == 2 {
            assert!(records(&Ledger::new(store.clone()))
                .iter()
                .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Failed)));
        }
    }
}
