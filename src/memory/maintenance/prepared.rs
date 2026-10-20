use super::*;
use crate::memory::{
    self,
    prepared::{self as evidence, Binding, Confirmation, Domain, Intent, Mode, Request},
    Provider,
};
use anyhow::Context;
use storage::{conditional::Refused, Envelope, Namespace};

pub(super) enum Guard {
    LocalPreview {
        store: Arc<Store>,
        intent: Intent,
    },
    LocalReceipt {
        store: Arc<Store>,
        intent: Intent,
        transaction: Uuid,
    },
    Selected {
        provider: Arc<dyn Provider>,
        inspection: memory::Inspection,
    },
}
pub(super) struct Pending {
    guard: Guard,
    sha256: String,
}
impl Pending {
    fn new(guard: Guard, report: &Report) -> anyhow::Result<Self> {
        Ok(Self {
            guard,
            sha256: storage::digest(&serde_json::to_vec(report)?),
        })
    }
    pub(super) fn write(
        &self,
        report: &Report,
        writer: &mut impl std::io::Write,
    ) -> anyhow::Result<()> {
        let bytes = report.encoded()?;
        let mut attempted = false;
        let valid = storage::digest(&serde_json::to_vec(report)?) == self.sha256;
        let result = if !valid {
            Err(anyhow::anyhow!("prepared report changed"))
        } else {
            let mut consume = || {
                attempted = true;
                writer.write_all(&bytes)?;
                Ok(())
            };
            match &self.guard {
                Guard::LocalPreview { store, intent } => store.with_conditional_heads(
                    intent.binding.store_generation,
                    Namespace::SubjectMemory,
                    intent.key.record_id()?,
                    &intent.expected_map(),
                    &mut consume,
                ),
                Guard::LocalReceipt {
                    store,
                    intent,
                    transaction,
                } => store.with_conditional_receipt(
                    intent.binding.store_generation,
                    &intent.envelope(store)?,
                    &intent.expected_map(),
                    *transaction,
                    &mut consume,
                ),
                Guard::Selected {
                    provider,
                    inspection,
                } => inspection.with_current(provider.as_ref(), &mut consume),
            }
        };
        if let Err(error) = result {
            if attempted {
                return Err(error);
            }
            let mut refused = Report::new(report.operation, ReportStatus::Unavailable);
            failure(
                &mut refused,
                ReportStatus::Unavailable,
                true,
                report.operation == Operation::Forget,
            );
            writer.write_all(&refused.encoded()?)?;
        }
        Ok(())
    }
}
fn failure(report: &mut Report, status: ReportStatus, incomplete: bool, attempted: bool) {
    report.status = status;
    report.incomplete = incomplete;
    report.prepared = None;
    report.data = match report.operation {
        Operation::PrepareForget => None,
        Operation::Forget => Some(
            serde_json::json!({"schema_version":1,"outcome":if attempted{"unknown"}else{"not-dispatched"}}),
        ),
        Operation::ForgetReceipt if status == ReportStatus::InvalidInput => None,
        Operation::ForgetReceipt => {
            Some(serde_json::json!({"schema_version":1,"outcome":"unresolved"}))
        }
        _ => None,
    };
}
fn refused(operation: Operation, status: ReportStatus, incomplete: bool) -> Report {
    let mut report = Report::new(operation, status);
    failure(&mut report, status, incomplete, false);
    report
}
fn config_matches(intent: &Intent, config: &Config, path: &Path) -> anyhow::Result<bool> {
    Ok(
        intent.binding.config_sha256 == evidence::config_digest(config, path)?
            && match (&intent.binding.domain, &config.memory.selection) {
                (Domain::Local {}, None) => true,
                (Domain::Selected { identity, .. }, Some(current)) => identity == current,
                _ => false,
            },
    )
}
fn acknowledge(report: &mut Report, intent: &Intent, transaction: Uuid) {
    report.data = Some(
        serde_json::json!({"schema_version":1,"outcome":"acknowledged","operation":intent.operation,"transaction":transaction}),
    );
}
fn local_parents(store: &Store, intent: &Intent) -> anyhow::Result<Vec<Envelope>> {
    let expected = intent.expected_map();
    store.snapshot_revisions_matching(&[Namespace::SubjectMemory], MAX_ITEMS, |e| {
        expected.contains_key(&e.revision_id)
    })
}

