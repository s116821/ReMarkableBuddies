//! Scripted host Writer flow. No tablet, network, credentials or native routing.
use crate::{
    config::Personalization,
    note_export::{self, Budget, CorrelationLocation, Discovery, NoteDestination, RemoteNote},
    storage::{Fault, Namespace, Uuid},
    writer_flow::{self, Phase},
    LLMEngine,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tokio::sync::watch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub mode: String,
    pub conversation: Uuid,
    pub replies: Vec<String>,
    #[serde(default)]
    pub reader_history: Vec<String>,
    pub steps: Vec<Step>,
    pub output: PathBuf,
    pub expect: Expected,
}
#[derive(Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Step {
    Run {
        request: Box<writer_flow::Request>,
        fault: Option<PhaseFault>,
    },
    Restart,
    LoseWriteAcknowledgment,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseFault {
    pub phase: FaultPhase,
    pub action: Action,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultPhase {
    BeforeAdmission,
    AfterAdmission,
    AfterModel,
    BeforeExportAdmission,
    AfterExportAdmission,
    BeforeDestinationPoll,
    BeforeWritePoll,
}
impl FaultPhase {
    fn phase(self) -> Phase {
        match self {
            Self::BeforeAdmission => Phase::BeforeAdmission,
            Self::AfterAdmission => Phase::AfterAdmission,
            Self::AfterModel => Phase::AfterModel,
            Self::BeforeExportAdmission => Phase::BeforeExportAdmission,
            Self::AfterExportAdmission => Phase::AfterExportAdmission,
            Self::BeforeDestinationPoll => Phase::BeforeDestinationPoll,
            Self::BeforeWritePoll => Phase::BeforeWritePoll,
        }
    }
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Cancel,
    DeleteRoot,
    LoseCommitAcknowledgment,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub statuses: Vec<writer_flow::Status>,
    pub model_calls: usize,
    pub destination_calls: usize,
    pub writes: usize,
    pub note_contains: Vec<String>,
}
#[derive(Serialize)]
pub struct Report {
    pub name: String,
    pub mode: &'static str,
    pub evidence: &'static str,
    pub reports: Vec<writer_flow::Report>,
    pub reader_history_provenance: &'static str,
    pub records: Vec<crate::conversation::Record>,
    pub notes: Vec<RemoteNote>,
    pub model_calls: usize,
    pub destination_calls: usize,
    pub writes: usize,
    pub destination_trace: Vec<String>,
    pub assertion_failures: Vec<String>,
}
struct Model {
    replies: VecDeque<String>,
    calls: usize,
}
impl LLMEngine for Model {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, _: &str) {}
    fn clear_content(&mut self) {}
    fn execute(&mut self) -> Result<String> {
        self.calls += 1;
        self.replies
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("scripted Writer replies exhausted"))
    }
}
#[derive(Default)]
struct State {
    notes: BTreeMap<String, RemoteNote>,
    trace: Vec<String>,
    writes: usize,
    lose_ack: bool,
}
#[derive(Default)]
struct Destination(Mutex<State>);
impl Destination {
    fn write(&self, id: &str, content: &str, version: u64) -> note_export::Result<RemoteNote> {
        let mut s = self.0.lock().unwrap();
        s.writes += 1;
        s.trace.push(format!("write:{id}:v{version}"));
        let note = RemoteNote {
            id: id.into(),
            title: content
                .lines()
                .next()
                .unwrap_or("")
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
}
impl NoteDestination for Destination {
    fn backend(&self) -> &str {
        "scripted-host"
    }
    fn correlation_location(&self) -> CorrelationLocation {
        CorrelationLocation::Title
    }
    async fn discover(&self, correlation: Uuid, _: Budget) -> note_export::Result<Discovery> {
        let mut s = self.0.lock().unwrap();
        s.trace.push(format!("discover:{correlation}"));
        let marker = format!("[RMB:{correlation}]");
        let notes: Vec<_> = s
            .notes
            .values()
            .filter(|n| n.title.contains(&marker))
            .cloned()
            .collect();
        Ok(match notes.as_slice() {
            [] => Discovery::Missing,
            [note] => Discovery::Unique(note.clone()),
            _ => Discovery::Duplicate,
        })
    }
    async fn read(&self, id: &str, _: Budget) -> note_export::Result<Option<RemoteNote>> {
        let mut s = self.0.lock().unwrap();
        s.trace.push(format!("read:{id}"));
        Ok(s.notes.get(id).cloned())
    }
    async fn create(
        &self,
        id: &str,
        content: &str,
        _: Uuid,
        _: Budget,
    ) -> note_export::Result<RemoteNote> {
        self.write(id, content, 1)
    }
    async fn update(
        &self,
        id: &str,
        content: &str,
        version: u64,
        _: Uuid,
        _: Budget,
    ) -> note_export::Result<RemoteNote> {
        self.write(id, content, version + 1)
    }
}
impl Scenario {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.mode == "writer",
            "only supplied-text Writer host simulation is supported"
        );
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 200 && !self.conversation.is_nil(),
            "invalid scenario identity"
        );
        ensure!(
            !self.steps.is_empty() && self.steps.len() <= 64 && self.replies.len() <= 64,
            "invalid step/reply count"
        );
        ensure!(
            self.replies.iter().all(|r| r.len() <= 24 * 1024),
            "reply too large"
        );
        ensure!(!self.output.as_os_str().is_empty(), "missing output");
        ensure!(
            self.reader_history.len() <= 16
                && self
                    .reader_history
                    .iter()
                    .all(|text| !text.trim().is_empty() && text.len() <= 4096),
            "invalid fixture Reader history"
        );
        for step in &self.steps {
            if let Step::Run { request, .. } = step {
                request.preflight()?;
                ensure!(
                    request.conversation() == self.conversation,
                    "foreign conversation"
                );
            }
        }
        Ok(())
    }
}
pub fn execute(scenario: &Scenario) -> Result<Report> {
    scenario.validate()?;
    let owned = super::SimulatorLedger::new();
    let mut ledger = owned.open()?;
    ledger.create(scenario.conversation, Uuid::new_v4(), 1)?;
    for text in &scenario.reader_history {
        use crate::conversation::{Mode, Outcome, Role, Turn};
        let expected = ledger.expected(Namespace::Conversation, scenario.conversation)?;
        ledger.append(
            Uuid::new_v4(),
            expected,
            Turn {
                id: Uuid::new_v4(),
                conversation: scenario.conversation,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::User,
                mode: Mode::Reader,
                outcome: Outcome::Interpreted,
                text: Some(text.clone()),
                sources: vec![],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            },
            vec![],
        )?;
    }
    let mut model = Model {
        replies: scenario.replies.clone().into(),
        calls: 0,
    };
    let destination = Destination::default();
    let preference = Personalization::default().snapshot()?;
    let mut reports = vec![];
    for step in &scenario.steps {
        match step {
            Step::Restart => {
                drop(ledger);
                ledger = owned.open()?;
            }
            Step::LoseWriteAcknowledgment => destination.0.lock().unwrap().lose_ack = true,
            Step::Run { request, fault } => {
                let (tx, rx) = watch::channel(false);
                let mut fired = false;
                let report = writer_flow::run_with_events(
                    &ledger,
                    &mut model,
                    &destination,
                    request,
                    &preference,
                    rx,
                    &mut |phase, ledger| {
                        if let Some(fault) = fault {
                            if !fired && phase == fault.phase.phase() {
                                fired = true;
                                match fault.action {
                                    Action::Cancel => {
                                        let _ = tx.send(true);
                                    }
                                    Action::LoseCommitAcknowledgment => {
                                        ledger.store().set_fault(Fault::AfterCommit).unwrap()
                                    }
                                    Action::DeleteRoot => {
                                        let expected = ledger
                                            .expected(
                                                Namespace::Conversation,
                                                scenario.conversation,
                                            )
                                            .unwrap();
                                        ledger
                                            .delete(
                                                scenario.conversation,
                                                Uuid::new_v4(),
                                                expected,
                                                None,
                                            )
                                            .unwrap();
                                    }
                                }
                            }
                        }
                    },
                );
                reports.push(report);
            }
        }
    }
    let records = ledger.inspect(scenario.conversation, true)?;
    let state = destination.0.into_inner().unwrap();
    let mut failures = vec![];
    let statuses: Vec<_> = reports.iter().map(|r| r.status).collect();
    if statuses != scenario.expect.statuses {
        failures.push(format!("statuses: {statuses:?}"));
    }
    for (label, actual, expected) in [
        ("model_calls", model.calls, scenario.expect.model_calls),
        (
            "destination_calls",
            state.trace.len(),
            scenario.expect.destination_calls,
        ),
        ("writes", state.writes, scenario.expect.writes),
    ] {
        if actual != expected {
            failures.push(format!("{label}: expected {expected}, got {actual}"));
        }
    }
    let content = state
        .notes
        .values()
        .map(|n| n.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in &scenario.expect.note_contains {
        if !content.contains(expected) {
            failures.push(format!("missing note text: {expected}"));
        }
    }
    Ok(Report {
        name: scenario.name.clone(),
        mode: "writer",
        evidence: "scripted supplied-text host flow; no native output",
        reports,
        reader_history_provenance: "seeded fixture history; no native Reader-to-Writer transition",
        records,
        notes: state.notes.into_values().collect(),
        model_calls: model.calls,
        destination_calls: state.trace.len(),
        writes: state.writes,
        destination_trace: state.trace,
        assertion_failures: failures,
    })
}
pub fn save(scenario: &Scenario, root: &Path) -> Result<()> {
    let report = execute(scenario)?;
    let output = root.join(&scenario.output);
    std::fs::create_dir_all(&output)?;
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "Writer host simulator report: {}",
        output.join("report.json").display()
    );
    ensure!(
        report.assertion_failures.is_empty(),
        "Writer simulator assertions failed: {}",
        report.assertion_failures.join("; ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintained_writer_host_scenarios_use_production_flow() {
        for name in [
            "writer-create-replay",
            "writer-create-refine",
            "writer-final-cancel",
            "writer-reconcile",
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs/simulator/scenarios")
                .join(format!("{name}.json"));
            let scenario: Scenario = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let report = execute(&scenario).unwrap();
            assert!(
                report.assertion_failures.is_empty(),
                "{name}: {:?}",
                report.assertion_failures
            );
            if name == "writer-reconcile" {
                let last = report.reports.last().unwrap();
                assert!(last.observation.destination_polled);
                assert!(!last.observation.write_polled);
            }
            if name == "writer-create-refine" {
                assert!(report.records.iter().any(|r|matches!(r,crate::conversation::Record::Turn(t) if t.mode==crate::conversation::Mode::Reader)));
                assert_eq!(report.notes.len(), 1);
            }
        }
    }
    #[test]
    fn writer_schema_refuses_combined_reader_fields_and_duplicates() {
        let bytes = include_str!("../../docs/simulator/scenarios/writer-create-replay.json");
        let mut v: serde_json::Value = serde_json::from_str(bytes).unwrap();
        v["pages"] = serde_json::json!([]);
        assert!(serde_json::from_value::<Scenario>(v).is_err());
        let duplicate = bytes.replacen(
            "\"mode\": \"writer\"",
            "\"mode\": \"writer\", \"mode\": \"writer\"",
            1,
        );
        assert!(serde_json::from_str::<Scenario>(&duplicate).is_err());
        let combined = bytes.replacen("\"mode\": \"writer\"", "\"mode\": \"combined\"", 1);
        assert!(serde_json::from_str::<Scenario>(&combined)
            .unwrap()
            .validate()
            .is_err());
    }
}
