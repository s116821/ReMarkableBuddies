//! Explicit notebook commands. Paths stay transport-only; saved stages never resume.
use super::*;
use crate::{
    config::{self, Config, PersonalizationSnapshot},
    conversation::{
        notebook::{ArchivedNormalizedSource, NotebookAdmission, NotebookPlan},
        ContextBudget, Ledger, Mode, Outcome, Record, Role, Turn, TurnRange,
    },
    storage::{Namespace, Store},
};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Generate {
    pub conversation: Uuid,
    pub operation: Uuid,
    pub request: String,
    pub document_file: PathBuf,
    pub document_sha256: String,
    pub selection: TextScope,
    pub guidance: Option<String>,
    pub max_model_calls: usize,
    pub history_range: Option<TurnRange>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Refine {
    pub conversation: Uuid,
    pub operation: Uuid,
    pub request: String,
    pub draft: Uuid,
    pub max_model_calls: usize,
    pub history_range: Option<TurnRange>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    GenerateNotebook(Box<Generate>),
    RefineNotebook(Box<Refine>),
    GenerateNotebookAndExport {
        outer_operation: Uuid,
        generation: Box<Request>,
        correlation: Uuid,
        effect_operation: Uuid,
    },
    RefineNotebookAndExport {
        outer_operation: Uuid,
        refinement: Box<Request>,
        correlation: Uuid,
        effect_operation: Uuid,
    },
}
impl Request {
    pub fn model(&self) -> &Self {
        match self {
            Self::GenerateNotebookAndExport { generation, .. } => generation,
            Self::RefineNotebookAndExport { refinement, .. } => refinement,
            _ => self,
        }
    }
    pub fn conversation(&self) -> Uuid {
        match self.model() {
            Self::GenerateNotebook(r) => r.conversation,
            Self::RefineNotebook(r) => r.conversation,
            _ => Uuid::nil(),
        }
    }
    pub fn operation(&self) -> Uuid {
        match self.model() {
            Self::GenerateNotebook(r) => r.operation,
            Self::RefineNotebook(r) => r.operation,
            _ => Uuid::nil(),
        }
    }
    pub fn joined(&self) -> Option<flow::Identity> {
        match self {
            Self::GenerateNotebookAndExport {
                outer_operation,
                correlation,
                effect_operation,
                ..
            }
            | Self::RefineNotebookAndExport {
                outer_operation,
                correlation,
                effect_operation,
                ..
            } => Some(flow::Identity {
                outer_operation: *outer_operation,
                correlation: *correlation,
                effect_operation: *effect_operation,
            }),
            _ => None,
        }
    }
    fn fields(&self) -> Result<(&str, usize, Option<TurnRange>)> {
        match self.model() {
            Self::GenerateNotebook(r) => Ok((&r.request, r.max_model_calls, r.history_range)),
            Self::RefineNotebook(r) => Ok((&r.request, r.max_model_calls, r.history_range)),
            _ => anyhow::bail!("notebook model variant required"),
        }
    }
    pub fn preflight(&self) -> Result<()> {
        match self {
            Self::GenerateNotebookAndExport { generation, .. } => ensure!(
                matches!(generation.as_ref(), Self::GenerateNotebook(_)),
                "notebook generation variant required"
            ),
            Self::RefineNotebookAndExport { refinement, .. } => ensure!(
                matches!(refinement.as_ref(), Self::RefineNotebook(_)),
                "notebook refinement variant required"
            ),
            _ => (),
        }
        let (text, calls, range) = self.fields()?;
        ensure!(
            !self.conversation().is_nil()
                && !self.operation().is_nil()
                && self.conversation() != self.operation()
                && !text.trim().is_empty()
                && text.len() <= 4096
                && !text.contains('\0')
                && (1..=9).contains(&calls)
                && range.is_none_or(|r| r.start < r.end_exclusive),
            "invalid notebook request"
        );
        match self.model() {
            Self::GenerateNotebook(r) => ensure!(
                !r.document_file.as_os_str().is_empty()
                    && crate::storage::valid_digest(&r.document_sha256)
                    && !matches!(r.selection, TextScope::Page { page: 0 })
                    && r.guidance.as_ref().is_none_or(|s| !s.trim().is_empty()
                        && s.len() <= 4096
                        && !s.contains('\0')),
                "invalid notebook generation input"
            ),
            Self::RefineNotebook(r) => ensure!(
                !r.draft.is_nil() && r.draft != r.operation && r.draft != r.conversation,
                "invalid notebook draft identity"
            ),
            _ => anyhow::bail!("notebook model variant required"),
        }
        if let Some(identity) = self.joined() {
            let draft = Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("urn:rmb:writer:v1:{}:draft", self.operation()).as_bytes(),
            );
            let ids = [
                self.conversation(),
                self.operation(),
                draft,
                identity.outer_operation,
                identity.correlation,
                identity.effect_operation,
            ];
            ensure!(
                ids.iter().all(|id| !id.is_nil())
                    && ids
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == ids.len(),
                "notebook flow identities collide"
            );
        }
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(untagged)]
pub enum Report {
    Generation(runner::Report),
    Joined(Box<flow::Report>),
}
struct Replay {
    plan: NotebookPlan,
    draft: Option<Uuid>,
}
enum Prepared {
    Fresh(Box<runner::Input>),
    Replay(Box<Replay>),
}

fn initial_history(ledger: &Ledger, plan: &NotebookPlan) -> Result<Vec<Turn>> {
    let candidates = ledger.store().snapshot_revisions_matching(
        &[Namespace::Conversation],
        crate::storage::MAX_ITEMS,
        |e| e.record_id == plan.id && e.parents.is_empty(),
    )?;
    ensure!(candidates.len() == 1, "original notebook plan ambiguous");
    let reference = ledger.store().notebook_revision_reference(&candidates[0])?;
    let manifests = ledger.store().manifests()?;
    let manifest = manifests
        .iter()
        .find(|m| m.records.iter().any(|r| r == &reference))
        .context("original notebook publication absent")?;
    let user = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:writer:v1:{}:user", plan.operation).as_bytes(),
    );
    let mut history = Vec::new();
    let mut bytes = 0u64;
    for reference in &manifest.records {
        bytes = bytes
            .checked_add(reference.bytes)
            .context("notebook history byte overflow")?;
        ensure!(
            bytes <= crate::storage::MAX_METADATA as u64,
            "notebook history inspection bound"
        );
        let envelope: crate::storage::Envelope =
            serde_json::from_slice(&ledger.store().read_object(reference)?)?;
        if let Record::Turn(turn) = Ledger::decode(&envelope)? {
            if turn.id != user && matches!(turn.outcome, Outcome::Completed | Outcome::Interpreted)
            {
                history.push(turn);
            }
        }
    }
    history.sort_by_key(|turn| turn.sequence);
    Ok(history)
}
fn prepare(
    ledger: &Ledger,
    request: &Request,
    preference: &PersonalizationSnapshot,
) -> Result<Prepared> {
    request.preflight()?;
    ensure!(
        ledger.store().unavailable_commits()? == 0,
        "incomplete notebook history"
    );
    let (text, calls, range) = request.fields()?;
    let plan_id = NotebookPlan::identity(request.operation());
    if let Some(envelope) = ledger.store().value(Namespace::Conversation, plan_id)? {
        let Record::NotebookPlan(plan) = Ledger::decode(&envelope)? else {
            anyhow::bail!("notebook operation identity changed")
        };
        ensure!(
            plan.operation == request.operation()
                && plan.conversation == request.conversation()
                && plan.request == text
                && plan.caller_max_model_calls == calls,
            "notebook replay input changed"
        );
        let source = ledger
            .store()
            .value(Namespace::Source, plan.archive_id)?
            .context("notebook replay archive absent")?;
        let Record::ArchivedNormalizedSource(archive) = Ledger::decode(&source)? else {
            anyhow::bail!("notebook replay archive absent")
        };
        archive.load(ledger.store())?;
        match request.model() {
            Request::GenerateNotebook(input) => ensure!(
                input.document_sha256 == archive.raw_input_sha256
                    && input.selection == archive.selection
                    && input.guidance == archive.guidance
                    && plan.prior.is_none(),
                "notebook replay source changed"
            ),
            Request::RefineNotebook(input) => ensure!(
                plan.prior
                    .as_ref()
                    .is_some_and(|prior| prior.0 == input.draft),
                "notebook replay draft changed"
            ),
            _ => unreachable!(),
        }
        let history = initial_history(ledger, &plan)?;
        let history_digest = digest(&serde_json::to_vec(&(
            "writer-notebook-history-v1",
            range.map(|r| [r.start, r.end_exclusive]),
            &history,
        ))?);
        let personalization_digest = digest(&serde_json::to_vec(&(
            "writer-notebook-personalization-v1",
            preference.schema_version(),
            preference.effective_text(),
        ))?);
        ensure!(
            history_digest == plan.history_digest
                && personalization_digest == plan.personalization_digest,
            "notebook replay context changed"
        );
        let draft = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:writer:v1:{}:draft", request.operation()).as_bytes(),
        );
        let draft = ledger
            .store()
            .value(Namespace::Conversation, draft)?
            .filter(|e| {
                matches!(Ledger::decode(e),Ok(Record::Turn(ref turn))
            if turn.outcome==Outcome::Generated && turn.exchange==request.operation())
            })
            .map(|_| draft);
        return Ok(Prepared::Replay(Box::new(Replay { plan: *plan, draft })));
    }
    // This snapshot is captured before all related reads, then carried through adoption.
    let expected = ledger.expected(Namespace::Conversation, request.conversation())?;
    let context = ledger.context_range(
        request.conversation(),
        range,
        &ContextBudget {
            max_text_bytes: 16 * 1024,
            max_turns: 256,
            provider_token_limit: Some(16 * 1024),
        },
        |turns| {
            Ok(turns
                .iter()
                .map(|t| t.text.as_ref().map_or(0, String::len))
                .sum())
        },
    )?;
    let user_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:writer:v1:{}:user", request.operation()).as_bytes(),
    );
    let (source, archive, prior) = match request.model() {
        Request::GenerateNotebook(input) => {
            let source = read_document_file(&input.document_file, &input.document_sha256)?;
            let archive = ArchivedNormalizedSource::from_notebook(
                input.conversation,
                user_id,
                &source,
                input.selection.clone(),
                input.guidance.clone(),
            )?;
            (source, archive, None)
        }
        Request::RefineNotebook(input) => {
            let envelope = ledger
                .store()
                .value(Namespace::Conversation, input.draft)?
                .context("notebook prior draft absent")?;
            let Record::Turn(draft) = Ledger::decode(&envelope)? else {
                anyhow::bail!("notebook prior draft absent")
            };
            ensure!(
                draft.conversation == input.conversation
                    && draft.mode == Mode::Writer
                    && draft.role == Role::Assistant
                    && draft.outcome == Outcome::Generated
                    && draft.sources.len() == 1,
                "invalid notebook prior draft"
            );
            let source = ledger
                .store()
                .value(Namespace::Source, draft.sources[0])?
                .context("notebook prior archive absent")?;
            let Record::ArchivedNormalizedSource(archive) = Ledger::decode(&source)? else {
                anyhow::bail!("not a notebook draft")
            };
            let source = archive.load(ledger.store())?;
            let reference = ledger.store().notebook_revision_reference(&envelope)?;
            (source, *archive, Some((draft, reference.sha256, envelope)))
        }
        _ => unreachable!(),
    };
    let payloads = FrozenNotebookPayloads::new(
        &source,
        &archive,
        PayloadContext {
            operation: request.operation(),
            request: text,
            personalization: preference,
            history: &context.turns,
            history_range: range,
            prior: prior
                .as_ref()
                .map(|(turn, revision, _)| (turn, revision.clone())),
            max_model_calls: calls,
        },
    )?;
    let mut guards = Vec::new();
    for turn in &context.turns {
        let envelope = ledger
            .store()
            .value(Namespace::Conversation, turn.id)?
            .context("notebook history turn absent")?;
        ensure!(
            matches!(Ledger::decode(&envelope)?,Record::Turn(ref current) if current==turn),
            "notebook history changed"
        );
        guards.push(envelope);
    }
    if let Some((_, _, envelope)) = &prior {
        guards.push(envelope.clone());
    }
    let user = Turn {
        id: user_id,
        conversation: request.conversation(),
        exchange: request.operation(),
        sequence: 0,
        role: Role::User,
        mode: Mode::Writer,
        outcome: Outcome::Interpreted,
        text: Some(text.into()),
        sources: vec![archive.id],
        correction_of: prior.as_ref().map(|(turn, _, _)| turn.id),
        created_ms: 0,
        updated_ms: 0,
        completion: None,
    };
    let input = runner::Input {
        admission: NotebookAdmission {
            plan: payloads.plan().clone(),
            archive,
            canonical: source.canonical_bytes().to_vec(),
            user,
            expected,
            guards,
        },
        payloads,
    };
    if let Some(identity) = request.joined() {
        flow::preflight(&input, identity)?;
    }
    Ok(Prepared::Fresh(Box::new(input)))
}
fn generation_report(
    request: &Request,
    status: runner::Status,
    failure: Option<runner::Failure>,
) -> runner::Report {
    runner::Report {
        status,
        conversation: request.conversation(),
        operation: request.operation(),
        draft: None,
        failure,
        local_invocations: 0,
        confirmed_stage_results: 0,
        admitted: false,
        unobserved_publication: false,
        remote_cost_unknown: false,
    }
}
fn refused(request: &Request, failure: runner::Failure) -> Report {
    if let Some(identity) = request.joined() {
        Report::Joined(Box::new(flow::Report::refused(
            request.conversation(),
            identity,
            NotebookPlan::identity(request.operation()),
            Uuid::nil(),
            match failure {
                runner::Failure::MissingCredentials => flow::Failure::MissingCredentials,
                runner::Failure::Unsupported => flow::Failure::Unsupported,
                runner::Failure::InvalidInput => flow::Failure::InvalidInput,
                _ => flow::Failure::Storage,
            },
        )))
    } else {
        Report::Generation(generation_report(
            request,
            runner::Status::Refused,
            Some(failure),
        ))
    }
}
pub fn run(config_path: Option<PathBuf>, request: Request) -> Report {
    let result = (|| -> std::result::Result<Report, runner::Failure> {
        request
            .preflight()
            .map_err(|_| runner::Failure::InvalidInput)?;
        let config = Config::load(config_path).map_err(|_| runner::Failure::Storage)?;
        let preference = config
            .personalization_snapshot()
            .map_err(|_| runner::Failure::Storage)?;
        let effective = config::resolve(
            &config,
            config::Overrides::default(),
            config::Environment::current(),
        )
        .map_err(|_| runner::Failure::MissingCredentials)?;
        let mut model = crate::OpenAI::new(
            effective.model,
            effective.key.into_string(),
            effective.base_url,
        );
        let destination = if request.joined().is_some() {
            Some(
                crate::note_export::mem::MemDestination::new(
                    &std::env::var("MEM_API_KEY")
                        .map_err(|_| runner::Failure::MissingCredentials)?,
                )
                .map_err(|_| runner::Failure::MissingCredentials)?,
            )
        } else {
            None
        };
        let (store, prepared) = Store::prepare_notebook_validated(config.paths, |ledger| {
            prepare(ledger, &request, &preference)
        })
        .map_err(|error| match error.downcast_ref::<InputRefusal>() {
            Some(InputRefusal::Unsupported) => runner::Failure::Unsupported,
            Some(InputRefusal::InvalidInput) => runner::Failure::InvalidInput,
            _ => runner::Failure::Storage,
        })?;
        let ledger = Ledger::new(Arc::new(store));
        if let Prepared::Replay(replay) = prepared {
            let mut generation = generation_report(&request, runner::Status::Historical, None);
            generation.draft = replay.draft;
            return Ok(if let Some(identity) = request.joined() {
                let mut report = flow::Report::refused(
                    request.conversation(),
                    identity,
                    replay.plan.id,
                    replay.plan.archive_id,
                    flow::Failure::NoFreshSelection,
                );
                report.status = crate::writer_flow::Status::Historical;
                report.draft = replay.draft;
                report.generation = Some(generation);
                report.failure = None;
                Report::Joined(Box::new(report))
            } else {
                Report::Generation(generation)
            });
        }
        let Prepared::Fresh(input) = prepared else {
            unreachable!()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| runner::Failure::Unsupported)?;
        let identity = request.joined();
        runtime.block_on(async move {
            let (sender,canceled)=tokio::sync::watch::channel(false);
            let mut worker=tokio::task::spawn_blocking(move|| {
                if let (Some(identity),Some(destination))=(identity,destination) {Report::Joined(Box::new(flow::run(&ledger,&mut model,&destination,*input,identity,canceled)))}
                else {Report::Generation(runner::run(&ledger,&mut model,*input,&mut||*canceled.borrow()).report)}
            });
            let joined=tokio::select! {result=&mut worker=>result,_=tokio::signal::ctrl_c()=>{let _=sender.send(true);worker.await}};
            drop(sender);joined.map_err(|_|runner::Failure::Unsupported)
        })
    })();
    result.unwrap_or_else(|failure| refused(&request, failure))
}

#[cfg(test)]
mod tests;
