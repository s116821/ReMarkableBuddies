//! Explicit shared notebook command. Saved transactions never grant model access.
use super::{
    command::{self, Request},
    runner::{self, Failure, Report, Status},
    *,
};
use crate::{
    config::{self, Config},
    conversation::{
        notebook::{
            notebook_input_digest,
            selected::{
                context_and_images, initial_snapshot, read_selected_media, NotebookPublication,
                NotebookSelection, NotebookStart, SelectedNotebookCursor,
            },
            NotebookStageKind, MAP_PROMPT, REDUCE_PROMPT,
        },
        ArchivedNormalizedSource, Record, SelectedAdmission, Turn,
    },
    memory::{Provider, QueryResult},
    storage::{
        selection::{SelectionScope, SelectionToken},
        sync::selected::{NotebookPhase, NotebookSelectionChanged, RequestPreferences},
        Store,
    },
    LLMEngine,
};
use anyhow::Context;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    wire_version: u32,
    expected: crate::writer::selected_command::Expected,
    root: crate::conversation::IntentRecordRef,
    refine: Option<crate::writer::selected_command::RefineReferences>,
    notebook: Request,
}

impl Input {
    fn preflight(&self) -> Result<()> {
        self.notebook.preflight()?;
        self.expected.selection.validate()?;
        self.root.validate()?;
        ensure!(
            self.wire_version == 1
                && self.root.namespace == crate::storage::Namespace::Conversation
                && self.root.record_id == self.notebook.conversation()
                && crate::storage::valid_digest(&self.expected.raw_head_sha256)
                && self.notebook.joined().is_none(),
            "invalid selected notebook request"
        );
        match (&self.notebook, &self.refine) {
            (Request::GenerateNotebook(_), None) => Ok(()),
            (Request::RefineNotebook(request), Some(prior)) => {
                prior.source.validate()?;
                prior.draft.validate()?;
                ensure!(
                    request.draft == prior.draft.record_id,
                    "selected notebook draft differs"
                );
                Ok(())
            }
            _ => anyhow::bail!("selected notebook references do not match request"),
        }
    }

    fn scope(&self) -> SelectionScope {
        let expected = &self.expected.selection;
        SelectionScope {
            group: expected.group,
            key_sha256: expected.key_sha256.clone(),
            binding_sha256: expected.binding_sha256.clone(),
        }
    }

    fn selection(&self) -> NotebookSelection {
        NotebookSelection {
            expected: self.expected.selection.clone(),
            raw_head_sha256: self.expected.raw_head_sha256.clone(),
            root: self.root.clone(),
            prior: self
                .refine
                .as_ref()
                .map(|p| (p.source.clone(), p.draft.clone())),
        }
    }
}

struct Original {
    token: SelectionToken,
    source: ValidatedNotebook,
    archive: ArchivedNormalizedSource,
    prior: Option<Turn>,
    history: Vec<Turn>,
    images: crate::conversation::receiving::images::Images,
}

