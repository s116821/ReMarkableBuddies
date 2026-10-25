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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
struct Document {
    path: PathBuf,
    bytes_sha: String,
    path_sha: String,
    roots_sha: String,
    config: Config,
    // None preserves every released deletion branch; only inventory has five reads.
    config_reads: Option<Arc<AtomicUsize>>,
}
impl Document {
    fn load(path: Option<PathBuf>) -> Result<Self> {
        Self::load_with_reads(path, None)
    }
    fn read_config(path: &Path, reads: Option<&AtomicUsize>) -> Result<Vec<u8>> {
        if let Some(reads) = reads {
            let remaining = reads.load(Ordering::Relaxed);
            let next = remaining
                .checked_sub(1)
                .ok_or_else(|| anyhow::anyhow!("configuration read schedule exhausted"))?;
            // One attempt, no retry/read on contention. Compatible with the MSRV
            // and newer compilers that deprecate AtomicUsize::fetch_update.
            reads
                .compare_exchange(remaining, next, Ordering::Relaxed, Ordering::Relaxed)
                .map_err(|_| anyhow::anyhow!("configuration read schedule exhausted"))?;
        }
        files::read(path, MAX_RECORD as u64)
    }
    fn load_with_reads(
        path: Option<PathBuf>,
        config_reads: Option<Arc<AtomicUsize>>,
    ) -> Result<Self> {
        let path = path.ok_or_else(|| anyhow::anyhow!("configuration unavailable"))?;
        ensure!(path.is_absolute(), "absolute configuration required");
        let text = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("UTF8 path required"))?;
        let path_sha = digest(text.as_bytes());
        let b = Self::read_config(&path, config_reads.as_deref())?;
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
            config_reads,
        })
    }
    fn check(&self) -> Result<()> {
        ensure!(
            digest(&Self::read_config(
                &self.path,
                self.config_reads.as_deref()
            )?) == self.bytes_sha,
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
    #[cfg(test)]
    pub(super) fn inventory_read_budget(&self) -> Option<Arc<AtomicUsize>> {
        self.document.config_reads.clone()
    }
    pub(super) fn check(&mut self) -> std::result::Result<(), Status> {
        self.document.check().map_err(|_| Status::Conflict)?;
        self.session.check().map_err(|_| Status::Conflict)?;
        self.observer.inactive()?;
        self.document.check().map_err(|_| Status::Conflict)?;
        self.session.check().map_err(|_| Status::Conflict)
    }
    pub(super) fn check_inventory(&mut self) -> std::result::Result<(), Status> {
        fn session_status(error: anyhow::Error) -> Status {
            if error
                .downcast_ref::<storage::conversation_cleanup::WorkExhausted>()
                .is_some()
            {
                Status::Unavailable
            } else {
                Status::Conflict
            }
        }
        self.document.check().map_err(|_| Status::Conflict)?;
        self.session.check().map_err(session_status)?;
        self.observer.inactive()?;
        self.document.check().map_err(|_| Status::Conflict)?;
        self.session.check().map_err(session_status)
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
fn existing_custody(
    doc: Document,
    observer: Arc<dyn service::Observer>,
    known: &std::collections::BTreeMap<String, u64>,
) -> std::result::Result<(Custody, String), Status> {
    let session = Session::open(doc.config.paths.clone(), known).map_err(|e| error_status(&e))?;
    let mode = session
        .store
        .sync_mode_snapshot()
        .map_err(|_| Status::Unsupported)?;
    if !matches!(mode.mode, None | Some(SyncMode::LocalOnly)) {
        return Err(Status::Unsupported);
    }
    let mode_sha = serde_json::to_value(&mode.token)
        .ok()
        .and_then(|v| v["sha256"].as_str().map(str::to_owned))
        .ok_or(Status::Unavailable)?;
    let lease = session
        .store
        .ordinary_mode_lease(&mode.token)
        .map_err(|_| Status::Busy)?;
    Ok((
        Custody {
            session,
            _mode: lease,
            document: doc,
            observer,
        },
        mode_sha,
    ))
}

pub(super) fn inventory(
    path: Option<PathBuf>,
    observer: Arc<dyn service::Observer>,
) -> inventory::InventoryReport {
    inventory_controlled(path, observer, |_| {})
}
fn inventory_controlled(
    path: Option<PathBuf>,
    observer: Arc<dyn service::Observer>,
    before_discovery: impl FnOnce(&mut Session),
) -> inventory::InventoryReport {
    use inventory::{InventoryReport, InventoryStatus};
    let doc = match Document::load_with_reads(path, Some(Arc::new(AtomicUsize::new(5)))) {
        Ok(doc) => doc,
        Err(_) => return InventoryReport::refused(InventoryStatus::Unavailable, None),
    };
    if let Err(s) = observer.inactive() {
        return InventoryReport::refused(InventoryStatus::observation(s), None);
    }
    let (mut custody, _) = match existing_custody(doc, observer, &std::collections::BTreeMap::new())
    {
        Ok(c) => c,
        Err(s) => return InventoryReport::refused(InventoryStatus::admission(s), None),
    };
    if let Err(s) = custody.check_inventory() {
        return InventoryReport::refused(InventoryStatus::observation(s), Some(custody));
    }
    before_discovery(&mut custody.session);
    match custody.session.candidate_roots() {
        Ok(ids) => InventoryReport::complete(ids, custody),
        Err(r) => InventoryReport::refused(InventoryStatus::discovery(r), Some(custody)),
    }
}
#[cfg(test)]
pub(super) fn inventory_at_work_limit(
    path: Option<PathBuf>,
    observer: Arc<dyn service::Observer>,
    byte_limit: bool,
) -> inventory::InventoryReport {
    inventory_controlled(path, observer, |session| {
        if byte_limit {
            session.budget.bytes = 512 * 1024 * 1024;
        } else {
            session.budget.visits = 65536;
        }
    })
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
    // Original Config matching/Plan construction still precede Store admission.
    let (mut custody, mode_sha) = match existing_custody(doc, observer, &known) {
        Ok(c) => c,
        Err(s) => {
            report.status = s;
            return report;
        }
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

#[cfg(test)]
mod inventory_read_schedule_tests {
    use super::*;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            if std::thread::panicking() {
                return;
            }
            let path = self.0.canonicalize().unwrap();
            assert!(
                path.starts_with(std::env::temp_dir().canonicalize().unwrap())
                    && path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("buddy-inventory-config-v1-")
            );
            std::fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn inventory_config_read_limit_is_sticky_and_does_not_limit_legacy_documents() {
        let temp = Temp(
            std::env::temp_dir().join(format!("buddy-inventory-config-v1-{}", Uuid::new_v4())),
        );
        std::fs::create_dir(&temp.0).unwrap();
        let paths = storage::StorePaths {
            data: temp.0.join("data"),
            cache: temp.0.join("cache"),
            credentials: temp.0.join("credentials"),
        };
        let mut config = Config {
            paths: paths.clone(),
            ..Config::default()
        };
        config.sync.enabled = false;
        let path = temp.0.join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        drop(storage::Store::open(paths).unwrap());
        let reads = Arc::new(AtomicUsize::new(5));
        let document = Document::load_with_reads(Some(path.clone()), Some(reads.clone())).unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 4);
        for _ in 0..4 {
            document.check().unwrap();
        }
        for _ in 0..2 {
            assert!(document
                .check()
                .unwrap_err()
                .to_string()
                .contains("schedule exhausted"));
            assert_eq!(reads.load(Ordering::Relaxed), 0);
        }
        let legacy = Document::load(Some(path)).unwrap();
        assert!(legacy.config_reads.is_none());
        for _ in 0..8 {
            legacy.check().unwrap();
        }
    }
}
