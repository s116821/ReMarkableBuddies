//! Deterministic Reader scenarios through the production orchestrator.
mod device;
mod raster;
pub mod scenario;

use crate::{LLMEngine, OpenAI, Orchestrator, Workflow};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use device::{Event, Shared, SimDevice, State};
use scenario::{ModelConfig, Operation, Reply, Scenario};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    path::Path,
    rc::Rc,
};

struct ScriptedModel {
    state: Shared,
    replies: VecDeque<Reply>,
    text_count: usize,
    images: Vec<String>,
}

struct LiveModel<M> {
    inner: M,
    state: Shared,
    max_calls: usize,
}
impl<M: LLMEngine> LLMEngine for LiveModel<M> {
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
        self.execute_with_progress(&mut || Ok(()))
    }
    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        {
            let mut state = self.state.borrow_mut();
            if state.model_calls >= self.max_calls {
                state.event("model_limit", "live request allowance exhausted");
                anyhow::bail!("Live model request allowance exhausted");
            }
            state.model_calls += 1;
            state.event("model_request", "live provider");
        }
        let result = self.inner.execute_with_progress(progress);
        self.state.borrow_mut().event(
            "model_response",
            if result.is_ok() {
                "live reply"
            } else {
                "live error"
            },
        );
        result
    }
}
impl LLMEngine for ScriptedModel {
    fn add_text_content(&mut self, _text: &str) {
        self.text_count += 1;
    }
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.to_owned());
    }
    fn clear_content(&mut self) {
        self.text_count = 0;
        self.images.clear();
    }
    fn execute(&mut self) -> Result<String> {
        let mut state = self.state.borrow_mut();
        state.model_calls += 1;
        state.event(
            "model_request",
            format!(
                "scripted texts={} images={}",
                self.text_count,
                self.images.len()
            ),
        );
        for encoded in &self.images {
            let bytes = STANDARD
                .decode(encoded)
                .context("Invalid model image base64")?;
            image::load_from_memory(&bytes).context("Invalid model image PNG")?;
        }
        let reply = self
            .replies
            .pop_front()
            .context("Scripted model replies exhausted")?;
        if let Some(error) = reply.error {
            state.event("model_response", "scripted error");
            anyhow::bail!("Scripted model error: {error}");
        }
        state.event("model_response", "scripted reply");
        Ok(reply.text.unwrap()) // Validated before device initialization.
    }
}

#[derive(Serialize)]
pub struct PageResult {
    pub index: usize,
    pub text: String,
    pub x_count: usize,
    pub failure_codes: Vec<String>,
    pub indicator_visible: bool,
    pub unchanged: bool,
    pub png: String,
}
#[derive(Serialize)]
pub struct Report {
    pub history: crate::workflow::history::State,
    pub name: String,
    pub model_mode: &'static str,
    pub active_page: usize,
    pub virtual_ms: u64,
    pub model_calls: usize,
    pub operations: BTreeMap<Operation, usize>,
    pub pages: Vec<PageResult>,
    pub errors: Vec<String>,
    pub assertion_failures: Vec<String>,
    pub trace: Vec<Event>,
}
pub struct Run {
    pub report: Report,
    pub images: Vec<image::RgbaImage>,
}

pub fn execute(scenario: &Scenario, root: &Path) -> Result<Run> {
    scenario.validate()?;
    // Read live configuration before initializing even the simulated device.
    let live = match &scenario.llm {
        ModelConfig::Scripted => None,
        ModelConfig::Live {
            model,
            timeout_seconds,
            ..
        } => Some(
            OpenAI::from_env(model.clone())?
                .with_timeout(std::time::Duration::from_secs(*timeout_seconds)),
        ),
    };
    let state = Rc::new(RefCell::new(State::new(scenario, root)?));
    if let (ModelConfig::Live { max_calls, .. }, Some(inner)) = (&scenario.llm, live) {
        let model = LiveModel {
            inner,
            state: state.clone(),
            max_calls: *max_calls,
        };
        return execute_with_model(scenario, state, model, "live-provider");
    }
    let model = ScriptedModel {
        state: state.clone(),
        replies: scenario.replies.clone().into(),
        text_count: 0,
        images: Vec::new(),
    };
    execute_with_model(scenario, state, model, "scripted-offline")
}