fn original(
    writer: &SelectedAdmission,
    preferences: &RequestPreferences,
    input: &Input,
    selected: &NotebookSelection,
    frozen: Option<ValidatedNotebook>,
) -> Result<Original> {
    let (_, _, range) = input.notebook.fields()?;
    let generated = match (&input.notebook, frozen) {
        (Request::GenerateNotebook(request), Some(source)) => {
            ensure!(
                source.raw_sha256 == request.document_sha256,
                "frozen selected notebook changed"
            );
            Some(source)
        }
        (Request::GenerateNotebook(request), None) => Some(read_document_file(
            &request.document_file,
            &request.document_sha256,
        )?),
        (Request::RefineNotebook(_), None) => None,
        _ => anyhow::bail!("invalid frozen selected notebook source"),
    };
    preferences.with_notebook_phase(
        writer,
        None,
        NotebookPhase::Admission,
        None,
        |store, reads| {
            let (mut snapshot, projection) = initial_snapshot(writer, store, selected, reads)?;
            let (source, archive, prior) = match (&input.notebook, generated, &selected.prior) {
                (Request::GenerateNotebook(request), Some(source), None) => {
                    let archive = ArchivedNormalizedSource::from_notebook(
                        request.conversation,
                        crate::writer::stage(request.operation, "user"),
                        &source,
                        request.selection.clone(),
                        request.guidance.clone(),
                    )?;
                    (source, archive, None)
                }
                (Request::RefineNotebook(_), None, Some((archive, draft))) => {
                    ensure!(
                        projection.current_reference(archive.namespace, archive.record_id)?
                            == *archive
                            && projection.current_reference(draft.namespace, draft.record_id)?
                                == *draft,
                        "selected notebook prior references stale"
                    );
                    let Record::ArchivedNormalizedSource(archive) =
                        projection.exact_reference(archive)?
                    else {
                        anyhow::bail!("selected notebook archive absent");
                    };
                    let Record::Turn(prior) = projection.exact_reference(draft)? else {
                        anyhow::bail!("selected notebook draft absent");
                    };
                    let bytes = read_selected_media(
                        &mut snapshot,
                        &crate::storage::ObjectRef {
                            sha256: archive.media.sha256.clone(),
                            bytes: archive.media.bytes,
                        },
                        reads,
                    )?;
                    (
                        archive.validate_bytes(&bytes)?,
                        (**archive).clone(),
                        Some(prior.clone()),
                    )
                }
                _ => anyhow::bail!("selected notebook input mismatch"),
            };
            ensure!(
                crate::memory::catalog::document_key(archive.document)?
                    == writer.scope().key_sha256,
                "selected notebook document differs"
            );
            let (history, images) = context_and_images(
                &mut snapshot,
                &projection,
                input.notebook.conversation(),
                range,
                reads,
            )?;
            Ok(Original {
                token: snapshot.token,
                source,
                archive,
                prior,
                history,
                images,
            })
        },
    )
}

struct StageRunner<'a, E> {
    writer: &'a SelectedAdmission,
    preferences: &'a RequestPreferences,
    memory: Option<(&'a dyn Provider, &'a QueryResult, &'a SelectionToken)>,
    started: Instant,
    timeout: Duration,
    engine: &'a mut E,
    canceled: &'a mut dyn FnMut() -> bool,
    report: &'a mut Report,
}

