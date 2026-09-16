//! Deterministic Reader scenarios through the production orchestrator.
mod device;
mod raster;
pub mod scenario;

use crate::{LLMEngine, Orchestrator, Workflow};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use device::{Event, Shared, SimDevice, State};
use scenario::{Operation, Reply, Scenario};
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
    pub unchanged: bool,
    pub png: String,
}
#[derive(Serialize)]
pub struct Report {
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
    let state = Rc::new(RefCell::new(State::new(scenario, root)?));
    let workflow = Workflow::with_device(Box::new(SimDevice(state.clone())), false);
    let model = ScriptedModel {
        state: state.clone(),
        replies: scenario.replies.clone().into(),
        text_count: 0,
        images: Vec::new(),
    };
    let mut orchestrator = Orchestrator::new(workflow, model);
    let mut errors = Vec::new();
    for (index, iteration) in scenario.iterations.iter().enumerate() {
        if let Some(page) = iteration.page {
            let mut state = state.borrow_mut();
            state.active = page;
            state.event("scenario_page_selected", page.to_string());
        }
        state.borrow_mut().event("iteration", index.to_string());
        orchestrator.set_trigger_enabled(iteration.wait_for_trigger);
        if let Err(error) = orchestrator.run_iteration() {
            errors.push(format!("{error:#}"));
        }
    }
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
            unchanged: images[index] == state.initial[index],
            png: format!("page-{index}.png"),
        })
        .collect();
    let mut failures = Vec::new();
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
    for (&page, &expected) in &scenario.expect.x_count {
        if pages[page].x_count != expected {
            failures.push(format!(
                "page {page} x_count expected {expected}, got {}",
                pages[page].x_count
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
    if state.model_calls != scenario.replies.len() {
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
            name: scenario.name.clone(),
            model_mode: "scripted-offline",
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
