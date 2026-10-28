//! One owned cooperative map/reduce invocation over actual Ledger publications.
use super::*;
use crate::{
    conversation::{
        notebook::{
            notebook_input_digest, NotebookAdmission, NotebookFailure, NotebookFinalized,
            NotebookResult, NotebookRunCursor, NotebookStageKind, NotebookStageOutcome,
            NotebookStagePublication, MAP_PROMPT, MAX_MAP_REPLY, REDUCE_PROMPT,
        },
        Ledger,
    },
    LLMEngine,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Generated,
    Historical,
    Refused,
    Failed,
    Canceled,
    Stale,
    Uncertain,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    InvalidInput,
    Busy,
    Storage,
    Provider,
    Output,
    Deadline,
    Canceled,
    Stale,
}
#[derive(Serialize)]
pub struct Report {
    pub status: Status,
    pub conversation: Uuid,
    pub operation: Uuid,
    pub draft: Option<Uuid>,
    pub failure: Option<Failure>,
    /// Observed engine-interface entries, not provider billing assurance.
    pub local_invocations: usize,
    pub confirmed_stage_results: usize,
    pub admitted: bool,
    pub unobserved_publication: bool,
    pub remote_cost_unknown: bool,
}
#[derive(Clone)]
pub struct Input {
    pub admission: NotebookAdmission,
    pub payloads: FrozenNotebookPayloads,
}
pub struct Completed {
    pub report: Report,
    pub finalized: Option<NotebookFinalized>,
}