impl<E: LLMEngine> StageRunner<'_, E> {
    fn step(
        &mut self,
        cursor: SelectedNotebookCursor,
        kind: NotebookStageKind,
        ordinal: usize,
        parts: &[String],
        images: &[String],
    ) -> Result<Option<SelectedNotebookCursor>> {
        let Self {
            writer,
            preferences,
            memory,
            started,
            timeout,
            engine,
            canceled,
            report,
        } = self;
        if let Some(failure) = runner::final_stop(canceled(), started.elapsed()) {
            report.status = Status::Canceled;
            report.failure = Some(failure);
            return Ok(None);
        }
        let revision = match kind {
            NotebookStageKind::Map => MAP_PROMPT,
            NotebookStageKind::Reduce => REDUCE_PROMPT,
        };
        let input_digest = notebook_input_digest(revision, parts)?;
        let token = cursor.token().clone();
        let stage_started = Instant::now();
        let deadline =
            (*started + Duration::from_secs(600)).min(stage_started + Duration::from_secs(95));
        let Some(prepared) = preferences.with_notebook_phase(
            writer,
            Some(&token),
            NotebookPhase::Publication,
            *memory,
            |store, reads| {
                let mut check = || {
                    ensure!(!canceled(), "selected notebook canceled");
                    preferences.notebook_phase_time(NotebookPhase::Publication, Some(deadline))
                };
                cursor.prepare(
                    writer,
                    store,
                    kind,
                    ordinal,
                    input_digest,
                    NotebookPublication {
                        reads,
                        check: &mut check,
                        unobserved: &mut report.unobserved_publication,
                    },
                )
            },
        )?
        else {
            report.status = Status::Uncertain;
            report.unobserved_publication = true;
            return Ok(None);
        };

        let mut stop = None;
        let mut invoked = false;
        let mut progress = || -> Result<()> {
            if canceled() {
                stop = Some(Failure::Canceled);
                anyhow::bail!("selected notebook canceled");
            }
            if Instant::now() >= deadline {
                stop = Some(Failure::Deadline);
                anyhow::bail!("selected notebook deadline");
            }
            if let Err(error) = preferences.notebook_progress(writer, prepared.token()) {
                stop = Some(if error.is::<NotebookSelectionChanged>() {
                    Failure::Stale
                } else {
                    Failure::Storage
                });
                return Err(error);
            }
            Ok(())
        };
        let response = {
            let content = crate::writer::Content(&mut **engine);
            content.0.clear_content();
            if progress().is_err() {
                None
            } else {
                for part in parts {
                    content.0.add_text_content(part);
                }
                for image in images {
                    content.0.add_image_content(image);
                }
                let mut handoff = |submit: &mut dyn FnMut() -> Result<()>| {
                    ensure!(!invoked, "notebook stage submission already consumed");
                    preferences.with_notebook_phase(
                        writer, Some(prepared.token()), NotebookPhase::Handoff, *memory,
                        |_, _| {
                            ensure!(
                                *timeout <= deadline.saturating_duration_since(Instant::now()),
                                "selected notebook adapter cannot finish within the original deadline"
                            );
                            invoked = true;
                            report.local_invocations += 1;
                            submit()
                        },
                    )
                };
                let response = content.0.execute_with_handoff(&mut progress, &mut handoff);
                let _ = progress();
                Some(if invoked {
                    response
                } else {
                    Err(anyhow::anyhow!("no guarded submission"))
                })
            }
        };
        let (result, status, failure) =
            runner::classify_result(kind, response.as_ref(), stop, invoked, report)?;
        let token = prepared.token().clone();
        let publication = preferences.with_notebook_phase(
            writer,
            Some(&token),
            NotebookPhase::Publication,
            *memory,
            |store, reads| {
                let mut check = || {
                    ensure!(!canceled(), "selected notebook canceled");
                    preferences.notebook_phase_time(NotebookPhase::Publication, Some(deadline))
                };
                prepared.finish(
                    writer,
                    store,
                    result,
                    NotebookPublication {
                        reads,
                        check: &mut check,
                        unobserved: &mut report.unobserved_publication,
                    },
                )
            },
        )?;
        match publication {
            Some((cursor, generated)) => {
                report.confirmed_stage_results += 1;
                if generated {
                    Ok(Some(cursor))
                } else {
                    report.status = status;
                    report.failure = failure;
                    Ok(None)
                }
            }
            None => {
                report.status = Status::Uncertain;
                report.unobserved_publication = true;
                report.remote_cost_unknown |= invoked;
                Ok(None)
            }
        }
    }
}

struct Invocation<'a> {
    writer: &'a SelectedAdmission,
    preferences: &'a RequestPreferences,
    memory: Option<&'a dyn Provider>,
    input: &'a Input,
    selected: &'a NotebookSelection,
    started: Instant,
}