fn execute_with_model<M: LLMEngine>(
    scenario: &Scenario,
    state: Shared,
    model: M,
    model_mode: &'static str,
) -> Result<Run> {
    let workflow = Workflow::with_device(Box::new(SimDevice(state.clone())), false);
    let mut orchestrator = Orchestrator::new(workflow, model);
    let mut errors = Vec::new();
    for (index, iteration) in scenario.iterations.iter().enumerate() {
        if let Some(page) = iteration.page {
            let mut state = state.borrow_mut();
            state.active = page;
            state.visit += 1;
            state.event("scenario_page_selected", page.to_string());
        }
        state.borrow_mut().event("iteration", index.to_string());
        orchestrator.set_trigger_enabled(iteration.wait_for_trigger);
        if let Err(error) = orchestrator.run_iteration() {
            errors.push(format!("{error:#}"));
        }
        history_actions(&iteration.actions, &state, &mut orchestrator, &mut errors)?;
    }
    let history = orchestrator.history_state();
    let state = state.borrow();
    let images: Vec<_> = state.pages.iter().map(device::Page::image).collect();
    let pages: Vec<_> = state
        .pages
        .iter()
        .enumerate()
        .map(|(index, page)| PageResult {
            index,
            text: page.text.clone(),
            x_count: page.x_count(),
            failure_codes: page.failure_codes(),
            indicator_visible: page.indicator_visible,
            unchanged: images[index] == state.initial[index],
            png: format!("page-{index}.png"),
        })
        .collect();
    let mut failures = Vec::new();
    if let Some(expected) = &scenario.expect.history {
        let actual = serde_json::to_value(history)?.as_str().unwrap().to_owned();
        if expected != &actual {
            failures.push(format!("history expected {expected}, got {actual}"));
        }
    }
    if errors.is_empty() && pages.iter().any(|page| page.indicator_visible) {
        failures.push("Successful iteration left a temporary circle".into());
    }
    if let Some(expected) = scenario.expect.active_page {
        if expected != state.active {
            failures.push(format!(
                "active_page expected {expected}, got {}",
                state.active
            ));
        }
    }
    if let Some(expected) = scenario.expect.model_calls {
        if expected != state.model_calls {
            failures.push(format!(
                "model_calls expected {expected}, got {}",
                state.model_calls
            ));
        }
    }
    for (&page, expected) in &scenario.expect.text {
        if pages[page].text != *expected {
            failures.push(format!("page {page} text differs from expected"));
        }
    }
    for (&page, expected) in &scenario.expect.text_contains {
        for substring in expected {
            if !pages[page].text.contains(substring) {
                failures.push(format!("page {page} missing expected text substring"));
            }
        }
    }
    for (&page, &expected) in &scenario.expect.x_count {
        if pages[page].x_count != expected {
            failures.push(format!(
                "page {page} x_count expected {expected}, got {}",
                pages[page].x_count
            ));
        }
    }
    for (&page, expected) in &scenario.expect.failure_codes {
        if &pages[page].failure_codes != expected {
            failures.push(format!(
                "page {page} failure codes expected {expected:?}, got {:?}",
                pages[page].failure_codes
            ));
        }
    }
    for &page in &scenario.expect.unchanged_pages {
        if !pages[page].unchanged {
            failures.push(format!("page {page} was modified"));
        }
    }
    for (&operation, &expected) in &scenario.expect.operations {
        let count = state.counts.get(&operation).copied().unwrap_or(0);
        if count != expected {
            failures.push(format!("{operation:?} expected {expected}, got {count}"));
        }
    }
    if errors.len() != scenario.expect.errors.len()
        || !errors
            .iter()
            .zip(&scenario.expect.errors)
            .all(|(actual, expected)| actual.contains(expected))
    {
        failures.push(format!(
            "errors expected {:?}, got {errors:?}",
            scenario.expect.errors
        ));
    }
    if matches!(scenario.llm, ModelConfig::Scripted) && state.model_calls != scenario.replies.len()
    {
        failures.push(format!(
            "{} scripted replies but {} calls",
            scenario.replies.len(),
            state.model_calls
        ));
    }
    if state.used_faults != scenario.faults.len() {
        failures.push("Not every declared fault was reached".into());
    }
    Ok(Run {
        report: Report {
            history,
            name: scenario.name.clone(),
            model_mode,
            active_page: state.active,
            virtual_ms: state.clock,
            model_calls: state.model_calls,
            operations: state.counts.clone(),
            pages,
            errors,
            assertion_failures: failures,
            trace: state.events.clone(),
        },
        images,
    })
}

