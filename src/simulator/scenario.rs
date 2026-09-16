use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    #[serde(default = "reader")]
    pub mode: String,
    pub pages: Vec<PageSpec>,
    #[serde(default)]
    pub active_page: usize,
    #[serde(default = "corner")]
    pub trigger_corner: String,
    pub iterations: Vec<Iteration>,
    #[serde(default)]
    pub gestures: Vec<Vec<GestureFrame>>,
    #[serde(default)]
    pub replies: Vec<Reply>,
    #[serde(default)]
    pub faults: Vec<Fault>,
    pub header_image: Option<PathBuf>,
    pub output: PathBuf,
    #[serde(default)]
    pub expect: Expected,
}
fn reader() -> String {
    "reader".into()
}
fn corner() -> String {
    "LL".into()
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSpec {
    pub image: Option<PathBuf>,
    #[serde(default)]
    pub text: String,
    pub strokes: Option<PathBuf>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Iteration {
    pub page: Option<usize>,
    #[serde(default)]
    pub wait_for_trigger: bool,
}

/// Complete slot-zero frames in virtual coordinates, relative to this wait.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureFrame {
    pub at_ms: u64,
    pub contact: bool,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub text: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Capture,
    Next,
    Previous,
    Text,
    Body,
    Line,
    Trigger,
    HeaderSave,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Error,
    NoMove,
    Stale,
    Corrupt,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub operation: Operation,
    pub call: usize,
    pub effect: Effect,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub active_page: Option<usize>,
    pub model_calls: Option<usize>,
    #[serde(default)]
    pub text: BTreeMap<usize, String>,
    #[serde(default)]
    pub x_count: BTreeMap<usize, usize>,
    #[serde(default)]
    pub unchanged_pages: Vec<usize>,
    #[serde(default)]
    pub operations: BTreeMap<Operation, usize>,
    #[serde(default)]
    pub errors: Vec<String>,
}

impl Scenario {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.mode == "reader",
            "Unsupported simulator mode {:?}; Writer/combined require REM-23",
            self.mode
        );
        ensure!(!self.name.trim().is_empty(), "Scenario name is empty");
        ensure!(
            !self.pages.is_empty() && self.pages.len() <= 100,
            "Scenario requires 1..100 pages"
        );
        ensure!(self.active_page < self.pages.len(), "Invalid active_page");
        ensure!(
            !self.iterations.is_empty() && self.iterations.len() <= 100,
            "Scenario requires 1..100 iterations"
        );
        crate::TriggerCorner::from_string(&self.trigger_corner)?;
        ensure!(
            !self.output.as_os_str().is_empty(),
            "Output directory is empty"
        );
        for iteration in &self.iterations {
            ensure!(
                iteration.page.is_none_or(|p| p < self.pages.len()),
                "Invalid iteration page"
            );
        }
        for reply in &self.replies {
            ensure!(
                reply.text.is_some() != reply.error.is_some(),
                "Reply needs exactly one of text/error"
            );
        }
        for frames in &self.gestures {
            let mut previous = None;
            for frame in frames {
                ensure!(
                    (0..768).contains(&frame.x) && (0..1024).contains(&frame.y),
                    "Gesture outside virtual screen"
                );
                ensure!(
                    frame.at_ms <= 60000 && previous.is_none_or(|p| frame.at_ms > p),
                    "Gesture times must increase within 60000 ms"
                );
                previous = Some(frame.at_ms);
            }
        }
        let mut faults = std::collections::BTreeSet::new();
        for fault in &self.faults {
            ensure!(
                fault.call > 0 && faults.insert((fault.operation, fault.call)),
                "Invalid or duplicate fault call"
            );
            ensure!(
                match fault.effect {
                    Effect::Error => true,
                    Effect::NoMove =>
                        matches!(fault.operation, Operation::Next | Operation::Previous),
                    Effect::Stale | Effect::Corrupt => fault.operation == Operation::Capture,
                },
                "Fault effect does not apply to operation"
            );
        }
        for page in self
            .expect
            .text
            .keys()
            .chain(self.expect.x_count.keys())
            .chain(self.expect.unchanged_pages.iter())
        {
            ensure!(
                *page < self.pages.len(),
                "Expected page index out of bounds"
            );
        }
        ensure!(
            self.expect.active_page.is_none_or(|p| p < self.pages.len()),
            "Expected active page out of bounds"
        );
        Ok(())
    }
}