fn run_prepared(
    invocation: Invocation<'_>,
    engine: &mut impl LLMEngine,
    original: Original,
    canceled: &mut dyn FnMut() -> bool,
) -> Report {
    let Invocation {
        writer,
        preferences,
        memory,
        input,
        selected,
        started,
    } = invocation;
    let mut report = command::generation_report(&input.notebook, Status::Refused, None);
    let result = (|| -> Result<()> {
        let Some(_owner) = crate::writer::Owner::acquire(writer.writer_store()) else {
            report.failure = Some(Failure::Busy);
            return Ok(());
        };
        let timeout = engine
            .guarded_timeout()
            .context("selected notebook requires guarded model submission")?;
        ensure!(
            timeout > Duration::ZERO && timeout <= Duration::from_secs(90),
            "notebook model timeout unsupported"
        );
        if let Some(failure) = runner::final_stop(canceled(), started.elapsed()) {
            report.status = Status::Canceled;
            report.failure = Some(failure);
            return Ok(());
        }
        let config = preferences.config();
        let personalization = config.personalization_snapshot()?;
        let (text, calls, range) = input.notebook.fields()?;
        let view = memory
            .map(|provider| {
                provider.require_selected_writer(writer)?;
                let mut query = crate::memory::Query::new(text.to_owned(), crate::writer::now());
                query.document = Some(original.archive.document);
                query.validate()?;
                let view = provider.query_selected_notebook(&query, calls)?;
                ensure!(view.selected(), "notebook memory must be selected");
                Ok::<_, anyhow::Error>(view)
            })
            .transpose()?;
        let memory_part = view
            .as_ref()
            .map(|view| crate::writer::selected::encode_memory(view, original.archive.document))
            .transpose()?
            .flatten();
        let payloads = FrozenNotebookPayloads::new_with_memory_part(
            &original.source,
            &original.archive,
            PayloadContext {
                operation: input.notebook.operation(),
                request: text,
                personalization: &personalization,
                history: &original.history,
                history_range: range,
                prior: original
                    .prior
                    .as_ref()
                    .zip(selected.prior.as_ref())
                    .map(|(draft, (_, reference))| (draft, reference.object.sha256.clone())),
                max_model_calls: calls,
            },
            original.images,
            memory_part,
        )?;
        let memory = memory
            .zip(view.as_ref())
            .map(|(provider, view)| (provider, view, &original.token));
        let Some(cursor) = preferences.with_notebook_phase(
            writer,
            Some(&original.token),
            NotebookPhase::Admission,
            memory,
            |store, reads| {
                let mut check = || {
                    ensure!(!canceled(), "selected notebook canceled");
                    preferences.notebook_phase_time(NotebookPhase::Admission, None)
                };
                SelectedNotebookCursor::begin(
                    writer,
                    store,
                    selected,
                    NotebookStart {
                        plan: payloads.plan().clone(),
                        archive: original.archive,
                        canonical: original.source.canonical_bytes().to_vec(),
                    },
                    NotebookPublication {
                        reads,
                        check: &mut check,
                        unobserved: &mut report.unobserved_publication,
                    },
                )
            },
        )?
        else {
            report.status = Status::Historical;
            return Ok(());
        };
        report.admitted = true;
        let mut runner = StageRunner {
            writer,
            preferences,
            memory,
            started,
            timeout,
            engine,
            canceled,
            report: &mut report,
        };
        let Some(cursor) = runner::run_sequence(
            cursor,
            &payloads,
            SelectedNotebookCursor::map_outputs,
            |cursor, kind, ordinal, parts, images| {
                runner.step(cursor, kind, ordinal, parts, images)
            },
        )?
        else {
            return Ok(());
        };
        if let Some(failure) = runner::final_stop(canceled(), started.elapsed()) {
            report.status = Status::Canceled;
            report.failure = Some(failure);
            return Ok(());
        }
        let token = cursor.token().clone();
        report.draft = preferences.with_notebook_phase(
            writer,
            Some(&token),
            NotebookPhase::Publication,
            memory,
            |store, reads| {
                let mut check = || {
                    ensure!(!canceled(), "selected notebook canceled");
                    preferences.notebook_phase_time(NotebookPhase::Publication, None)
                };
                cursor.finalize(
                    writer,
                    store,
                    NotebookPublication {
                        reads,
                        check: &mut check,
                        unobserved: &mut report.unobserved_publication,
                    },
                )
            },
        )?;
        report.status = if report.draft.is_some() {
            Status::Generated
        } else {
            Status::Uncertain
        };
        report.unobserved_publication = report.draft.is_none();
        Ok(())
    })();
    if let Err(error) = result {
        let stale = error.is::<NotebookSelectionChanged>();
        report.status = if report.unobserved_publication {
            Status::Uncertain
        } else if stale {
            Status::Stale
        } else if report.admitted {
            Status::Failed
        } else {
            Status::Refused
        };
        report.failure = Some(if stale {
            Failure::Stale
        } else {
            Failure::Storage
        });
        report.remote_cost_unknown |= report.local_invocations > 0;
    }
    report
}