fn history_actions<M: LLMEngine>(
    actions: &[scenario::HistoryAction],
    state: &Shared,
    orchestrator: &mut Orchestrator<M>,
    errors: &mut Vec<String>,
) -> Result<()> {
    use crate::{
        device::interaction::{ContactReducer, Interaction},
        workflow::history::Action,
    };
    use scenario::HistoryAction;
    use std::time::Duration;
    let deliver =
        |events: Vec<Interaction>, orchestrator: &mut Orchestrator<M>, errors: &mut Vec<String>| {
            if events.contains(&Interaction::Invalidated) {
                orchestrator.invalidate_history();
            }
            for event in events {
                state
                    .borrow_mut()
                    .event("history_interaction", format!("{event:?}"));
                let action = match event {
                    Interaction::Undo => Some(Action::Undo),
                    Interaction::Redo => Some(Action::Redo),
                    _ => {
                        orchestrator.invalidate_history();
                        None
                    }
                };
                if let Some(action) = action {
                    match orchestrator.history_action(action) {
                        Ok(changed) => state
                            .borrow_mut()
                            .event("history_result", format!("changed={changed}")),
                        Err(error) => errors.push(format!("{error:#}")),
                    }
                }
            }
        };
    for action in actions {
        match action {
            HistoryAction::Hold { frames } => {
                let mut reducer = ContactReducer::new(state.borrow().corner.clone());
                let start = state.borrow().clock;
                let mut previous = Vec::new();
                let mut timer = 0;
                for frame in frames {
                    while timer < frame.at_ms {
                        state.borrow_mut().clock = start + timer;
                        deliver(
                            reducer.frame(&previous, Duration::from_millis(timer)),
                            orchestrator,
                            errors,
                        );
                        timer += 10;
                    }
                    state.borrow_mut().clock = start + frame.at_ms;
                    deliver(
                        reducer.frame(&frame.contacts, Duration::from_millis(frame.at_ms)),
                        orchestrator,
                        errors,
                    );
                    previous = frame.contacts.clone();
                    timer = frame.at_ms + 10;
                }
                if !previous.is_empty() {
                    deliver(vec![reducer.cancel()], orchestrator, errors);
                }
            }
            HistoryAction::Page { page } => {
                orchestrator.invalidate_history();
                let mut state = state.borrow_mut();
                state.active = *page;
                state.visit += 1;
                state.event(
                    "history_departure",
                    "visit advanced; returning cannot revive ownership",
                );
            }
            HistoryAction::Edit { text } => {
                orchestrator.invalidate_history();
                let mut state = state.borrow_mut();
                let page = state.active;
                state.pages[page].text.push_str(text);
                state.event("history_external_edit", "manual text retained");
            }
            HistoryAction::InputLost => {
                orchestrator.invalidate_history();
                state
                    .borrow_mut()
                    .event("history_input_lost", "observation discontinuity");
            }
            HistoryAction::Restart => {
                orchestrator.invalidate_history();
                let mut state = state.borrow_mut();
                state.session += 1;
                state.event("history_restart", "native session changed");
            }
        }
        state.borrow_mut().event(
            "history_state",
            format!("{:?}", orchestrator.history_state()),
        );
    }
    Ok(())
}

pub fn run_file(path: &Path) -> Result<Report> {
    let scenario: Scenario = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("Read scenario {}", path.display()))?,
    )?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let run = execute(&scenario, root)?;
    let output = root.join(&scenario.output);
    std::fs::create_dir_all(&output)?;
    for (index, image) in run.images.iter().enumerate() {
        image.save(output.join(format!("page-{index}.png")))?;
    }
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&run.report)?,
    )?;
    println!("Simulator report: {}", output.join("report.json").display());
    ensure!(
        run.report.assertion_failures.is_empty(),
        "Simulator assertions failed: {}",
        run.report.assertion_failures.join("; ")
    );
    Ok(run.report)
}

#[cfg(test)]
mod indicator_capture_tests {
    use super::*;
    use crate::workflow::{indicator, AnswerPageType};
    use image::Rgba;

