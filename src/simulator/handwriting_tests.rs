//! Actual production Reader calls with synthetic device/model data.
use super::*;
use crate::{
    config::Config,
    handwriting::{self, LocalCorrections, Provider},
    storage::{Store, Uuid},
    workflow::conversation_dispatch::Dispatch,
};
use serde_json::{json, Value};
use std::{io::Cursor, sync::Arc};
struct Model {
    parts: Rc<RefCell<Vec<Value>>>,
    calls: Rc<RefCell<Vec<Vec<Value>>>>,
    store: Arc<Store>,
    state: Shared,
    cancel: bool,
    fail: bool,
    disagree: bool,
}
impl LLMEngine for Model {
    fn add_text_content(&mut self, s: &str) {
        self.parts
            .borrow_mut()
            .push(json!({"type":"text","text":s}));
    }
    fn add_image_content(&mut self, s: &str) {
        self.parts
            .borrow_mut()
            .push(json!({"type":"image_url","image_url":s}));
    }
    fn clear_content(&mut self) {
        self.parts.borrow_mut().clear();
    }
    fn execute(&mut self) -> Result<String> {
        let mode = self.store.sync_mode_snapshot()?;
        ensure!(
            self.store
                .set_sync_mode(
                    &mode.token,
                    Uuid::new_v4(),
                    crate::storage::sync::mode::SyncModeChoice::LocalOnly
                )
                .is_err(),
            "operation lost actual mode custody"
        );
        self.calls.borrow_mut().push(self.parts.borrow().clone());
        if self.cancel {
            self.state.borrow_mut().request_lost = true;
        }
        ensure!(!self.fail, "synthetic provider failure");
        if self.calls.borrow().len() == 1 {
            Ok("QUESTION: Why?\nQUESTION_BOX: 20,20,30,30\nSELECTION_CENTER: 300,400\n---\nANSWER: Because.".into())
        } else if self.disagree {
            Ok("TRANSCRIPTION: Different?".into())
        } else {
            Ok("TRANSCRIPTION: Why?".into())
        }
    }
}
#[test]
fn runtime_handwriting_actual_reader_two_passes_none_compatibility_and_failures() {
    let mut no_context = None;
    for case in 0..7 {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let profile = Uuid::new_v4();
        let mut config = Config {
            paths: store.paths.clone(),
            ..Config::default()
        };
        config.handwriting = handwriting::Configuration {
            enabled: case > 0,
            profile: Some(profile),
        };
        let mut sample = Vec::new();
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut Cursor::new(&mut sample), image::ImageFormat::Png)
            .unwrap();
        if case >= 2 {
            for _ in 0..if case == 6 { 3 } else { 1 } {
                let request = json!({"handwriting_version":1,"command":"confirm","profile":profile,"correction":Uuid::new_v4(),"operation":Uuid::new_v4(),"expected_heads":[],"confirmation":"user-confirmed","original_text":if case==6 {format!("{}x","\t".repeat(4095))}else{"EXAMPLE_ORIGINAL".into()},"corrected_text":if case==6 {format!("{}y","\t".repeat(4095))}else{"EXAMPLE_CONFIRMED".into()},"confirmed_ms":1,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic Reader integration control","source":null},"sample_png_base64":STANDARD.encode(&sample)});
                let input =
                    handwriting::parse(&mut Cursor::new(serde_json::to_vec(&request).unwrap()))
                        .unwrap();
                drop(
                    LocalCorrections::new(store.clone(), &config)
                        .unwrap()
                        .execute(input),
                );
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
        let scenario: Scenario =
            serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap())
                .unwrap();
        let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
        state.borrow_mut().refuse_repeated_details = true;
        let calls = Rc::new(RefCell::new(Vec::new()));
        let parts = Rc::new(RefCell::new(Vec::new()));
        let model = Model {
            parts: parts.clone(),
            calls: calls.clone(),
            store: store.clone(),
            state: state.clone(),
            cancel: case == 3,
            fail: case == 4,
            disagree: case == 5,
        };
        let dispatch = Dispatch::local(&config, &ledger).unwrap();
        let mut reader = Orchestrator::new(
            Workflow::with_device(Box::new(SimDevice(state.clone())), false),
            model,
            ledger,
        );
        reader.set_trigger_enabled(false);
        reader.set_local_handwriting(&dispatch).unwrap();
        let _ = reader.run_iteration();
        assert!(parts.borrow().is_empty());
        let calls = calls.borrow();
        assert_eq!(
            calls.len(),
            if case == 6 {
                0
            } else if case == 3 || case == 4 {
                1
            } else {
                2
            }
        );
        if case == 0 {
            no_context = Some(calls.clone());
        }
        if case == 1 {
            assert_eq!(Some(&*calls), no_context.as_ref());
        }
        if (2..6).contains(&case) {
            for call in calls.iter() {
                assert!(call.iter().any(|p| p["text"]
                    .as_str()
                    .is_some_and(|s| s.contains("EXAMPLE_CONFIRMED"))));
                assert_eq!(
                    call.iter().filter_map(|p| p["image_url"].as_str()).next(),
                    Some(STANDARD.encode(&sample).as_str())
                );
            }
            if calls.len() == 2 {
                let text = calls[1]
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    !text.contains("Because.")
                        && !text.contains("QUESTION_BOX:")
                        && !text.contains("QUESTION: Why?")
                );
                let images = |call: &Vec<Value>| {
                    call.iter()
                        .filter_map(|p| p["image_url"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                };
                assert_eq!(images(&calls[0]), images(&calls[1]));
            }
        }
        if case >= 3 {
            assert!(state.borrow().pages.iter().all(|page| page.text.is_empty()));
        }
    }
}