fn historical_preparation(writer: &SelectedAdmission, input: &Input) -> Result<bool> {
    use crate::storage::{selection::SelectedReadBudget, Namespace, MAX_ITEMS, MAX_METADATA};
    let mut reads = SelectedReadBudget::new(
        4,
        MAX_ITEMS as u64 * 8,
        MAX_METADATA as u64 * 8,
        crate::storage::MAX_MEDIA * 8,
    );
    let store = writer.writer_store();
    let operation = input.notebook.operation();
    let Some(receipt) =
        store.selected_receipt_with_budget(writer.scope(), operation, &mut reads)?
    else {
        return Ok(false);
    };
    let expected = &input.expected;
    ensure!(
        receipt.mutation == crate::storage::selection::SelectionMutation::Commit
            && receipt.previous_sha256.as_deref() == Some(expected.raw_head_sha256.as_str())
            && receipt.store_generation == expected.selection.store_generation
            && receipt.accepted_base_sha256 == expected.selection.accepted_base_sha256,
        "selected notebook historical preparation differs"
    );
    let read = store
        .selected_snapshot_with_budget(writer.scope(), MAX_ITEMS, &mut reads)?
        .context("selected notebook history absent")?;
    let original = read
        .accepted_history
        .iter()
        .find(|history| history.transaction.operation == operation)
        .context("selected notebook original disappeared")?;
    let predecessor = read
        .accepted_history
        .iter()
        .find(|history| history.sha256 == expected.raw_head_sha256)
        .context("selected notebook predecessor absent")?;
    ensure!(
        predecessor.transaction.aggregate_generation == expected.selection.aggregate_generation
            && digest(&serde_json::to_vec(&predecessor.transaction)?)
                == expected.selection.selection_sha256,
        "selected notebook original selection differs"
    );
    let predecessor = crate::conversation::SelectedDomainProjection::from_mixed_records(
        &predecessor.selected_records,
        &predecessor.transaction.selected.records,
        writer.scope(),
    )?;
    ensure!(
        predecessor.current_reference(Namespace::Conversation, input.notebook.conversation())?
            == input.root,
        "selected notebook historical root differs"
    );
    let projection = crate::conversation::SelectedDomainProjection::from_mixed_records(
        &original.selected_records,
        &original.transaction.selected.records,
        writer.scope(),
    )?;
    let id = crate::conversation::notebook::NotebookPlan::identity(operation);
    let Some((_, Some(Record::NotebookPlan(plan)))) = projection.head(Namespace::Conversation, id)
    else {
        anyhow::bail!("original selected notebook plan absent");
    };
    let (text, calls, range) = input.notebook.fields()?;
    let history: Vec<_> = predecessor
        .inspect(input.notebook.conversation())?
        .into_iter()
        .filter_map(|record| match record {
            Record::Turn(turn)
                if matches!(
                    turn.outcome,
                    crate::conversation::Outcome::Interpreted
                        | crate::conversation::Outcome::Completed
                ) && range.is_none_or(|range| {
                    turn.sequence >= range.start && turn.sequence < range.end_exclusive
                }) =>
            {
                Some(turn)
            }
            _ => None,
        })
        .collect();
    let history_digest = digest(&serde_json::to_vec(&(
        "writer-notebook-history-v1",
        range.map(|range| [range.start, range.end_exclusive]),
        &history,
    ))?);
    ensure!(
        plan.operation == operation
            && plan.conversation == input.notebook.conversation()
            && plan.request == text
            && plan.caller_max_model_calls == calls
            && plan.history_digest == history_digest,
        "original selected notebook request differs"
    );
    let Some((_, Some(Record::ArchivedNormalizedSource(archive)))) =
        projection.head(Namespace::Source, plan.archive_id)
    else {
        anyhow::bail!("original selected notebook archive absent");
    };
    match (&input.notebook, &input.refine) {
        (Request::GenerateNotebook(request), None) => ensure!(
            plan.prior.is_none()
                && archive.raw_input_sha256 == request.document_sha256
                && archive.selection == request.selection
                && archive.guidance == request.guidance,
            "original selected notebook source differs"
        ),
        (Request::RefineNotebook(_), Some(prior)) => ensure!(
            predecessor.current_reference(Namespace::Source, archive.id)? == prior.source
                && predecessor.current_reference(Namespace::Conversation, prior.draft.record_id)?
                    == prior.draft
                && plan
                    .prior
                    .as_ref()
                    .is_some_and(|(id, sha, _)| *id == prior.draft.record_id
                        && *sha == prior.draft.object.sha256),
            "original selected notebook refinement differs"
        ),
        _ => anyhow::bail!("original selected notebook variant differs"),
    }
    Ok(true)
}

