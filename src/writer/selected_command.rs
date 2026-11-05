//! Explicit selected Writer command adapter; evidence is never a serialized permit.
use super::{selected, Request, Status as WriterStatus};
use crate::{
    config::{self, Config},
    conversation::*,
    storage::{
        self, digest, files,
        selection::{SelectedReadBudget, SelectionScope, SelectionToken},
        sync::mode::{SyncMode, SyncModeLease},
        Store, Uuid, MAX_ITEMS, MAX_METADATA,
    },
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

const MAX_INPUT: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Inspect,
    Generate,
    Refine,
    Recover,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Inspect => "writer-inspect",
            Self::Generate => "writer-generate",
            Self::Refine => "writer-refine",
            Self::Recover => "writer-recover",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub selection: IntentSelectionEvidence,
    pub raw_head_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefineReferences {
    pub source: IntentRecordRef,
    pub draft: IntentRecordRef,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub wire_version: u32,
    pub expected: Expected,
    pub root: IntentRecordRef,
    pub refine: Option<RefineReferences>,
    pub request: Request,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    wire_version: u32,
    scope: SelectionScope,
    conversation: Uuid,
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: &'static str,
    pub status: &'static str,
    native_authority: bool,
    pub data: Option<serde_json::Value>,
    #[serde(skip)]
    guard: Option<Guard>,
    #[serde(skip)]
    mode: Option<SyncModeLease>,
}
struct Guard {
    writer: SelectedAdmission,
    token: SelectionToken,
    raw_head: String,
    sha256: String,
    consumed: bool,
}
impl Report {
    fn new(op: Operation, status: &'static str) -> Self {
        Self {
            report_version: 1,
            operation: op.name(),
            status,
            native_authority: false,
            data: None,
            guard: None,
            mode: None,
        }
    }
    fn encoded(&self) -> Result<Vec<u8>> {
        let mut b = serde_json::to_vec(self)?;
        ensure!(b.len() < MAX_METADATA + 65536, "Writer report bound");
        b.push(b'\n');
        Ok(b)
    }
    pub fn write_to(&mut self, out: &mut impl Write) -> Result<()> {
        let bytes = self.encoded()?;
        let actual = digest(&serde_json::to_vec(self)?);
        let Some(guard) = self.guard.as_mut() else {
            return Ok(out.write_all(&bytes)?);
        };
        ensure!(
            !guard.consumed && guard.sha256 == actual,
            "Writer report changed or consumed"
        );
        guard.consumed = true;
        let mut attempted = false;
        let result = SelectedAdmission::with_gates(&[&guard.writer], |store| {
            self.mode
                .as_ref()
                .context("Writer report mode lease absent")?
                .require_shared_scope(guard.token.scope())?;
            let mut budget = allowance();
            let read = store
                .selected_snapshot_with_budget(guard.token.scope(), MAX_ITEMS, &mut budget)?
                .context("Writer emission selection absent")?;
            ensure!(
                read.snapshot.token == guard.token
                    && read.accepted_history[0].sha256 == guard.raw_head,
                "Writer emission replaced"
            );
            attempted = true;
            Ok(out.write_all(&bytes)?)
        });
        if let Err(e) = result {
            if attempted {
                return Err(e);
            }
            self.status = "historical";
            if let Some(data) = self.data.as_mut() {
                data["handles"] = serde_json::Value::Null;
                data["disposition"] = serde_json::json!("historical");
            }
            out.write_all(&self.encoded()?)?;
        }
        Ok(())
    }
}
fn allowance() -> SelectedReadBudget {
    SelectedReadBudget::new(64, 4096, 8 * 1024 * 1024, 32 * 1024 * 1024)
}
fn scope(e: &IntentSelectionEvidence) -> SelectionScope {
    SelectionScope {
        group: e.group,
        key_sha256: e.key_sha256.clone(),
        binding_sha256: e.binding_sha256.clone(),
    }
}
fn prepare(i: &Input, op: Operation) -> Result<SelectedWriterPrepare> {
    ensure!(i.wire_version == 1, "Writer wire version");
    i.expected.selection.validate()?;
    ensure!(
        storage::valid_digest(&i.expected.raw_head_sha256),
        "Writer raw head"
    );
    i.root.validate()?;
    i.request
        .preflight()
        .map_err(|_| anyhow::anyhow!("invalid Writer request"))?;
    ensure!(
        i.root.namespace == storage::Namespace::Conversation
            && i.root.record_id == i.request.conversation(),
        "Writer root identity"
    );
    let source = match &i.request {
        Request::Generate {
            document,
            selection,
            guidance,
            ..
        } => {
            ensure!(
                op != Operation::Refine && i.refine.is_none(),
                "Writer Generate selector/refine mismatch"
            );
            SelectedWriterSource::Generate {
                normalized: NormalizedTextSource::new(
                    i.request.conversation(),
                    super::stage(i.request.operation(), "user"),
                    document.clone(),
                    selection.clone(),
                    guidance.clone(),
                )?,
            }
        }
        Request::Refine { draft, .. } => {
            ensure!(op != Operation::Generate, "Writer Refine selector mismatch");
            let r = i
                .refine
                .as_ref()
                .context("Writer Refine references absent")?;
            r.source.validate()?;
            r.draft.validate()?;
            ensure!(r.draft.record_id == *draft, "Writer Refine draft identity");
            SelectedWriterSource::Refine {
                source: r.source.clone(),
                draft: r.draft.clone(),
            }
        }
        _ => anyhow::bail!("selected export unavailable"),
    };
    let request = match &i.request {
        Request::Generate { request, .. } | Request::Refine { request, .. } => request.clone(),
        _ => unreachable!(),
    };
    Ok(SelectedWriterPrepare {
        operation: i.request.operation(),
        root: i.root.clone(),
        request,
        source,
    })
}
fn shared(store: &Arc<Store>, scope: &SelectionScope) -> Result<SyncModeLease> {
    let mode = store.sync_mode_snapshot()?;
    ensure!(
        matches!(&mode.mode,Some(SyncMode::SharedGroup{group,binding_sha256,..})if *group==scope.group && *binding_sha256==scope.binding_sha256),
        "saved Writer shared mode/binding required"
    );
    let lease = store.sync_mode_lease(&mode.token)?;
    lease.require_shared_scope(scope)?;
    Ok(lease)
}
fn attach_current(
    report: &mut Report,
    writer: SelectedAdmission,
    token: SelectionToken,
    conversation: Uuid,
) -> Result<()> {
    let handles = SelectedAdmission::with_gates(&[&writer], |store| {
        let mut budget = allowance();
        let read = store
            .selected_snapshot_with_budget(token.scope(), MAX_ITEMS, &mut budget)?
            .context("Writer current handles absent")?;
        ensure!(
            read.snapshot.token == token,
            "Writer result already historical"
        );
        let p = SelectedDomainProjection::from_snapshot(&read.snapshot)?;
        let records = p.inspect(conversation)?;
        let mut sources = Vec::new();
        let mut drafts = Vec::new();
        for record in records {
            match record {
                Record::NormalizedText(s) => {
                    sources.push(p.current_reference(storage::Namespace::Source, s.id)?)
                }
                Record::Turn(t)
                    if t.role == Role::Assistant
                        && t.mode == Mode::Writer
                        && t.outcome == Outcome::Generated =>
                {
                    drafts.push(p.current_reference(storage::Namespace::Conversation, t.id)?)
                }
                _ => {}
            }
        }
        Ok((
            serde_json::json!({"expected":Expected{selection:IntentSelectionEvidence::from_token(&token),raw_head_sha256:read.accepted_history[0].sha256.clone()},"root":p.current_reference(storage::Namespace::Conversation,conversation)?,"sources":sources,"drafts":drafts}),
            read.accepted_history[0].sha256.clone(),
        ))
    })?;
    report.data.as_mut().context("Writer report data absent")?["handles"] = handles.0;
    let hash = digest(&serde_json::to_vec(&report)?);
    report.guard = Some(Guard {
        writer,
        token,
        raw_head: handles.1,
        sha256: hash,
        consumed: false,
    });
    Ok(())
}
fn historical(op: Operation, history: SelectedWriterPreparation) -> Report {
    let mut report = Report::new(op, "historical-pending");
    let SelectedWriterPreparation::Historical {
        transaction,
        finished,
    } = history
    else {
        unreachable!("evidence recovery cannot mint Fresh")
    };
    report.data = Some(
        serde_json::json!({"preparation":transaction,"handles":null,"disposition":"historical"}),
    );
    if let Some(finished) = finished {
        let SelectedWriterPublication::Historical {
            transaction,
            acknowledgment,
        } = *finished
        else {
            unreachable!()
        };
        report.status = "historical";
        report.data = Some(
            serde_json::json!({"transaction":transaction,"receipt":acknowledgment.receipt,"draft":acknowledgment.written,"handles":null,"disposition":"historical"}),
        );
    }
    report
}
pub fn run(
    op: Operation,
    path: &Path,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> Report {
    let decoded = (|| -> Result<Vec<u8>> {
        if path == Path::new("-") {
            let mut bytes = Vec::new();
            stdin.take(MAX_INPUT as u64 + 1).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= MAX_INPUT, "Writer input bound");
            Ok(bytes)
        } else {
            files::read(path, MAX_INPUT as u64)
        }
    })();
    let Ok(bytes) = decoded else {
        return Report::new(op, "invalid-input");
    };
    let inspect = if op == Operation::Inspect {
        match serde_json::from_slice::<Inspect>(&bytes) {
            Ok(i)
                if i.wire_version == 1
                    && !i.conversation.is_nil()
                    && i.scope.validate().is_ok() =>
            {
                Some(i)
            }
            _ => return Report::new(op, "invalid-input"),
        }
    } else {
        None
    };
    let input = if inspect.is_none() {
        match serde_json::from_slice::<Input>(&bytes) {
            Ok(i) if prepare(&i, op).is_ok() => Some(i),
            _ => return Report::new(op, "invalid-input"),
        }
    } else {
        None
    };
    let result = (|| -> Result<Report> {
        let config = Config::load(config_path)?;
        ensure!(
            config.admits_writer(false) && !config.sync.enabled,
            "Writer configuration unsupported"
        );
        crate::memory::maintenance::preflight(&config, false)
            .map_err(|_| anyhow::anyhow!("Writer Store unavailable"))?;
        let store = Arc::new(Store::open(config.paths.clone())?);
        let selected_scope = inspect
            .as_ref()
            .map(|i| i.scope.clone())
            .unwrap_or_else(|| scope(&input.as_ref().unwrap().expected.selection));
        let mode = shared(&store, &selected_scope)?;
        let writer = SelectedAdmission::new(store.clone(), selected_scope)?;
        if let Some(i) = inspect {
            let read = SelectedAdmission::with_gates(&[&writer], |s| {
                let mut b = allowance();
                s.selected_snapshot_with_budget(writer.scope(), MAX_ITEMS, &mut b)?
                    .context("Writer selection absent")
            })?;
            let p = SelectedDomainProjection::from_snapshot(&read.snapshot)?;
            let records = p.inspect(i.conversation)?;
            let mut report = Report::new(op, "complete");
            report.data = Some(
                serde_json::json!({"conversation":i.conversation,"records":records,"disposition":"current"}),
            );
            report.mode = Some(mode);
            attach_current(&mut report, writer, read.snapshot.token, i.conversation)?;
            return Ok(report);
        }
        let i = input.unwrap();
        let req = prepare(&i, op)?;
        if let Some(history) = writer.recover_selected_writer_evidence(
            &i.expected.selection,
            &i.expected.raw_head_sha256,
            &req,
        )? {
            let mut report = historical(op, history);
            report.mode = Some(mode);
            return Ok(report);
        }
        if op == Operation::Recover {
            let mut report = Report::new(op, "not-found");
            report.mode = Some(mode);
            return Ok(report);
        }
        let accepted = SelectedAdmission::with_gates(&[&writer], |s| {
            let mut b = allowance();
            let read = s
                .selected_snapshot_with_budget(writer.scope(), MAX_ITEMS, &mut b)?
                .context("Writer selection absent")?;
            ensure!(
                IntentSelectionEvidence::from_token(&read.snapshot.token) == i.expected.selection
                    && read.accepted_history[0].sha256 == i.expected.raw_head_sha256,
                "Writer expected selection differs"
            );
            let p = SelectedDomainProjection::from_snapshot(&read.snapshot)?;
            p.exact_reference(&i.root)?;
            Ok(read.snapshot.token)
        })?;
        writer.selected_writer_input(&accepted, &req)?;
        let memory = if config.memory.enabled {
            ensure!(req.request.len() <= 2048, "Writer memory query bound");
            let m = crate::memory::configured(store, &config.memory)?;
            m.require_selected_writer(&writer)?;
            Some(m)
        } else {
            None
        };
        let effective = config::resolve(
            &config,
            config::Overrides::default(),
            config::Environment::current(),
        )
        .map_err(|_| anyhow::anyhow!("Writer credentials unavailable"))?;
        let preference = config.personalization_snapshot()?;
        let mut engine = crate::OpenAI::new(
            effective.model,
            effective.key.into_string(),
            effective.base_url,
        );
        let selection = selected::Selection {
            accepted,
            root: i.root.clone(),
            refine: i.refine.map(|r| (r.source, r.draft)),
        };
        let flag = Arc::new(AtomicBool::new(false));
        let canceled = flag.clone();
        let worker_writer = writer.clone();
        let conversation = i.request.conversation();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let received=runtime.block_on(async move{let mut worker=tokio::task::spawn_blocking(move||selected::run(&worker_writer,&selection,&mut engine,&i.request,&preference,memory.as_deref(),&mut||canceled.load(Ordering::Acquire)));tokio::select!{r=&mut worker=>r,_=tokio::signal::ctrl_c()=>{flag.store(true,Ordering::Release);worker.await}}})?;
        let mut report = Report::new(
            op,
            match received.report.status {
                WriterStatus::Generated => "generated",
                WriterStatus::HistoricalGenerated => "historical",
                WriterStatus::Failed => "failed",
                WriterStatus::Canceled => "canceled",
                WriterStatus::HistoricalPending => "historical-pending",
                WriterStatus::Busy => "busy",
                _ => "refused",
            },
        );
        let mut current = None;
        let data = match received.publication {
            Some(SelectedWriterPublication::Published {
                publication,
                acknowledgment,
            }) => {
                current = Some(publication.token);
                serde_json::json!({"report":received.report,"transaction":publication.transaction,"receipt":acknowledgment.receipt,"draft":acknowledgment.written,"handles":null,"disposition":"published"})
            }
            Some(SelectedWriterPublication::Historical {
                transaction,
                acknowledgment,
            }) => {
                report.status = "historical";
                serde_json::json!({"report":received.report,"transaction":transaction,"receipt":acknowledgment.receipt,"draft":acknowledgment.written,"handles":null,"disposition":"historical"})
            }
            None => {
                serde_json::json!({"report":received.report,"preparation":received.historical_preparation,"handles":null})
            }
        };
        report.data = Some(data);
        report.mode = Some(mode);
        if let Some(token) = current {
            if attach_current(&mut report, writer, token, conversation).is_err() {
                report.status = "historical";
                report.data.as_mut().unwrap()["disposition"] = serde_json::json!("historical");
            }
        }
        Ok(report)
    })();
    result.unwrap_or_else(|_| Report::new(op, "unavailable"))
}

#[cfg(test)]
mod tests;
