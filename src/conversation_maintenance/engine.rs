use super::*;
use crate::{
    config::Config,
    storage::{
        self,
        conversation_cleanup::{Plan, Session},
        files,
        sync::mode::{OrdinaryModeLease, SyncMode},
        MAX_RECORD,
    },
};
use std::sync::Arc;
struct Document {
    path: PathBuf,
    bytes_sha: String,
    path_sha: String,
    roots_sha: String,
    config: Config,
}
impl Document {
    fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = path.ok_or_else(|| anyhow::anyhow!("configuration unavailable"))?;
        ensure!(path.is_absolute(), "absolute configuration required");
        let text = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("UTF8 path required"))?;
        let path_sha = digest(text.as_bytes());
        let b = files::read(&path, MAX_RECORD as u64)?;
        let (config, _) =
            inspection::document(&b).map_err(|_| anyhow::anyhow!("configuration unsupported"))?;
        config.validate()?;
        ensure!(!config.sync.enabled, "legacy sync enabled");
        let roots = [
            &config.paths.data,
            &config.paths.cache,
            &config.paths.credentials,
        ]
        .map(|p| {
            p.to_str()
                .ok_or_else(|| anyhow::anyhow!("UTF8 roots required"))
        })
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        let roots_sha = digest(&serde_json::to_vec(&roots)?);
        Ok(Self {
            path,
            bytes_sha: digest(&b),
            path_sha,
            roots_sha,
            config,
        })
    }
    fn check(&self) -> Result<()> {
        ensure!(
            digest(&files::read(&self.path, MAX_RECORD as u64)?) == self.bytes_sha,
            "configuration changed"
        );
        Ok(())
    }
    fn matches(&self, i: &Intent) -> bool {
        i.config_path_sha256 == self.path_sha
            && i.config_bytes_sha256 == self.bytes_sha
            && i.roots_sha256 == self.roots_sha
    }
}
pub(super) struct Custody {
    session: Session,
    _mode: OrdinaryModeLease,
    document: Document,
    observer: Arc<dyn service::Observer>,
}
impl Custody {
    pub(super) fn check(&mut self) -> std::result::Result<(), Status> {
        self.document.check().map_err(|_| Status::Conflict)?;
        self.session.check().map_err(|_| Status::Conflict)?;
        self.observer.inactive()
    }
}
fn error_status(e: &anyhow::Error) -> Status {
    if let Some(r) = e.downcast_ref::<storage::maintenance::Refusal>() {
        return match r {
            storage::maintenance::Refusal::Busy => Status::Busy,
            storage::maintenance::Refusal::Unavailable => Status::Unavailable,
            storage::maintenance::Refusal::Unsupported => Status::Unsupported,
        };
    }
    Status::Conflict
}
pub(super) fn run(
    op: Operation,
    path: Option<PathBuf>,
    request: Request,
    observer: Arc<dyn service::Observer>,
) -> Report {
    let mut report = Report::new(op, Status::Unavailable);
    let doc = match Document::load(path) {
        Ok(d) => d,
        Err(_) => return report,
    };
    if let Err(s) = observer.inactive() {
        report.status = s;
        return report;
    }
    report.service_observation = "inactive";
    let original = match &request {
        Request::Confirm(i) => Some(i.as_ref()),
        _ => None,
    };
    if original.is_some_and(|i| !doc.matches(i)) {
        report.status = Status::Conflict;
        return report;
    }
    let existing_plan = match original.map(Plan::new).transpose() {
        Ok(p) => p,
        Err(_) => {
            report.status = Status::InvalidInput;
            return report;
        }
    };
    let known = existing_plan.as_ref().map(Plan::known).unwrap_or_default();
    let session = match Session::open(doc.config.paths.clone(), &known) {
        Ok(s) => s,
        Err(e) => {
            report.status = error_status(&e);
            return report;
        }
    };
    let mode = match session.store.sync_mode_snapshot() {
        Ok(s) => s,
        Err(_) => {
            report.status = Status::Unsupported;
            return report;
        }
    };
    if !matches!(mode.mode, None | Some(SyncMode::LocalOnly)) {
        report.status = Status::Unsupported;
        return report;
    }
    let mode_sha = match serde_json::to_value(&mode.token)
        .ok()
        .and_then(|v| v["sha256"].as_str().map(str::to_owned))
    {
        Some(s) => s,
        None => return report,
    };
    let lease = match session.store.ordinary_mode_lease(&mode.token) {
        Ok(l) => l,
        Err(_) => {
            report.status = Status::Busy;
            return report;
        }
    };
    let mut custody = Custody {
        session,
        _mode: lease,
        document: doc,
        observer,
    };
    let result = (|| -> Result<()> {
        custody
            .check()
            .map_err(|_| anyhow::anyhow!("custody changed"))?;
        if let Some(i) = original {
            ensure!(
                custody.session.store.actor_id == uuid(&i.actor)?
                    && custody.session.generation()? == uuid(&i.generation)?
                    && mode_sha == i.mode_sha256,
                "original binding changed"
            );
            let plan = existing_plan.as_ref().context("original plan absent")?;
            report.conversation = Some(i.conversation.clone());
            if custody.session.acknowledged(i, plan)? {
                report.status = Status::Acknowledged;
                report.possible_effect = true;
                report.receipt = Some(plan.receipt(i));
                report.current_state = Some(custody.session.current_state(i, plan)?);
                return Ok(());
            }
            let current = custody.session.current_state(i, plan)?;
            report.current_state = Some(current);
            if custody.session.partial(plan) {
                report.status = Status::Unresolved;
                report.possible_effect = true;
                return Ok(());
            }
            let snapshot = custody.session.snapshot(uuid(&i.conversation)?)?;
            if op == Operation::Status {
                report.status = if current == "live-original"
                    && storage::conversation_cleanup::history_hash(&snapshot)?
                        == i.target_history_sha256
                {
                    Status::Unchanged
                } else {
                    Status::Unresolved
                };
                return Ok(());
            }
            ensure!(
                current == "live-original"
                    && storage::conversation_cleanup::history_hash(&snapshot)?
                        == i.target_history_sha256,
                "original target changed"
            );
            let (root, binding, summary) = crate::conversation::cleanup::eligible(
                &snapshot,
                uuid(&i.conversation)?,
                &mut custody.session.budget,
            )?;
            ensure!(
                root == i.root && binding == i.binding && summary == i.summary,
                "target changed"
            );
            custody.session.reserve(i, plan, &snapshot)?;
            custody
                .check()
                .map_err(|_| anyhow::anyhow!("custody changed"))?;
            let mut attempted = false;
            let result = custody.session.publish(i, plan, &mut attempted);
            if let Err(e) = result {
                report.possible_effect = attempted;
                if attempted {
                    report.status = Status::Unknown;
                    return Ok(());
                }
                return Err(e);
            }
            report.status = Status::Acknowledged;
            report.possible_effect = true;
            report.receipt = Some(plan.receipt(i));
            report.current_state = Some("desired-tombstones");
            return Ok(());
        }
        let (operation, conversation) = match &request {
            Request::Inspect(c) => (None, c),
            Request::Prepare(o, c) => (Some(o), c),
            _ => unreachable!(),
        };
        let snapshot = custody.session.snapshot(uuid(conversation)?)?;
        let (root, binding, summary) = crate::conversation::cleanup::eligible(
            &snapshot,
            uuid(conversation)?,
            &mut custody.session.budget,
        )?;
        report.conversation = Some(conversation.clone());
        report.summary = Some(summary.clone());
        report.current_state = Some("live-original");
        if let Some(operation) = operation {
            let mut intent = Intent {
                intent_version: 1,
                operation: operation.clone(),
                conversation: conversation.clone(),
                config_path_sha256: custody.document.path_sha.clone(),
                config_bytes_sha256: custody.document.bytes_sha.clone(),
                roots_sha256: custody.document.roots_sha.clone(),
                actor: custody.session.store.actor_id.to_string(),
                generation: snapshot.generation.to_string(),
                mode_sha256: mode_sha,
                target_history_sha256: storage::conversation_cleanup::history_hash(&snapshot)?,
                root,
                binding,
                summary,
                intent_sha256: String::new(),
            };
            intent.intent_sha256 = intent.hash()?;
            let plan = Plan::new(&intent)?;
            custody.session.reserve(&intent, &plan, &snapshot)?;
            report.intent = Some(intent);
            report.status = Status::Prepared;
        } else {
            report.status = Status::Ready
        }
        Ok(())
    })();
    if let Err(e) = result {
        report.status = error_status(&e);
        report.intent = None;
        report.receipt = None;
        report.summary = None;
        report.current_state = None;
    }
    report.custody = Some(custody);
    report
}
use anyhow::Context;