    fn blank_successor() -> (Shared, Workflow) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
        let scenario: Scenario =
            serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap())
                .unwrap();
        let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
        state.borrow_mut().active = 1;
        let workflow = Workflow::with_device(Box::new(SimDevice(state.clone())), false);
        (state, workflow)
    }

    #[test]
    fn all_stages_have_three_edges_at_333_ms_with_bounded_retracing_cleanup() {
        use indicator::{Stage, Stroke};
        let (state, mut workflow) = blank_successor();
        workflow.capture_page_data().unwrap();
        workflow.tick_indicator().unwrap();
        workflow.set_indicator_stage(Stage::AnswerPending);
        workflow.tick_indicator().unwrap();
        workflow.set_indicator_stage(Stage::AnswerReady);
        workflow.finish_indicator_stage().unwrap();
        workflow.auxiliary_indicator().unwrap();
        for _ in 0..30 {
            workflow.tick_indicator().unwrap();
        }
        {
            let state = state.borrow();
            let paths: Vec<_> = state
                .events
                .iter()
                .filter(|e| e.action == "status_path")
                .collect();
            assert_eq!(paths.len(), 39);
            assert!(paths.windows(2).all(|p| p[1].at_ms - p[0].at_ms == 333));
            assert_eq!(paths[0].detail, "Edge(Preparing, 0)");
            assert_eq!(paths[3].detail, "Edge(AnswerPending, 0)");
            assert_eq!(paths[6].detail, "Edge(AnswerReady, 0)");
            assert_eq!(paths[9].detail, "Auxiliary(AnswerReady)");
            assert_eq!(state.pages[1].indicator_paths.len(), 10);
            assert_eq!(
                state.pages[1].indicator_paths.last(),
                Some(&(Stroke::Auxiliary(Stage::AnswerReady), 30))
            );
        }
        workflow.clear_indicator().unwrap();
        assert!(state.borrow().pages[1].indicator_paths.is_empty());
        assert!(state.borrow().pages[1].lines.is_empty());
    }

    #[test]
    fn repeated_loop_failures_never_type_diagnostics_or_retry_failed_markers() {
        use crate::device::interaction::Interaction;
        use scenario::{Effect, Fault};
        for failure in ["provider", "capture", "marker"] {
            let (state, workflow) = blank_successor();
            state.borrow_mut().idle_events = vec![vec![Interaction::Reader]; 2].into();
            if failure == "capture" {
                state.borrow_mut().faults = (1..=2)
                    .map(|call| Fault {
                        operation: Operation::Capture,
                        call,
                        effect: Effect::Error,
                    })
                    .collect();
            } else if failure == "marker" {
                state.borrow_mut().faults.push(Fault {
                    operation: Operation::Line,
                    call: 1,
                    effect: Effect::Error,
                });
            }
            let model = ScriptedModel {
                state: state.clone(),
                replies: vec![
                    Reply {
                        text: None,
                        error: Some("network unavailable".into())
                    };
                    2
                ]
                .into(),
                text_count: 0,
                images: Vec::new(),
            };
            let mut orchestrator = Orchestrator::new(workflow, model);
            assert!(orchestrator
                .run_loop()
                .unwrap_err()
                .to_string()
                .contains("no remaining input"));
            let state = state.borrow();
            assert_eq!(state.model_calls, if failure == "capture" { 0 } else { 2 });
            assert!(state
                .pages
                .iter()
                .all(|p| p.text.is_empty() && !p.indicator_visible));
            assert!(!state
                .events
                .iter()
                .any(|e| matches!(e.action.as_str(), "text" | "body")));
            if failure == "provider" {
                // First error mark makes the corner occupied on the next trigger.
                assert_eq!(state.pages[1].failure_codes(), ["Provider"]);
                assert_eq!(state.counts.get(&Operation::Line), Some(&3));
            } else if failure == "capture" {
                assert!(state.pages[1].lines.is_empty());
            } else {
                // One failed first attempt, then one normal marker next iteration.
                assert_eq!(state.counts.get(&Operation::Line), Some(&4));
                assert_eq!(state.pages[1].failure_codes(), ["Provider"]);
            }
        }
    }

    #[test]
    fn idle_loop_preserves_history_until_reader_and_prioritizes_input_loss() {
        use crate::{device::interaction::Interaction as I, workflow::history::State as H};
        for (events, mutations) in [
            (vec![vec![I::Undo], vec![I::Redo], vec![I::Reader]], 2),
            (vec![vec![I::Undo, I::Invalidated], vec![I::Reader]], 0),
            // Observer initialization fallback must retain the Reader event
            // while invalidating its former history in the very same batch.
            (vec![vec![I::Invalidated, I::Reader]], 0),
        ] {
            let (state, mut workflow) = blank_successor();
            workflow.render_text("Header\n").unwrap();
            workflow.render_qa("Q: new?\nA: yes.\n---\n").unwrap();
            assert_eq!(workflow.history_state(), H::Applied);
            state.borrow_mut().idle_events = events.into();
            workflow.wait_for_reader().unwrap();
            assert_eq!(workflow.history_state(), H::Empty);
            let state = state.borrow();
            assert!(state.idle_events.is_empty());
            assert_eq!(
                state
                    .counts
                    .get(&Operation::HistoryMutation)
                    .copied()
                    .unwrap_or(0),
                mutations
            );
            assert_eq!(state.model_calls, 0);
            assert_eq!(state.pages[1].text, "Header\nQ: new?\nA: yes.\n---\n");
        }
    }

    #[test]
    fn classification_clears_owned_paths_before_capture() {
        let (state, mut workflow) = blank_successor();
        workflow.capture_page_data().unwrap();
        workflow.tick_indicator().unwrap();
        assert!(state.borrow().pages[1].indicator_visible);
        assert_eq!(
            workflow.is_valid_answer_page().unwrap(),
            AnswerPageType::Blank
        );
        let state = state.borrow();
        assert!(!state.pages[1].indicator_visible);
        assert_eq!(state.counts.get(&Operation::StatusClear), Some(&1));
    }

    #[test]
    fn status_style_restoration_failure_stops_future_input() {
        use scenario::{Effect, Fault};
        let (state, mut workflow) = blank_successor();
        workflow.capture_page_data().unwrap();
        workflow.tick_indicator().unwrap();
        state.borrow_mut().faults.push(Fault {
            operation: Operation::StatusStyleEnd,
            call: 1,
            effect: Effect::Error,
        });
        assert!(workflow.clear_indicator().is_err());
        assert!(workflow.cleanup_failed());
        assert!(workflow.begin_iteration().is_err());
        assert!(!state.borrow().pages[1].indicator_visible);
        assert!(state.borrow().status_style_active);
    }

    #[test]
    fn unsupported_style_suppresses_without_finish_loop_or_strokes() {
        use scenario::{Effect, Fault};
        let (state, mut workflow) = blank_successor();
        workflow.capture_page_data().unwrap();
        state.borrow_mut().faults.push(Fault {
            operation: Operation::StatusStyleBegin,
            call: 1,
            effect: Effect::Unavailable,
        });
        workflow.set_indicator_stage(indicator::Stage::AnswerReady);
        workflow.finish_indicator_stage().unwrap();
        workflow.tick_indicator().unwrap();
        let state = state.borrow();
        assert_eq!(state.counts.get(&Operation::StatusStyleBegin), Some(&1));
        assert!(!state.counts.contains_key(&Operation::StatusStroke));
        assert!(!state.status_style_active);
    }

    #[test]
    fn classification_refreshes_eligibility_from_settled_frame() {
        let (state, mut workflow) = blank_successor();
        workflow.capture_page_data().unwrap();
        state.borrow_mut().pages[1].background.put_pixel(
            indicator::LEFT as u32,
            indicator::TOP as u32,
            Rgba([0, 0, 0, 255]),
        );
        assert_eq!(
            workflow.is_valid_answer_page().unwrap(),
            AnswerPageType::Blank
        );
        workflow.tick_indicator().unwrap();
        let state = state.borrow();
        assert!(!state.pages[1].indicator_visible);
        assert!(!state.counts.contains_key(&Operation::StatusStroke));
        assert!(!state.counts.contains_key(&Operation::StatusClear));
        assert!(state
            .events
            .iter()
            .any(|event| event.action == "status_suppressed"));
        assert_eq!(
            state.pages[1]
                .background
                .get_pixel(indicator::LEFT as u32, indicator::TOP as u32,),
            &Rgba([0, 0, 0, 255])
        );
    }
}
