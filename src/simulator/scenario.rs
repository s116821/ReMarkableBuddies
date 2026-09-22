use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    #[serde(default = "reader")]
    pub mode: String,
    #[serde(default)]
    pub llm: ModelConfig,
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

#[derive(Clone, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelConfig {
    #[default]
    Scripted,
    Live {
        model: Option<String>,
        #[serde(default = "max_calls")]
        max_calls: usize,
        #[serde(default = "timeout_seconds")]
        timeout_seconds: u64,
    },
}
fn max_calls() -> usize {
    2
}
fn timeout_seconds() -> u64 {
    90
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
    #[serde(default)]
    pub actions: Vec<HistoryAction>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryAction {
    Hold { frames: Vec<ContactFrame> },
    Page { page: usize },
    Edit { text: String },
    InputLost,
    Restart,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactFrame {
    pub at_ms: u64,
    pub contacts: Vec<crate::device::interaction::Contact>,
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
    StatusStroke,
    StatusClear,
    StatusStyleBegin,
    StatusStyleEnd,
    HistorySnapshot,
    HistoryMutation,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Error,
    NoMove,
    Stale,
    Corrupt,
    Lag,
    Partial,
    WrongPage,
    Unavailable,
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
    pub text_contains: BTreeMap<usize, Vec<String>>,
    #[serde(default)]
    pub x_count: BTreeMap<usize, usize>,
    #[serde(default)]
    pub failure_codes: BTreeMap<usize, Vec<String>>,
    #[serde(default)]
    pub unchanged_pages: Vec<usize>,
    #[serde(default)]
    pub operations: BTreeMap<Operation, usize>,
    #[serde(default)]
    pub errors: Vec<String>,
    pub history: Option<String>,
}

impl Scenario {
    pub fn validate(&self) -> Result<()> {
        if let ModelConfig::Live {
            model,
            max_calls,
            timeout_seconds,
        } = &self.llm
        {
            ensure!(
                self.replies.is_empty(),
                "Live scenarios cannot include scripted replies"
            );
            ensure!(
                (1..=200).contains(max_calls),
                "Live max_calls must be in 1..200"
            );
            ensure!(
                (1..=300).contains(timeout_seconds),
                "Live timeout_seconds must be in 1..300"
            );
            ensure!(
                model.as_ref().is_none_or(|m| !m.trim().is_empty()),
                "Live model is empty"
            );
        }
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
            ensure!(iteration.actions.len() <= 100, "Too many history actions");
            for action in &iteration.actions {
                match action {
                    HistoryAction::Page { page } => {
                        ensure!(*page < self.pages.len(), "Invalid history page")
                    }
                    HistoryAction::Edit { text } => {
                        ensure!(text.len() <= 16000, "History edit too long")
                    }
                    HistoryAction::Hold { frames } => {
                        ensure!(
                            !frames.is_empty() && frames.len() <= 1000,
                            "Invalid contact frame count"
                        );
                        let mut previous = None;
                        for frame in frames {
                            ensure!(
                                frame.at_ms <= 60000 && previous.is_none_or(|t| frame.at_ms > t),
                                "Invalid contact frame time"
                            );
                            ensure!(
                                frame.contacts.len() <= crate::device::contact_frames::MAX_SLOTS,
                                "Too many contacts"
                            );
                            previous = Some(frame.at_ms);
                        }
                    }
                    _ => {}
                }
            }
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
                    Effect::NoMove => matches!(
                        fault.operation,
                        Operation::Next | Operation::Previous | Operation::StatusClear
                    ),
                    Effect::Stale => matches!(
                        fault.operation,
                        Operation::Capture | Operation::HistorySnapshot
                    ),
                    Effect::Corrupt => matches!(
                        fault.operation,
                        Operation::Capture | Operation::HistoryMutation
                    ),
                    Effect::Lag => fault.operation == Operation::HistorySnapshot,
                    Effect::Partial => fault.operation == Operation::HistoryMutation,
                    Effect::WrongPage => fault.operation == Operation::HistorySnapshot,
                    Effect::Unavailable => matches!(
                        fault.operation,
                        Operation::HistorySnapshot | Operation::StatusStyleBegin
                    ),
                },
                "Fault effect does not apply to operation"
            );
        }
        for page in self
            .expect
            .text
            .keys()
            .chain(self.expect.text_contains.keys())
            .chain(self.expect.x_count.keys())
            .chain(self.expect.failure_codes.keys())
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