pub(super) fn run(
    operation: Operation,
    input: &mut impl Read,
    config_path: Option<PathBuf>,
) -> Report {
    let Some(path) = config_path else {
        return refused(operation, ReportStatus::Unavailable, false);
    };
    if !path.is_absolute() || files::safe_path(&path).is_err() {
        return refused(operation, ReportStatus::Unavailable, false);
    }
    let config = match Config::load(Some(path.clone())) {
        Ok(c) => c,
        Err(_) => return refused(operation, ReportStatus::Unavailable, false),
    };
    let Some(profile) = config.memory.profile else {
        return refused(operation, ReportStatus::Unavailable, false);
    };
    if config.sync.enabled {
        return refused(operation, ReportStatus::Unsupported, false);
    }
    let mut bytes = Vec::new();
    if input
        .take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_RECORD
    {
        return refused(operation, ReportStatus::InvalidInput, false);
    }
    let parsed = if operation == Operation::PrepareForget {
        serde_json::from_slice::<Request>(&bytes).and_then(|request| {
            request
                .validate(profile)
                .map_err(serde::de::Error::custom)?;
            Ok(Parsed::Prepare(request))
        })
    } else {
        serde_json::from_slice::<Confirmation>(&bytes).and_then(|c| {
            c.intent
                .validate(profile)
                .map_err(serde::de::Error::custom)?;
            Ok(Parsed::Confirm(c.intent))
        })
    };
    let parsed = match parsed {
        Ok(p) => p,
        Err(_) => return refused(operation, ReportStatus::InvalidInput, false),
    };
    if let Parsed::Confirm(intent) = &parsed {
        if !config_matches(intent, &config, &path).unwrap_or(false) {
            return refused(
                operation,
                if operation == Operation::ForgetReceipt {
                    ReportStatus::Unavailable
                } else {
                    ReportStatus::Conflict
                },
                operation == Operation::ForgetReceipt,
            );
        }
    }
    if let Err(status) = preflight(&config, false) {
        return refused(operation, status, false);
    }
    let store = match Store::open(config.paths.clone()) {
        Ok(s) => Arc::new(s),
        Err(e) => return refused(operation, store_open_status(&e), false),
    };
    execute(operation, parsed, &path, &config, store)
}
enum Parsed {
    Prepare(Request),
    Confirm(Intent),
}
fn execute(
    operation: Operation,
    parsed: Parsed,
    path: &Path,
    config: &Config,
    store: Arc<Store>,
) -> Report {
    let profile = config.memory.profile.expect("validated memory profile");
    let snapshot = match store.sync_mode_snapshot() {
        Ok(s) => s,
        Err(_) => return refused(operation, ReportStatus::Unsupported, false),
    };
    let token = match &parsed {
        Parsed::Prepare(_) => snapshot.token.clone(),
        Parsed::Confirm(intent) => {
            match storage::sync::mode::SyncModeToken::from_maintenance_request(
                intent.binding.mode.actor,
                intent.binding.mode.sha256.clone(),
            ) {
                Ok(t) => t,
                Err(_) => return refused(operation, ReportStatus::InvalidInput, false),
            }
        }
    };
    if let Parsed::Confirm(intent) = &parsed {
        if intent.binding.actor != store.actor_id {
            return refused(
                operation,
                if operation == Operation::ForgetReceipt {
                    ReportStatus::Unavailable
                } else {
                    ReportStatus::Conflict
                },
                operation == Operation::ForgetReceipt,
            );
        }
    }
    if matches!(&parsed, Parsed::Confirm(_)) && token != snapshot.token {
        return refused(
            operation,
            if operation == Operation::ForgetReceipt {
                ReportStatus::Unavailable
            } else {
                ReportStatus::Conflict
            },
            operation == Operation::ForgetReceipt,
        );
    }
    let mode = match ModeGuard::acquire_token(&store, config, &token) {
        Ok(g) => g,
        Err(_) => return refused(operation, ReportStatus::Unsupported, false),
    };
    let selected = match config
        .memory
        .selection
        .as_ref()
        .map(|identity| {
            memory::selected::SelectedMemory::new(store.clone(), identity.clone()).map(Arc::new)
        })
        .transpose()
    {
        Ok(s) => s,
        Err(_) => return refused(operation, ReportStatus::Unsupported, false),
    };
    let provider: Arc<dyn Provider> = if let Some(s) = &selected {
        s.clone()
    } else {
        match memory::LocalMemory::new_local(store.clone(), profile) {
            Ok(m) => Arc::new(m),
            Err(_) => return refused(operation, ReportStatus::Unsupported, false),
        }
    };
    let mut report = Report::new(operation, ReportStatus::Ok);
    report.mode_guard = Some(mode);
    report.provider_guard = Some(provider.clone());
    let mut attempted = false;
    let result = (|| -> anyhow::Result<Guard> {
        match parsed {
            Parsed::Prepare(request) => {
                let (generation, domain, inspection) = if let Some(selected) = &selected {
                    let (pins, inspection) = selected.prepare_forget_view(&request.key)?;
                    let generation = pins
                        .first()
                        .context("prepared pins absent")?
                        .store_generation;
                    (
                        generation,
                        Domain::Selected {
                            identity: config.memory.selection.clone().expect("selected identity"),
                            pins,
                        },
                        inspection,
                    )
                } else {
                    let (generation, heads) = store
                        .conditional_snapshot(Namespace::SubjectMemory, request.key.record_id()?)?;
                    (
                        generation,
                        Domain::Local {},
                        memory::Inspection::new(heads, None)?,
                    )
                };
                if inspection.envelopes.len() != 1
                    || evidence::supported(&request.key, &inspection.envelopes[0]).is_err()
                {
                    return Err(NotConfirmable.into());
                }
                let intent = Intent {
                    prepared_version: 1,
                    operation: request.operation,
                    key: request.key,
                    expected: evidence::expected(&inspection.envelopes)?,
                    binding: Binding {
                        config_sha256: evidence::config_digest(config, path)?,
                        mode: serde_json::from_value::<Mode>(serde_json::to_value(&token)?)?,
                        actor: store.actor_id,
                        store_generation: generation,
                        domain,
                    },
                };
                intent.validate(profile)?;
                report.data = Some(
                    serde_json::json!({"schema_version":1,"preview":inspection.envelopes[0],"intent":intent}),
                );
                Ok(if selected.is_some() {
                    Guard::Selected {
                        provider: provider.clone(),
                        inspection,
                    }
                } else {
                    Guard::LocalPreview {
                        store: store.clone(),
                        intent,
                    }
                })
            }
            Parsed::Confirm(intent) => {
                if let Some(selected) = &selected {
                    let (transaction, inspection) = selected.prepared_forget(
                        &intent,
                        operation == Operation::ForgetReceipt,
                        &mut attempted,
                    )?;
                    acknowledge(&mut report, &intent, transaction);
                    Ok(Guard::Selected {
                        provider: provider.clone(),
                        inspection,
                    })
                } else {
                    evidence::verify_parents(&intent, &local_parents(&store, &intent)?)
                        .map_err(|_| Refused)?;
                    let envelope = intent.envelope(&store)?;
                    let transaction = if operation == Operation::ForgetReceipt {
                        store
                            .conditional_receipt(
                                intent.binding.store_generation,
                                &envelope,
                                &intent.expected_map(),
                            )?
                            .context("original receipt unresolved")?
                    } else {
                        store.conditional_commit(
                            intent.binding.store_generation,
                            envelope,
                            &intent.expected_map(),
                            &mut attempted,
                        )?
                    };
                    acknowledge(&mut report, &intent, transaction);
                    Ok(Guard::LocalReceipt {
                        store: store.clone(),
                        intent,
                        transaction,
                    })
                }
            }
        }
    })();
    match result {
        Ok(guard) => match Pending::new(guard, &report) {
            Ok(p) => report.prepared = Some(p),
            Err(_) => failure(&mut report, ReportStatus::Unavailable, true, attempted),
        },
        Err(error) => {
            let (status, incomplete) = if attempted || operation == Operation::ForgetReceipt {
                (ReportStatus::Unavailable, true)
            } else if error.is::<NotConfirmable>() {
                (ReportStatus::Unsupported, false)
            } else if error.is::<Refused>() {
                (ReportStatus::Conflict, false)
            } else {
                (ReportStatus::Unavailable, true)
            };
            failure(&mut report, status, incomplete, attempted);
        }
    }
    report
}
#[derive(Debug)]
struct NotConfirmable;
impl std::fmt::Display for NotConfirmable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("memory value not confirmable")
    }
}
impl std::error::Error for NotConfirmable {}
#[cfg(test)]
mod tests {
    use super::*;
    use memory::{
        selected::{InitializeAggregate, InitializeSelection},
        Basis, Fact, Key, Mutation, Provenance, Scope,
    };
    use std::collections::BTreeSet;
    use storage::{Fault, Kind, StorePaths};
    struct Fixture {
        root: PathBuf,
        config: Config,
        store: Arc<Store>,
        fact: Fact,
    }
    impl Fixture {
        fn new(selected: bool) -> Self {
            let root =
                std::env::temp_dir().join(format!("prepared-memory-output-{}", Uuid::new_v4()));
            let profile = Uuid::new_v4();
            let mut config = Config {
                paths: StorePaths {
                    data: root.join("data"),
                    cache: root.join("cache"),
                    credentials: root.join("credentials"),
                },
                memory: memory::Configuration {
                    profile: Some(profile),
                    ..Default::default()
                },
                ..Default::default()
            };
            let store = Arc::new(Store::open(config.paths.clone()).unwrap());
            let fact = Fact {
                key: Key {
                    profile,
                    scope: Scope::Global,
                    fact: Uuid::new_v4(),
                },
                claim: "units".into(),
                value: "SI ÃƒÅ½Ã‚Â±".into(),
                subject: "physics".into(),
                tags: vec![],
                provenance: Provenance {
                    basis: Basis::UserConfirmed,
                    description: "fixture".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 8000,
                observed_ms: 1,
                expires_ms: None,
            };
            if selected {
                let identity = memory::catalog::Identity {
                    group: Uuid::new_v4(),
                    profile,
                    binding_sha256: storage::digest(b"selected prepared fixture"),
                };
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        storage::sync::mode::SyncModeChoice::SharedGroup {
                            group: identity.group,
                            binding_sha256: identity.binding_sha256.clone(),
                            root_id: "fixture".into(),
                        },
                    )
                    .unwrap();
                config.memory.selection = Some(identity.clone());
                let selected =
                    memory::selected::SelectedMemory::new(store.clone(), identity).unwrap();
                let registration = selected
                    .initialize_aggregate(InitializeAggregate {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: storage::digest(b"aggregate"),
                        fact: fact.clone(),
                    })
                    .unwrap()
                    .registration
                    .unwrap();
                selected
                    .initialize_selection(InitializeSelection {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: storage::digest(b"catalog"),
                        registrations: vec![registration],
                    })
                    .unwrap();
            } else {
                memory::LocalMemory::new_local(store.clone(), profile)
                    .unwrap()
                    .put(Mutation {
                        operation: Uuid::new_v4(),
                        expected: BTreeSet::new(),
                        fact: fact.clone(),
                    })
                    .unwrap();
            }
            Self {
                root,
                config,
                store,
                fact,
            }
        }
        fn prepare(&self) -> Report {
            execute(
                Operation::PrepareForget,
                Parsed::Prepare(Request {
                    prepared_version: 1,
                    operation: Uuid::new_v4(),
                    key: self.fact.key.clone(),
                }),
                &self.root.join("config.json"),
                &self.config,
                self.store.clone(),
            )
        }
        fn intent(&self) -> Intent {
            let report = self.prepare();
            assert_eq!(report.status, ReportStatus::Ok);
            let mut out = vec![];
            report.write_to(&mut out).unwrap();
            serde_json::from_value(report.data.as_ref().unwrap()["intent"].clone()).unwrap()
        }
        fn confirm(&self, intent: Intent, receipt: bool) -> Report {
            execute(
                if receipt {
                    Operation::ForgetReceipt
                } else {
                    Operation::Forget
                },
                Parsed::Confirm(intent),
                &self.root.join("config.json"),
                &self.config,
                self.store.clone(),
            )
        }
        fn provider(&self) -> Arc<dyn Provider> {
            memory::configured(self.store.clone(), &self.config.memory).unwrap()
        }
    }
    struct Prefix {
        bytes: Vec<u8>,
        calls: usize,
    }
    impl std::io::Write for Prefix {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                let n = bytes.len().min(9);
                self.bytes.extend_from_slice(&bytes[..n]);
                Ok(n)
            } else {
                Err(std::io::Error::other("prefix failure"))
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn prepared_both_modes_seal_stale_forged_and_prefix_output() {
        for selected in [false, true] {
            let f = Fixture::new(selected);
            let stale = f.prepare();
            assert_eq!(stale.status, ReportStatus::Ok);
            let provider = f.provider();
            let old = provider.inspect(&f.fact.key).unwrap()[0].revision_id;
            let mut fact = f.fact.clone();
            fact.value = "new current value".into();
            provider
                .put(Mutation {
                    operation: Uuid::new_v4(),
                    expected: BTreeSet::from([old]),
                    fact,
                })
                .unwrap();
            let mut out = vec![];
            stale.write_to(&mut out).unwrap();
            let refused: serde_json::Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(refused["status"], "unavailable");
            assert!(refused["data"].is_null());
            drop(stale);
            let mut forged = f.prepare();
            forged.data.as_mut().unwrap()["intent"]["operation"] =
                serde_json::json!(Uuid::new_v4());
            let mut out = vec![];
            forged.write_to(&mut out).unwrap();
            assert!(serde_json::from_slice::<serde_json::Value>(&out).unwrap()["data"].is_null());
            drop(forged);
            let report = f.prepare();
            let before = provider.inspect(&f.fact.key).unwrap();
            let mut writer = Prefix {
                bytes: vec![],
                calls: 0,
            };
            assert!(report.write_to(&mut writer).is_err());
            assert_eq!(writer.calls, 2);
            assert_eq!(writer.bytes.len(), 9);
            assert!(provider.inspect(&f.fact.key).unwrap() == before);
        }
    }
    #[test]
    fn prepared_both_modes_before_after_commit_faults_emit_unknown_then_receipt_only() {
        for selected in [false, true] {
            for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
                let f = Fixture::new(selected);
                let intent = f.intent();
                f.store.set_fault(fault).unwrap();
                let report = f.confirm(intent.clone(), false);
                assert_eq!(report.status, ReportStatus::Unavailable);
                assert!(report.incomplete);
                assert_eq!(report.data.as_ref().unwrap()["outcome"], "unknown");
                let mut out = vec![];
                report.write_to(&mut out).unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&out).unwrap()["data"]["outcome"],
                    "unknown"
                );
                drop(report);
                let before = f.provider().inspect(&f.fact.key).unwrap();
                let receipt = f.confirm(intent, true);
                let mut out = vec![];
                receipt.write_to(&mut out).unwrap();
                let output: serde_json::Value = serde_json::from_slice(&out).unwrap();
                assert_eq!(
                    output["data"]["outcome"],
                    if fault == Fault::AfterCommit {
                        "acknowledged"
                    } else {
                        "unresolved"
                    }
                );
                assert_eq!(
                    before[0].kind,
                    if fault == Fault::AfterCommit {
                        Kind::Tombstone
                    } else {
                        Kind::Value
                    }
                );
                assert!(f.provider().inspect(&f.fact.key).unwrap() == before);
            }
        }
    }
    #[test]
    fn prepared_both_modes_commit_then_prefix_failure_does_not_replay_or_append() {
        for selected in [false, true] {
            let f = Fixture::new(selected);
            let intent = f.intent();
            let report = f.confirm(intent.clone(), false);
            assert_eq!(report.status, ReportStatus::Ok);
            let mut writer = Prefix {
                bytes: vec![],
                calls: 0,
            };
            assert!(report.write_to(&mut writer).is_err());
            assert_eq!(writer.calls, 2);
            assert_eq!(writer.bytes.len(), 9);
            drop(report);
            let before = f.provider().inspect(&f.fact.key).unwrap();
            assert_eq!(before[0].kind, Kind::Tombstone);
            let receipt = f.confirm(intent, true);
            let mut out = vec![];
            receipt.write_to(&mut out).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&out).unwrap()["data"]["outcome"],
                "acknowledged"
            );
            assert!(f.provider().inspect(&f.fact.key).unwrap() == before);
        }
    }
}