pub(crate) fn run(config_path: Option<PathBuf>, input: Input) -> Report {
    let started = Instant::now();
    let report_request = input.notebook.clone();
    let refused =
        |failure| command::generation_report(&report_request, Status::Refused, Some(failure));
    if input.preflight().is_err() {
        return refused(Failure::InvalidInput);
    }
    let result = (|| -> std::result::Result<Report, Failure> {
        let config = Config::load(config_path.clone()).map_err(|_| Failure::Storage)?;
        ensure_selected_config(&config).map_err(|_| Failure::Unsupported)?;
        crate::memory::maintenance::preflight(&config, false).map_err(|_| Failure::Storage)?;
        let store = Arc::new(
            Store::open_existing_for_mode(config.paths.clone()).map_err(|_| Failure::Storage)?,
        );
        if store
            .ordinary_reader_family()
            .map_err(|_| Failure::Storage)?
        {
            return Err(Failure::Unsupported);
        }
        let writer =
            SelectedAdmission::new(store.clone(), input.scope()).map_err(|_| Failure::Storage)?;
        if historical_preparation(&writer, &input).map_err(|_| Failure::Storage)? {
            return Ok(command::generation_report(
                &input.notebook,
                Status::Historical,
                None,
            ));
        }
        let selected = input.selection();
        let (_, calls, _) = input.notebook.fields().map_err(|_| Failure::InvalidInput)?;
        let preferences = RequestPreferences::open_notebook(
            store.clone(),
            config_path
                .unwrap_or_else(|| "/home/root/.config/remarkable-buddies/config.json".into()),
            started,
            calls,
        )
        .map_err(|_| Failure::Storage)?;
        let config = preferences.config();
        ensure_selected_config(config).map_err(|_| Failure::Unsupported)?;
        if !config.admits_writer(false) {
            return Err(Failure::Unsupported);
        }
        let original = original(&writer, &preferences, &input, &selected, None)
            .map_err(|_| Failure::InvalidInput)?;
        let memory = if config.memory.enabled {
            Some(crate::memory::configured(store, &config.memory).map_err(|_| Failure::Storage)?)
        } else {
            None
        };
        let effective = config::resolve(
            config,
            config::Overrides::default(),
            config::Environment::current(),
        )
        .map_err(|_| Failure::MissingCredentials)?;
        let mut engine = crate::OpenAI::new(
            effective.model,
            effective.key.into_string(),
            effective.base_url,
        )
        .with_single_request();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Failure::Storage)?;
        let canceled = Arc::new(AtomicBool::new(false));
        let stop = canceled.clone();
        runtime
            .block_on(async move {
                let mut worker = tokio::task::spawn_blocking(move || {
                    run_prepared(
                        Invocation {
                            writer: &writer,
                            preferences: &preferences,
                            memory: memory.as_deref(),
                            input: &input,
                            selected: &selected,
                            started,
                        },
                        &mut engine,
                        original,
                        &mut || stop.load(Ordering::Acquire),
                    )
                });
                tokio::select! {
                    report = &mut worker => report,
                    _ = tokio::signal::ctrl_c() => {
                        canceled.store(true, Ordering::Release);
                        worker.await
                    }
                }
            })
            .map_err(|_| Failure::Storage)
    })();
    result.unwrap_or_else(refused)
}

#[cfg(test)]
pub(crate) fn run_fixture(
    writer: &SelectedAdmission,
    preferences: &RequestPreferences,
    memory: Option<&dyn Provider>,
    engine: &mut impl LLMEngine,
    request: serde_json::Value,
    frozen: Option<ValidatedNotebook>,
    started: Instant,
) -> Result<Report> {
    run_fixture_canceled(
        writer,
        preferences,
        memory,
        engine,
        request,
        frozen,
        started,
        &mut || false,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_fixture_canceled(
    writer: &SelectedAdmission,
    preferences: &RequestPreferences,
    memory: Option<&dyn Provider>,
    engine: &mut impl LLMEngine,
    request: serde_json::Value,
    frozen: Option<ValidatedNotebook>,
    started: Instant,
    canceled: &mut dyn FnMut() -> bool,
) -> Result<Report> {
    let input: Input = serde_json::from_value(request)?;
    input.preflight()?;
    let selected = input.selection();
    let original = original(writer, preferences, &input, &selected, frozen)?;
    Ok(run_prepared(
        Invocation {
            writer,
            preferences,
            memory,
            input: &input,
            selected: &selected,
            started,
        },
        engine,
        original,
        canceled,
    ))
}

fn ensure_selected_config(config: &Config) -> Result<()> {
    ensure!(
        !config.sync.enabled,
        "legacy sync conflicts with selected notebook"
    );
    Ok(())
}