pub fn run<E: LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    input: Input,
    canceled: &mut dyn FnMut() -> bool,
) -> Completed {
    let Some(owner) = super::super::Owner::acquire(ledger.store()) else {
        let mut report = Report::new(&input);
        report.status = Status::Refused;
        report.failure = Some(Failure::Busy);
        return Completed {
            report,
            finalized: None,
        };
    };
    run_owned(ledger, engine, input, canceled, &owner, Instant::now())
}
impl Report {
    fn new(input: &Input) -> Self {
        Self {
            status: Status::Refused,
            conversation: input.admission.plan.conversation,
            operation: input.admission.plan.operation,
            draft: None,
            failure: None,
            local_invocations: 0,
            confirmed_stage_results: 0,
            admitted: false,
            unobserved_publication: false,
            remote_cost_unknown: false,
        }
    }
}
pub(crate) fn run_owned<E: LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    input: Input,
    canceled: &mut dyn FnMut() -> bool,
    owner: &super::super::Owner<'_>,
    started: Instant,
) -> Completed {
    let mut report = Report::new(&input);
    let result = (|| -> Result<Option<NotebookFinalized>> {
        ensure!(
            std::ptr::eq(owner.0, ledger.store().as_ref()),
            "foreign notebook writer owner"
        );
        ensure!(
            input.admission.plan == input.payloads.plan,
            "notebook frozen plan mismatch"
        );
        if canceled() {
            report.status = Status::Canceled;
            report.failure = Some(Failure::Canceled);
            return Ok(None);
        }
        let Input {
            admission,
            payloads,
        } = input;
        let Some(mut cursor) = ledger.admit_notebook(admission)? else {
            report.status = Status::Historical;
            return Ok(None);
        };
        report.admitted = true;
        {
            let mut runner = Runner {
                ledger,
                engine: &mut *engine,
                canceled: &mut *canceled,
                report: &mut report,
                started,
            };
            for ordinal in 0..payloads.plan.chunks.len() {
                let Some(next) = runner.step(
                    cursor,
                    NotebookStageKind::Map,
                    ordinal,
                    payloads.map_parts(ordinal)?,
                )?
                else {
                    return Ok(None);
                };
                cursor = next;
            }
            let outputs = cursor.map_outputs()?;
            let parts = payloads.reducer_parts(&outputs)?;
            let Some(next) = runner.step(cursor, NotebookStageKind::Reduce, 0, &parts)? else {
                return Ok(None);
            };
            cursor = next;
        }
        if canceled() || started.elapsed() >= Duration::from_secs(600) {
            report.status = Status::Canceled;
            report.failure = Some(Failure::Canceled);
            return Ok(None);
        }
        let Some((_, finalized)) = ledger.finalize_notebook_granted(cursor)? else {
            report.status = Status::Uncertain;
            report.unobserved_publication = true;
            return Ok(None);
        };
        report.status = Status::Generated;
        report.draft = Some(finalized.draft().record_id);
        Ok(Some(finalized))
    })();
    match result {
        Ok(finalized) => Completed { report, finalized },
        Err(_) => {
            report.status = Status::Stale;
            report.failure = Some(Failure::Storage);
            report.unobserved_publication = true;
            report.remote_cost_unknown |= report.local_invocations > 0;
            Completed {
                report,
                finalized: None,
            }
        }
    }
}
struct Runner<'a, E> {
    ledger: &'a Ledger,
    engine: &'a mut E,
    canceled: &'a mut dyn FnMut() -> bool,
    report: &'a mut Report,
    started: Instant,
}
impl<E: LLMEngine> Runner<'_, E> {
    fn step(
        &mut self,
        cursor: NotebookRunCursor,
        kind: NotebookStageKind,
        ordinal: usize,
        parts: &[String],
    ) -> Result<Option<NotebookRunCursor>> {
        let revision = match kind {
            NotebookStageKind::Map => MAP_PROMPT,
            NotebookStageKind::Reduce => REDUCE_PROMPT,
        };
        let input_digest = notebook_input_digest(revision, parts)?;
        let Some(prepared) =
            self.ledger
                .prepare_notebook_stage(cursor, kind, ordinal, input_digest.clone())?
        else {
            self.report.status = Status::Uncertain;
            self.report.unobserved_publication = true;
            return Ok(None);
        };
        ensure!(
            prepared.input_digest()? == input_digest,
            "notebook prepared input changed"
        );
        let stage_started = Instant::now();
        let mut stop = None;
        let mut invoked = false;
        let response = {
            let ledger = self.ledger;
            let canceled = &mut *self.canceled;
            let started = self.started;
            let mut progress = || -> Result<()> {
                if canceled() {
                    stop = Some(Failure::Canceled);
                    anyhow::bail!("notebook canceled")
                }
                if started.elapsed() >= Duration::from_secs(600)
                    || stage_started.elapsed() >= Duration::from_secs(95)
                {
                    stop = Some(Failure::Deadline);
                    anyhow::bail!("notebook deadline")
                }
                if !ledger.notebook_stage_current(&prepared).unwrap_or(false) {
                    stop = Some(Failure::Stale);
                    anyhow::bail!("notebook state changed")
                }
                Ok(())
            };
            let content = super::super::Content(&mut *self.engine);
            content.0.clear_content();
            if progress().is_err() {
                None
            } else {
                for part in parts {
                    content.0.add_text_content(part);
                }
                invoked = true;
                self.report.local_invocations += 1;
                let response = content.0.execute_with_progress(&mut progress);
                let _ = progress();
                Some(response)
            }
        };
        let body = response.as_ref().and_then(|result| result.as_ref().ok());
        let observed_output_digest = body.map(|body| digest(body.as_bytes()));
        let observed_output_bytes = body.map(|body| body.len() as u64);
        let (outcome, output, failure, status, reported) = if let Some(stop) = stop {
            self.report.remote_cost_unknown |= invoked;
            let (outcome, failure, status) = match stop {
                Failure::Canceled => (
                    NotebookStageOutcome::Canceled,
                    NotebookFailure::Canceled,
                    Status::Canceled,
                ),
                Failure::Deadline => (
                    NotebookStageOutcome::Canceled,
                    NotebookFailure::Deadline,
                    Status::Canceled,
                ),
                _ => (
                    NotebookStageOutcome::Uncertain,
                    NotebookFailure::PublicationUnknown,
                    Status::Stale,
                ),
            };
            (outcome, None, Some(failure), status, Some(stop))
        } else if let Some(body) = body {
            let valid = match kind {
                NotebookStageKind::Map => {
                    !body.trim().is_empty()
                        && !body.contains('\0')
                        && serde_json::to_vec(body)?.len() <= MAX_MAP_REPLY
                }
                NotebookStageKind::Reduce => super::super::parse_draft(body).is_ok(),
            };
            if valid {
                (
                    NotebookStageOutcome::Generated,
                    Some(body.clone()),
                    None,
                    Status::Generated,
                    None,
                )
            } else {
                (
                    NotebookStageOutcome::Failed,
                    None,
                    Some(NotebookFailure::InvalidOutput),
                    Status::Failed,
                    Some(Failure::Output),
                )
            }
        } else {
            self.report.remote_cost_unknown |= invoked;
            (
                NotebookStageOutcome::Failed,
                None,
                Some(NotebookFailure::Provider),
                Status::Failed,
                Some(Failure::Provider),
            )
        };
        let publication = self.ledger.finish_notebook_stage_observed(
            prepared,
            NotebookResult {
                outcome,
                invoked,
                output,
                observed_output_digest,
                observed_output_bytes,
                failure,
            },
        )?;
        match publication {
            NotebookStagePublication::Generated(cursor) => {
                self.report.confirmed_stage_results += 1;
                Ok(Some(cursor))
            }
            NotebookStagePublication::Stopped => {
                self.report.confirmed_stage_results += 1;
                self.report.status = status;
                self.report.failure = reported;
                Ok(None)
            }
            NotebookStagePublication::Unobserved => {
                self.report.status = Status::Uncertain;
                self.report.unobserved_publication = true;
                self.report.remote_cost_unknown |= invoked;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Personalization,
        conversation::{ArchivedNormalizedSource, ExpectedHeads, Mode, Outcome, Role, Turn},
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
        storage::{Fault, Namespace, Store, StorePaths},
    };
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("notebook-runner-{}", Uuid::new_v4())))
        }
        fn paths(&self) -> StorePaths {
            StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn input(ledger: &Ledger) -> Input {
        let conversation = Uuid::new_v4();
        let operation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 0).unwrap();
        let document = NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "runner-fixture-v1".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "Force = 10 N.".into(),
            }],
        };
        let bytes = serde_json::to_vec(&document).unwrap();
        let source = ValidatedNotebook::from_bytes(&bytes, &digest(&bytes)).unwrap();
        let user_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:writer:v1:{operation}:user").as_bytes(),
        );
        let archive = ArchivedNormalizedSource::from_notebook(
            conversation,
            user_id,
            &source,
            TextScope::SuppliedNotebook,
            None,
        )
        .unwrap();
        let preference = Personalization::default().snapshot().unwrap();
        let payloads = FrozenNotebookPayloads::new(
            &source,
            &archive,
            PayloadContext {
                operation,
                request: "Generate study notes",
                personalization: &preference,
                history: &[],
                history_range: None,
                prior: None,
                max_model_calls: 2,
            },
        )
        .unwrap();
        let user = Turn {
            id: user_id,
            conversation,
            exchange: operation,
            sequence: 0,
            role: Role::User,
            mode: Mode::Writer,
            outcome: Outcome::Interpreted,
            text: Some(payloads.plan.request.clone()),
            sources: vec![archive.id],
            correction_of: None,
            created_ms: 0,
            updated_ms: 0,
            completion: None,
        };
        let expected: ExpectedHeads = ledger
            .expected(Namespace::Conversation, conversation)
            .unwrap();
        Input {
            admission: NotebookAdmission {
                plan: payloads.plan.clone(),
                archive,
                canonical: source.canonical_bytes().to_vec(),
                user,
                expected,
                guards: vec![],
            },
            payloads,
        }
    }
    struct Model {
        store: Arc<Store>,
        calls: usize,
        parts: Vec<String>,
        lost_result: Option<usize>,
        bad_output: bool,
        cancel: Option<Arc<AtomicBool>>,
    }
    impl LLMEngine for Model {
        fn add_text_content(&mut self, text: &str) {
            self.parts.push(text.into());
        }
        fn add_image_content(&mut self, _: &str) {
            panic!("notebook runner is text-only")
        }
        fn clear_content(&mut self) {
            self.parts.clear();
        }
        fn execute(&mut self) -> Result<String> {
            assert!(self.store.writer_active.load(Ordering::Acquire));
            assert!(self.parts.len() >= 2);
            self.calls += 1;
            if self.lost_result == Some(self.calls) {
                self.store.set_fault(Fault::AfterCommit).unwrap();
            }
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::Release);
            }
            Ok(if self.bad_output {
                "x".repeat(MAX_MAP_REPLY + 1)
            } else if self.calls % 2 == 1 {
                "Force = 10 N. (page 1).".into()
            } else {
                "# Force\n\nForce = 10 N. (page 1).".into()
            })
        }
    }
    fn model(store: Arc<Store>) -> Model {
        Model {
            store,
            calls: 0,
            parts: vec![],
            lost_result: None,
            bad_output: false,
            cancel: None,
        }
    }
    #[test]
    fn production_runner_owns_all_calls_records_final_and_replays_with_no_engine_entry() {
        let f = Fixture::new();
        let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
        let ledger = Ledger::new(store.clone());
        let input = input(&ledger);
        let replay = input.clone();
        let mut model = model(store.clone());
        let result = run(&ledger, &mut model, input, &mut || false);
        assert_eq!(result.report.status, Status::Generated);
        assert_eq!(model.calls, 2);
        assert_eq!(result.report.local_invocations, 2);
        assert_eq!(result.report.confirmed_stage_results, 2);
        let finalized = result.finalized.unwrap();
        assert!(finalized.is_for_store(&store));
        assert_eq!(
            store
                .value(Namespace::Conversation, finalized.root().record_id)
                .unwrap()
                .unwrap()
                .revision_id,
            finalized.root().revision_id
        );
        assert!(model.parts.is_empty());
        assert!(!store.writer_active.load(Ordering::Acquire));
        let result = run(&ledger, &mut model, replay, &mut || false);
        assert_eq!(result.report.status, Status::Historical);
        assert_eq!(result.report.local_invocations, 0);
        assert_eq!(model.calls, 2);
        assert!(result.finalized.is_none());
    }
    #[test]
    fn lost_map_or_reducer_result_ack_stops_before_next_call_or_final_grant() {
        for fault in [1, 2] {
            let f = Fixture::new();
            let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
            let ledger = Ledger::new(store.clone());
            let input = input(&ledger);
            let replay = input.clone();
            let mut model = model(store.clone());
            model.lost_result = Some(fault);
            let result = run(&ledger, &mut model, input, &mut || false);
            assert_eq!(result.report.status, Status::Uncertain);
            assert_eq!(model.calls, fault);
            assert_eq!(result.report.confirmed_stage_results, fault - 1);
            assert!(result.report.unobserved_publication);
            assert!(result.finalized.is_none());
            assert!(model.parts.is_empty());
            let result = run(&ledger, &mut model, replay, &mut || false);
            assert_eq!(result.report.local_invocations, 0);
            assert_eq!(model.calls, fault);
        }
    }
    #[test]
    fn invalid_reply_or_post_response_cancellation_is_durably_stopped_without_reduction() {
        for canceled in [false, true] {
            let f = Fixture::new();
            let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
            let ledger = Ledger::new(store.clone());
            let input = input(&ledger);
            let mut model = model(store.clone());
            model.bad_output = !canceled;
            let flag = Arc::new(AtomicBool::new(false));
            if canceled {
                model.cancel = Some(flag.clone());
            }
            let result = run(&ledger, &mut model, input, &mut || {
                flag.load(Ordering::Acquire)
            });
            assert_eq!(model.calls, 1);
            assert_eq!(result.report.confirmed_stage_results, 1);
            assert_eq!(
                result.report.status,
                if canceled {
                    Status::Canceled
                } else {
                    Status::Failed
                }
            );
            assert!(result.finalized.is_none());
            assert!(model.parts.is_empty());
        }
    }
}
