//! Research composition for an admitted ordinary follow-up.
use super::RequestScope;
use crate::{
    config::{researched_reader::OrdinarySearch, Config},
    conversation::{
        receiving::{
            answer::{self, FollowupAttachments},
            memory::{
                research::{
                    openai::{OpenAIWebResearch, TransportOptions},
                    ResearchDirective, ResearchInput, ResearchOptions, ResearchPolicy,
                },
                CombinedBudget,
            },
            FrozenRequestContext,
        },
        Ledger, SourceObservation,
    },
    llm::{LLMEngine, ModelHandoff},
    memory::LocalMemory,
    storage::{digest, Uuid},
};
use anyhow::{ensure, Context, Result};
use std::time::Instant;

/// The trusted current input owner supplies this decision. Matching bytes
/// verify correspondence and do not classify the question.
pub(crate) struct FollowupResearch<'a> {
    operation: Uuid,
    visit: &'a SourceObservation,
    question_sha256: String,
    directive: ResearchDirective,
    search: Option<OrdinarySearch<'a>>,
    deadline: Instant,
}

impl<'a> FollowupResearch<'a> {
    #[allow(dead_code)]
    pub(crate) fn new(
        scope: &'a RequestScope,
        question: &str,
        directive: ResearchDirective,
        search: Option<OrdinarySearch<'a>>,
        deadline: Instant,
    ) -> Self {
        Self {
            operation: scope.operation,
            visit: &scope.source,
            question_sha256: digest(question.as_bytes()),
            directive,
            search,
            deadline,
        }
    }

    pub(super) fn check(&self, scope: &RequestScope, question: &str) -> Result<()> {
        ensure!(
            self.operation == scope.operation
                && self.visit == &scope.source
                && self.question_sha256 == digest(question.as_bytes()),
            "Reader research intent does not match the current question"
        );
        ensure!(
            Instant::now() < self.deadline,
            "Reader follow-up deadline expired"
        );
        Ok(())
    }
}

fn budget() -> CombinedBudget {
    CombinedBudget {
        max_encoded_bytes: 512 * 1024,
        max_tokens: 512 * 1024,
    }
}

fn once<T>(
    submit: &impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()>,
    stage: impl FnOnce(&mut ModelHandoff<'_>) -> Result<T>,
) -> Result<T> {
    let mut attempted = false;
    let mut accepted = false;
    let mut refused = None;
    let result = stage(&mut |execute| {
        if std::mem::replace(&mut attempted, true) {
            accepted = false;
            anyhow::bail!("Reader follow-up stage already attempted");
        }
        if let Err(error) = submit(execute) {
            refused = Some(error);
            anyhow::bail!("Reader follow-up handoff refused");
        }
        accepted = true;
        Ok(())
    });
    if let Some(error) = refused {
        return Err(error);
    }
    let result = result?;
    ensure!(
        accepted,
        "Reader follow-up stage bypassed guarded submission"
    );
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn answer(
    ledger: &Ledger,
    engine: &mut dyn LLMEngine,
    context: FrozenRequestContext,
    config: &Config,
    memory: Option<&LocalMemory>,
    timestamp: u64,
    policy: ResearchPolicy,
    research: &FollowupResearch<'_>,
    check: &impl Fn() -> Result<()>,
    submit: impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()>,
) -> Result<String> {
    let preference = config.personalization_snapshot()?;
    let count = |text: &str| Ok(text.len());
    check()?;
    let images = FollowupAttachments::acquire(ledger, &context)?;
    let memory = memory
        .map(|provider| {
            Ok::<_, anyhow::Error>((
                provider,
                config.memory.profile.context("memory profile absent")?,
            ))
        })
        .transpose()?;
    let context =
        context.with_followup_memory(memory, ledger.store(), timestamp, budget(), count)?;
    check()?;
    let options = ResearchOptions {
        now_ms: timestamp,
        max_queries: 4,
        max_sources: 8,
        max_evidence_bytes: 16 * 1024,
    };
    let directive = research.directive;
    let deadline = research.deadline;
    let context =
        if directive == ResearchDirective::NoInternet || policy == ResearchPolicy::Disabled {
            context.with_research(
                ResearchInput {
                    policy,
                    directive,
                    provider: None,
                    options,
                },
                budget(),
                count,
            )?
        } else {
            let search = research
                .search
                .as_ref()
                .context("Reader follow-up research unavailable")?;
            let models = search.resolve(config)?;
            let (model, endpoint, credential) = models.required_search()?;
            check()?;
            once(&submit, |handoff| {
                let mut progress = |_| check();
                let provider = OpenAIWebResearch::new(
                    model.into(),
                    search.endpoint(endpoint).into(),
                    credential.into(),
                    TransportOptions::default(),
                    &mut progress,
                )?
                .with_guarded_dispatch(handoff, deadline);
                context.with_research(
                    ResearchInput {
                        policy,
                        directive,
                        provider: Some(&provider),
                        options,
                    },
                    budget(),
                    count,
                )
            })?
        };
    check()?;
    let draft = once(&submit, |handoff| {
        answer::answer_followup_guarded(
            &context,
            engine,
            &images,
            &preference,
            count,
            &mut |_| check(),
            handoff,
            deadline,
        )
    })?;
    check()?;
    answer::render_references(&draft)
}
