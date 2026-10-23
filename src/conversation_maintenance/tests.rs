use super::*;
#[test]
fn frozen_original_vectors_and_closed_wire() {
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/conversation_cleanup_digest_vectors.json"
    ))
    .unwrap();
    for row in v["cases"].as_array().unwrap() {
        let mut raw = row["original_fields"].clone();
        raw["intent_sha256"] = row["intent_sha256"].clone();
        let i: Intent = serde_json::from_value(raw).unwrap();
        i.validate().unwrap();
        assert_eq!(i.hash().unwrap(), row["intent_sha256"]);
        for (role, id) in row["derived_uuids"].as_object().unwrap() {
            assert_eq!(i.derived(role).to_string(), id.as_str().unwrap());
        }
        let p = crate::storage::conversation_cleanup::Plan::new(&i).unwrap();
        assert_eq!(
            p.records.last().unwrap().record_id.to_string(),
            row["receipt_record_id"].as_str().unwrap()
        );
        assert_eq!(
            p.records.last().unwrap().payload["record"]["fingerprint"],
            row["receipt_fingerprint"]
        );
    }
    for b in [
        b"[]".as_slice(),
        b"{}",
        b"{} {}",
        b"{\"request_version\":1,\"request_version\":1}",
        b"{\"intent\":[]}",
        &[255],
    ] {
        for op in [
            Operation::Inspect,
            Operation::Prepare,
            Operation::Apply,
            Operation::Status,
        ] {
            assert!(parse(op, b).is_err());
        }
    }
}
#[test]
fn refusal_encoding_and_no_input_platform() {
    struct NoRead;
    impl Read for NoRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("early boundary read")
        }
    }
    for op in [
        Operation::Inspect,
        Operation::Prepare,
        Operation::Apply,
        Operation::Status,
    ] {
        let r = run(op, Path::new("-/"), None, &mut NoRead);
        let mut b = Vec::new();
        r.write_to(&mut b).unwrap();
        assert!(b.len() < MAX_OUTPUT);
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["status"], "unsupported");
        assert_eq!(v["physical_reclamation"], false);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            run(op, Path::new("-"), None, &mut NoRead).status,
            Status::Unsupported
        );
    }
}

use crate::{
    config::Config,
    conversation::*,
    storage::{
        self, Envelope, Fault, Kind, Manifest, Namespace, ObjectRef, Scope, Store, StorePaths,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::Arc,
};
mod inventory_controls {
    use super::*;
    include!("inventory_tests.rs");
}
struct Inactive;
impl service::Observer for Inactive {
    fn inactive(&self) -> std::result::Result<(), Status> {
        Ok(())
    }
}
struct Fixture {
    root: PathBuf,
    config: PathBuf,
    paths: StorePaths,
    conversation: Uuid,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "buddy-conversation-maintenance-v1-{}",
            Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let mut config = Config {
            paths: paths.clone(),
            ..Config::default()
        };
        config.sync.enabled = false;
        let path = root.join("config.json");
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let store = Arc::new(Store::open(paths.clone()).unwrap());
        let ledger = Ledger::new(store);
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        drop(ledger);
        Self {
            root,
            config: path,
            paths,
            conversation,
        }
    }
    fn call(&self, op: Operation, request: Request) -> Report {
        engine::run(op, Some(self.config.clone()), request, Arc::new(Inactive))
    }
    fn prepared(&self) -> Intent {
        let report = self.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), self.conversation.to_string()),
        );
        assert_eq!(
            report.status,
            Status::Prepared,
            "{}",
            serde_json::to_string(&report).unwrap()
        );
        report.intent.as_ref().unwrap().clone()
    }
    fn confirm(&self, op: Operation, i: &Intent) -> Report {
        self.call(op, Request::Confirm(Box::new(i.clone())))
    }
    fn state(&self) -> BTreeMap<PathBuf, (Vec<u8>, Option<std::time::SystemTime>)> {
        fn scan(
            root: &Path,
            out: &mut BTreeMap<PathBuf, (Vec<u8>, Option<std::time::SystemTime>)>,
        ) {
            for e in fs::read_dir(root).unwrap() {
                let e = e.unwrap();
                if e.file_type().unwrap().is_dir() {
                    scan(&e.path(), out)
                } else {
                    out.insert(
                        e.path(),
                        (
                            fs::read(e.path()).unwrap(),
                            e.metadata().unwrap().modified().ok(),
                        ),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        scan(&self.root, &mut out);
        out
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let target = self.root.canonicalize().unwrap();
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        assert!(
            target.starts_with(tmp)
                && target
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("buddy-conversation-maintenance-v1-")
        );
        fs::remove_dir_all(target).unwrap();
    }
}
fn observation() -> SourceObservation {
    SourceObservation {
        document: Uuid::new_v4(),
        page: Uuid::new_v4(),
        session: "synthetic-session".into(),
        visit: "synthetic-visit".into(),
        content_revision: Some("r1".into()),
        order_revision: Some("o1".into()),
        capability_revision: "fixture-v1".into(),
        evidence_procedure: "synthetic".into(),
    }
}
fn user(conversation: Uuid, exchange: Uuid) -> Turn {
    Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange,
        sequence: 0,
        role: Role::User,
        mode: Mode::Reader,
        outcome: Outcome::Interpreted,
        text: Some("synthetic request sentinel".into()),
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    }
}
fn append_pair(f: &Fixture, complete: bool) {
    let store = Arc::new(Store::open(f.paths.clone()).unwrap());
    let ledger = Ledger::new(store);
    let exchange = Uuid::new_v4();
    let u = user(f.conversation, exchange);
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            u,
            vec![],
        )
        .unwrap();
    if complete {
        let text = "synthetic completed response";
        let a = Turn {
            id: Uuid::new_v4(),
            conversation: f.conversation,
            exchange,
            sequence: 0,
            role: Role::Assistant,
            mode: Mode::Reader,
            outcome: Outcome::Completed,
            text: Some(text.into()),
            sources: vec![],
            correction_of: None,
            created_ms: 2,
            updated_ms: 2,
            completion: Some(CompletionEvidence {
                native_operation: Uuid::new_v4(),
                exact_text: text.into(),
                observation: observation(),
                procedure: "synthetic fixture only".into(),
            }),
        };
        ledger
            .append(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                a,
                vec![],
            )
            .unwrap();
    }
}
#[test]
fn synthetic_empty_and_interpreted_completed_preserve_history_and_exact_retry() {
    for pair in [false, true] {
        let f = Fixture::new();
        if pair {
            append_pair(&f, true)
        }
        let before = f.state();
        let intent = f.prepared();
        assert_eq!(before, f.state());
        let report = f.confirm(Operation::Apply, &intent);
        assert_eq!(
            report.status,
            Status::Acknowledged,
            "{}",
            serde_json::to_string(&report).unwrap()
        );
        let transaction = report.receipt.as_ref().unwrap().transaction;
        drop(report);
        let after = f.state();
        for (path, bytes) in before {
            assert_eq!(
                after.get(&path),
                Some(&bytes),
                "original changed {}",
                path.display()
            );
        }
        for op in [Operation::Apply, Operation::Status] {
            let report = f.confirm(op, &intent);
            assert_eq!(report.status, Status::Acknowledged);
            assert_eq!(report.receipt.as_ref().unwrap().transaction, transaction);
            assert_eq!(report.current_state, Some("desired-tombstones"));
            drop(report);
            assert_eq!(after, f.state());
        }
    }
}
#[test]
fn synthetic_unsettled_busy_and_changed_original_refuse_without_writes() {
    let f = Fixture::new();
    append_pair(&f, false);
    let before = f.state();
    assert_eq!(
        f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string())
        )
        .status,
        Status::Conflict
    );
    assert_eq!(before, f.state());
    let f = Fixture::new();
    let intent = f.prepared();
    let held = Store::open(f.paths.clone()).unwrap();
    assert_eq!(f.confirm(Operation::Apply, &intent).status, Status::Busy);
    drop(held);
    let mut b = fs::read(&f.config).unwrap();
    b.push(b' ');
    fs::write(&f.config, b).unwrap();
    let before = f.state();
    assert_eq!(
        f.confirm(Operation::Apply, &intent).status,
        Status::Conflict
    );
    assert_eq!(before, f.state());
}
#[test]
fn synthetic_lost_ack_partial_objects_and_original_status_never_republish() {
    use storage::conversation_cleanup::{Plan, Session};
    for fault in [Fault::AfterObjects, Fault::BeforeCommit, Fault::AfterCommit] {
        let f = Fixture::new();
        let i = f.prepared();
        let p = Plan::new(&i).unwrap();
        let mut s = Session::open(f.paths.clone(), &p.known()).unwrap();
        let mode = s.store.sync_mode_snapshot().unwrap();
        let guard = s.store.ordinary_mode_lease(&mode.token).unwrap();
        let snapshot = s.snapshot(f.conversation).unwrap();
        s.reserve(&i, &p, &snapshot).unwrap();
        s.store.set_fault(fault).unwrap();
        let mut attempted = false;
        assert!(s.publish(&i, &p, &mut attempted).is_err());
        assert!(attempted);
        drop(guard);
        drop(s);
        let after = f.state();
        for op in [Operation::Status, Operation::Apply] {
            let r = f.confirm(op, &i);
            assert_eq!(
                r.status,
                if fault == Fault::AfterCommit {
                    Status::Acknowledged
                } else {
                    Status::Unresolved
                },
                "{}",
                serde_json::to_string(&r).unwrap()
            );
            assert!(r.possible_effect);
            drop(r);
            assert_eq!(after, f.state());
        }
    }
}

#[test]
fn before_objects_fault_has_no_effect_and_original_status_remains_unchanged() {
    use storage::conversation_cleanup::{Plan, Session};
    let f = Fixture::new();
    let i = f.prepared();
    let plan = Plan::new(&i).unwrap();
    let before = f.state();
    let mut session = Session::open(f.paths.clone(), &plan.known()).unwrap();
    let mode = session.store.sync_mode_snapshot().unwrap();
    let guard = session.store.ordinary_mode_lease(&mode.token).unwrap();
    let snapshot = session.snapshot(f.conversation).unwrap();
    session.reserve(&i, &plan, &snapshot).unwrap();
    session.store.set_fault(Fault::BeforeObjects).unwrap();
    let mut attempted = false;
    assert!(session.publish(&i, &plan, &mut attempted).is_err());
    assert!(attempted); // Dispatch began; caller must report uncertainty.
    drop(guard);
    drop(session);
    assert_eq!(before, f.state());
    let report = f.confirm(Operation::Status, &i);
    assert_eq!(report.status, Status::Unchanged);
    assert!(!report.possible_effect && report.receipt.is_none());
    drop(report);
    assert_eq!(before, f.state());
}

#[test]
fn corrupt_original_receipt_and_changed_derived_identity_never_acknowledge() {
    for defect in ["corrupt-object", "changed-manifest", "changed-derived"] {
        let f = Fixture::new();
        let i = f.prepared();
        drop(f.confirm(Operation::Apply, &i));
        let plan = storage::conversation_cleanup::Plan::new(&i).unwrap();
        let g = f.paths.data.join("generations").join(&i.generation);
        match defect {
            "corrupt-object" => {
                let receipt = plan.manifest.records.last().unwrap();
                fs::write(g.join("objects").join(&receipt.sha256), b"corrupt original").unwrap();
            }
            "changed-manifest" => {
                let mut manifest = plan.manifest.clone();
                manifest.records.reverse();
                fs::write(
                    g.join("commits")
                        .join(format!("{}.json", manifest.transaction_id)),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            }
            _ => {
                fs::remove_file(
                    g.join("commits")
                        .join(format!("{}.json", plan.manifest.transaction_id)),
                )
                .unwrap();
                let mut e = plan.records[0].clone();
                e.actor_id = Uuid::new_v4();
                let b = serde_json::to_vec(&e).unwrap();
                let sha = digest(&b);
                fs::write(g.join("objects").join(&sha), &b).unwrap();
                let m = Manifest {
                    format: 1,
                    transaction_id: Uuid::new_v4(),
                    scope: Scope::LocalTransaction,
                    records: vec![ObjectRef {
                        sha256: sha.clone(),
                        bytes: b.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(sha, Namespace::Conversation)]),
                    media: vec![],
                    media_coverage: vec![],
                };
                fs::write(
                    g.join("commits").join(format!("{}.json", m.transaction_id)),
                    serde_json::to_vec(&m).unwrap(),
                )
                .unwrap();
            }
        }
        let before = f.state();
        for op in [Operation::Apply, Operation::Status] {
            let r = f.confirm(op, &i);
            assert_eq!(
                r.status,
                if defect == "corrupt-object" {
                    Status::Unsupported
                } else {
                    Status::Conflict
                },
                "{defect}"
            );
            assert!(r.receipt.is_none());
            drop(r);
            assert_eq!(before, f.state());
        }
    }
}
#[test]
fn synthetic_historical_ack_survives_later_revision_and_conflict() {
    let f = Fixture::new();
    let i = f.prepared();
    drop(f.confirm(Operation::Apply, &i));
    let store = Store::open(f.paths.clone()).unwrap();
    let actor = store.actor_id;
    // A separate deliberate fixture publication creates a competing historical branch.
    let e = Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: f.conversation,
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::from([uuid(&i.root.revision).unwrap()]),
        operation_id: Uuid::new_v4(),
        actor_id: actor,
        kind: Kind::Value,
        payload: serde_json::to_value(Record::Root(Root {
            id: f.conversation,
            next_sequence: 0,
            binding: None,
            created_ms: 1,
            updated_ms: 3,
        }))
        .unwrap(),
        media_descriptors: vec![],
    };
    let b = serde_json::to_vec(&e).unwrap();
    let sha = digest(&b);
    let m = Manifest {
        format: 1,
        transaction_id: Uuid::new_v4(),
        scope: Scope::LocalTransaction,
        records: vec![ObjectRef {
            sha256: sha.clone(),
            bytes: b.len() as u64,
        }],
        record_namespaces: BTreeMap::from([(sha.clone(), Namespace::Conversation)]),
        media: vec![],
        media_coverage: vec![],
    };
    store.import(m, BTreeMap::from([(sha, b)])).unwrap();
    drop(store);
    let after = f.state();
    for op in [Operation::Apply, Operation::Status] {
        let r = f.confirm(op, &i);
        assert_eq!(r.status, Status::Acknowledged);
        assert_eq!(r.current_state, Some("changed"));
        drop(r);
        assert_eq!(after, f.state());
    }
}
#[test]
fn synthetic_commit_entry_headroom_refuses_before_prepare() {
    let f = Fixture::new();
    let generation: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
    let commits = f
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("commits");
    for _ in 0..4094 {
        fs::write(commits.join(format!(".stage-{}", Uuid::new_v4())), []).unwrap();
    }
    let before = f.state();
    let r = f.call(
        Operation::Prepare,
        Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
    );
    assert_ne!(r.status, Status::Prepared);
    drop(r);
    assert_eq!(before, f.state());
}

#[test]
fn actual_commit_entry_boundary_still_allows_original_receipt_reopen() {
    let f = Fixture::new();
    let generation: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
    let commits = f
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("commits");
    let existing = fs::read_dir(&commits).unwrap().count();
    for _ in existing..4094 {
        fs::write(commits.join(format!(".stage-{}", Uuid::new_v4())), []).unwrap();
    }
    let i = f.prepared();
    let r = f.confirm(Operation::Apply, &i);
    assert_eq!(r.status, Status::Acknowledged);
    drop(r);
    assert_eq!(fs::read_dir(&commits).unwrap().count(), 4095);
    let after = f.state();
    for op in [Operation::Apply, Operation::Status] {
        let r = f.confirm(op, &i);
        assert_eq!(r.status, Status::Acknowledged);
        drop(r);
        assert_eq!(after, f.state());
    }
}

#[test]
fn completed_export_associations_are_retained_and_unsettled_exports_refuse() {
    for outcome in [
        Outcome::Completed,
        Outcome::Prepared,
        Outcome::Failed,
        Outcome::Canceled,
        Outcome::ReconcileRequired,
    ] {
        let f = Fixture::new();
        let store = Arc::new(Store::open(f.paths.clone()).unwrap());
        let ledger = Ledger::new(store);
        let operation = Uuid::new_v4();
        ledger
            .associate_export(
                operation,
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                ExpectedHeads::default(),
                ExportAssociation {
                    id: Uuid::new_v4(),
                    conversation: f.conversation,
                    backend: "synthetic".into(),
                    native_note: if outcome == Outcome::Completed {
                        Some("synthetic-note-retain".into())
                    } else {
                        None
                    },
                    source_scope: "synthetic".into(),
                    source_revision: "synthetic".into(),
                    operation,
                    outcome: outcome.clone(),
                    attempt: None,
                },
            )
            .unwrap();
        drop(ledger);
        let before = f.state();
        if outcome == Outcome::Completed {
            let i = f.prepared();
            assert_eq!(i.summary.export_associations, 1);
            drop(f.confirm(Operation::Apply, &i));
            let after = f.state();
            for (p, b) in before {
                assert_eq!(after.get(&p), Some(&b));
            }
        } else {
            let r = f.call(
                Operation::Prepare,
                Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
            );
            assert_eq!(r.status, Status::Conflict);
            drop(r);
            assert_eq!(before, f.state());
        }
    }
}

#[test]
fn orphan_contradictory_and_ambiguous_outcome_facts_refuse() {
    for defect in ["orphan", "contradictory", "ambiguous"] {
        let f = Fixture::new();
        let store = Arc::new(Store::open(f.paths.clone()).unwrap());
        let ledger = Ledger::new(store.clone());
        let mut turn = user(f.conversation, Uuid::new_v4());
        turn.outcome = Outcome::Prepared;
        turn.text = None;
        let id = turn.id;
        ledger
            .append(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                turn,
                vec![],
            )
            .unwrap();
        let mut e = store.value(Namespace::Conversation, id).unwrap().unwrap();
        e.parents = BTreeSet::from([e.revision_id]);
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        e.payload["record"]["outcome"] = serde_json::json!("failed");
        let fact_id = Uuid::new_v4();
        let fact = OutcomeFact {
            id: fact_id,
            conversation: f.conversation,
            turn: if defect == "orphan" {
                Uuid::new_v4()
            } else {
                id
            },
            outcome: if defect == "contradictory" {
                Outcome::Canceled
            } else {
                Outcome::Failed
            },
            reason: AttemptReason::RequestFailed,
            settlement: None,
        };
        let envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: fact_id,
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::OutcomeFact(fact)).unwrap(),
            media_descriptors: vec![],
        };
        let mut records = vec![e, envelope.clone()];
        if defect == "ambiguous" {
            let mut another = envelope;
            another.record_id = Uuid::new_v4();
            another.revision_id = Uuid::new_v4();
            another.operation_id = Uuid::new_v4();
            another.payload["record"]["id"] = serde_json::json!(another.record_id);
            records.push(another);
        }
        store.commit(records, BTreeMap::new()).unwrap();
        drop(ledger);
        drop(store);
        let before = f.state();
        let r = f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        );
        assert_eq!(r.status, Status::Conflict, "{defect}");
        drop(r);
        assert_eq!(before, f.state());
    }
}

fn bind_historical_fixture(f: &Fixture) -> Uuid {
    let store = Arc::new(Store::open(f.paths.clone()).unwrap());
    let ledger = Ledger::new(store);
    let source = observation();
    let page = Uuid::new_v4();
    let operation = Uuid::new_v4();
    let binding = Ledger::binding_id(source.document, page);
    let claim = BindingReceipt {
        operation,
        conversation: f.conversation,
        request_fingerprint: "synthetic request".into(),
        adapter_fingerprint: "synthetic adapter".into(),
        intended_target: Some(page),
        observed_document: source.document,
        observed_page: page,
        expected_order: vec![source.page],
        observed_order: vec![source.page, page],
        source,
        persisted_revision: "synthetic revision".into(),
        observed_ms: 2,
        evidence_origin: "synthetic".into(),
    };
    ledger
        .bind(
            operation,
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            ExpectedHeads::default(),
            claim,
        )
        .unwrap();
    drop(ledger);
    binding
}
#[test]
fn synthetic_historical_binding_is_tombstoned_without_history_loss() {
    let f = Fixture::new();
    let binding = bind_historical_fixture(&f);
    let before = f.state();
    let intent = f.prepared();
    assert_eq!(intent.binding.as_ref().unwrap().id, binding.to_string());
    let r = f.confirm(Operation::Apply, &intent);
    assert_eq!(r.status, Status::Acknowledged);
    assert!(r.receipt.as_ref().unwrap().binding_revision.is_some());
    drop(r);
    let after = f.state();
    for (p, b) in before {
        assert_eq!(after.get(&p), Some(&b));
    }
    let store = Arc::new(Store::open(f.paths.clone()).unwrap());
    let ledger = Ledger::new(store);
    assert_eq!(
        ledger
            .store()
            .heads(Namespace::Conversation, binding)
            .unwrap()[0]
            .kind,
        Kind::Tombstone
    );
    assert!(ledger.inspect(f.conversation, true).is_ok());
}
#[test]
fn synthetic_terminal_failed_request_uses_same_manifest_fact() {
    let f = Fixture::new();
    let store = Arc::new(Store::open(f.paths.clone()).unwrap());
    let ledger = Ledger::new(store);
    let mut u = user(f.conversation, Uuid::new_v4());
    u.outcome = Outcome::Prepared;
    u.text = None;
    let id = u.id;
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            u.clone(),
            vec![],
        )
        .unwrap();
    u.outcome = Outcome::Failed;
    ledger
        .record_attempt_outcome(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            ledger.expected(Namespace::Conversation, id).unwrap(),
            u,
            AttemptReason::RequestFailed,
        )
        .unwrap();
    drop(ledger);
    let before = f.state();
    let intent = f.prepared();
    assert_eq!(before, f.state());
    drop(f.confirm(Operation::Apply, &intent));
}

#[test]
fn missing_foreign_conflicted_and_changed_original_bindings_refuse() {
    for defect in ["missing", "foreign", "conflict", "changed-original"] {
        let f = Fixture::new();
        let binding = bind_historical_fixture(&f);
        let intent = f.prepared();
        let store = Store::open(f.paths.clone()).unwrap();
        let mut e = store
            .value(Namespace::Conversation, binding)
            .unwrap()
            .unwrap();
        e.parents = if defect == "conflict" {
            e.parents.clone()
        } else {
            BTreeSet::from([e.revision_id])
        };
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        match defect {
            "missing" => {
                e.kind = Kind::Tombstone;
                e.payload = serde_json::Value::Null;
            }
            "foreign" => {
                let foreign = Uuid::new_v4();
                e.payload["record"]["conversation"] = serde_json::json!(foreign);
                e.payload["record"]["receipt"]["conversation"] = serde_json::json!(foreign);
            }
            _ => {
                e.payload["record"]["receipt"]["persisted_revision"] =
                    serde_json::json!("changed synthetic evidence")
            }
        }
        if defect == "conflict" {
            // Import deliberately models a competing retained branch; the normal
            // local commit correctly rejects stale expected parents itself.
            let b = serde_json::to_vec(&e).unwrap();
            let sha = digest(&b);
            store
                .import(
                    Manifest {
                        format: 1,
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::LocalTransaction,
                        records: vec![ObjectRef {
                            sha256: sha.clone(),
                            bytes: b.len() as u64,
                        }],
                        record_namespaces: BTreeMap::from([(sha.clone(), Namespace::Conversation)]),
                        media: vec![],
                        media_coverage: vec![],
                    },
                    BTreeMap::from([(sha, b)]),
                )
                .unwrap();
        } else {
            store.commit(vec![e], BTreeMap::new()).unwrap();
        }
        drop(store);
        let before = f.state();
        let r = if defect == "changed-original" {
            f.confirm(Operation::Apply, &intent)
        } else {
            f.call(
                Operation::Prepare,
                Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
            )
        };
        assert_eq!(r.status, Status::Conflict, "{defect}");
        drop(r);
        assert_eq!(before, f.state());
    }
}

#[test]
fn shared_isolated_and_legacy_sync_enabled_never_grant_local_deletion() {
    use storage::sync::mode::SyncModeChoice;
    for choice in [
        SyncModeChoice::IsolatedBackup {
            binding_sha256: "a".repeat(64),
        },
        SyncModeChoice::SharedGroup {
            group: Uuid::new_v4(),
            binding_sha256: "b".repeat(64),
            root_id: "synthetic-owned-root".into(),
        },
    ] {
        let f = Fixture::new();
        let store = Store::open(f.paths.clone()).unwrap();
        let token = store.sync_mode_snapshot().unwrap().token;
        store.set_sync_mode(&token, Uuid::new_v4(), choice).unwrap();
        drop(store);
        let before = f.state();
        let r = f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        );
        assert_eq!(r.status, Status::Unsupported);
        drop(r);
        assert_eq!(before, f.state());
    }
    let f = Fixture::new();
    let mut config: Config = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
    config.sync.enabled = true;
    fs::write(&f.config, serde_json::to_vec(&config).unwrap()).unwrap();
    let before = f.state();
    let r = f.call(
        Operation::Prepare,
        Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
    );
    assert_eq!(r.status, Status::Unavailable);
    drop(r);
    assert_eq!(before, f.state());
}
#[test]
fn synthetic_output_failure_holds_lock_and_cannot_emit_second_report() {
    struct Failing {
        paths: StorePaths,
        writes: usize,
    }
    impl Write for Failing {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            assert!(matches!(
                Store::open_existing(self.paths.clone()),
                Err(storage::maintenance::Refusal::Busy)
            ));
            self.writes += 1;
            if self.writes == 1 {
                Ok(b.len().min(7))
            } else {
                Err(std::io::Error::other("synthetic output failure"))
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let f = Fixture::new();
    let before = f.state();
    let report = f.call(
        Operation::Prepare,
        Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
    );
    assert_eq!(report.status, Status::Prepared);
    let mut writer = Failing {
        paths: f.paths.clone(),
        writes: 0,
    };
    assert!(report.write_to(&mut writer).is_err());
    assert_eq!(writer.writes, 2);
    assert_eq!(before, f.state());
    assert!(Store::open_existing(f.paths.clone()).is_ok());
}

#[test]
fn synthetic_used_image_survives_logical_deletion_byte_exact() {
    let f = Fixture::new();
    let store = Arc::new(Store::open(f.paths.clone()).unwrap());
    let ledger = Ledger::new(store);
    let bytes = include_bytes!("../../tests/fixtures/trigger-dismiss/open.png");
    let dimensions = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    let image = storage::Media {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    };
    ledger.stage_image(&image, &bytes[..]).unwrap();
    let mut u = user(f.conversation, Uuid::new_v4());
    let sid = Uuid::new_v4();
    u.sources.push(sid);
    let source = ImageUse {
        id: sid,
        conversation: f.conversation,
        turn: u.id,
        image: image.clone(),
        observation: observation(),
        width: dimensions.0,
        height: dimensions.1,
        purpose: "synthetic overview".into(),
        ordinal: 0,
        parent: None,
        parent_dimensions: None,
        crop: None,
        transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        target: None,
        captured_ms: 1,
    };
    let exchange = u.exchange;
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            u,
            vec![source],
        )
        .unwrap();
    let text = "synthetic image response";
    let a = Turn {
        id: Uuid::new_v4(),
        conversation: f.conversation,
        exchange,
        sequence: 0,
        role: Role::Assistant,
        mode: Mode::Reader,
        outcome: Outcome::Completed,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 2,
        updated_ms: 2,
        completion: Some(CompletionEvidence {
            native_operation: Uuid::new_v4(),
            exact_text: text.into(),
            observation: observation(),
            procedure: "synthetic".into(),
        }),
    };
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            a,
            vec![],
        )
        .unwrap();
    let generation: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
    let object = f
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects")
        .join(&image.sha256);
    drop(ledger);
    let before = f.state();
    let i = f.prepared();
    assert_eq!(i.summary.source_records, 1);
    let r = f.confirm(Operation::Apply, &i);
    assert_eq!(r.status, Status::Acknowledged);
    drop(r);
    assert_eq!(fs::read(object).unwrap(), bytes);
    let after = f.state();
    for (p, b) in before {
        assert_eq!(after.get(&p), Some(&b));
    }
}

#[test]
fn original_apply_and_status_reject_semantically_equal_manifest_rewrite() {
    let f = Fixture::new();
    let i = f.prepared();
    let r = f.confirm(Operation::Apply, &i);
    assert_eq!(r.status, Status::Acknowledged);
    let tx = r.receipt.as_ref().unwrap().transaction;
    drop(r);
    let path = f
        .paths
        .data
        .join("generations")
        .join(&i.generation)
        .join("commits")
        .join(format!("{tx}.json"));
    let old = fs::read(&path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&old).unwrap();
    let changed = serde_json::to_vec_pretty(&value).unwrap();
    assert_ne!(old, changed);
    fs::write(path, changed).unwrap();
    let before = f.state();
    for op in [Operation::Apply, Operation::Status] {
        let report = f.confirm(op, &i);
        assert_eq!(report.status, Status::Conflict);
        assert!(report.receipt.is_none());
        drop(report);
        assert_eq!(before, f.state());
    }
}
struct ChangedOnObservation {
    config: PathBuf,
    lock: PathBuf,
    at: usize,
    calls: std::sync::atomic::AtomicUsize,
    replace_lock: bool,
}
impl service::Observer for ChangedOnObservation {
    fn inactive(&self) -> std::result::Result<(), Status> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 == self.at {
            if self.replace_lock {
                let old = self.lock.with_extension("synthetic-held-lock");
                assert_eq!(old.parent(), self.lock.parent());
                fs::rename(&self.lock, old).unwrap();
                fs::write(&self.lock, []).unwrap();
            } else {
                let mut b = fs::read(&self.config).unwrap();
                b.push(b' ');
                fs::write(&self.config, b).unwrap();
            }
        }
        Ok(())
    }
}
#[test]
fn final_observer_configuration_change_clears_prepared_and_postattempt_fields() {
    for apply in [false, true] {
        let f = Fixture::new();
        let (op, request, at) = if apply {
            let i = f.prepared();
            (Operation::Apply, Request::Confirm(Box::new(i)), 4)
        } else {
            (
                Operation::Prepare,
                Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
                3,
            )
        };
        let observer = Arc::new(ChangedOnObservation {
            config: f.config.clone(),
            lock: f.paths.data.join("store.lock"),
            at,
            calls: std::sync::atomic::AtomicUsize::new(0),
            replace_lock: false,
        });
        let report = engine::run(op, Some(f.config.clone()), request, observer);
        assert_eq!(
            report.status,
            if apply {
                Status::Acknowledged
            } else {
                Status::Prepared
            }
        );
        let mut out = Vec::new();
        report.write_to(&mut out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["status"], if apply { "unknown" } else { "conflict" });
        assert_eq!(v["possible_effect"], apply);
        for key in ["intent", "receipt", "summary", "current_state"] {
            assert!(v[key].is_null());
        }
    }
}
#[cfg(unix)]
#[test]
fn final_observer_named_lock_replacement_cannot_emit_prepared() {
    let f = Fixture::new();
    let observer = Arc::new(ChangedOnObservation {
        config: f.config.clone(),
        lock: f.paths.data.join("store.lock"),
        at: 3,
        calls: std::sync::atomic::AtomicUsize::new(0),
        replace_lock: true,
    });
    let report = engine::run(
        Operation::Prepare,
        Some(f.config.clone()),
        Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        observer,
    );
    assert_eq!(report.status, Status::Prepared);
    let mut out = Vec::new();
    report.write_to(&mut out).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "conflict");
    assert!(v["intent"].is_null());
    assert_eq!(v["possible_effect"], false);
}

use remarkable_open_sdk::{
    capture::{synthetic::*, *},
    mock::MockPlatform,
    Platform,
};
use std::{io::Cursor, time::Duration};
fn sdk(id: Uuid) -> remarkable_open_sdk::Uuid {
    remarkable_open_sdk::Uuid::parse(&id.to_string()).unwrap()
}
fn png(image: &image::DynamicImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn batch(device: Uuid, document: Uuid, page: Uuid) -> CapturedBatch {
    let source = MockPlatform::new(sdk(device), sdk(document), vec![sdk(page)], sdk(page))
        .observe_page()
        .unwrap();
    let native = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(4, 6, |x, y| {
        image::Rgba([(x * 40) as u8, (y * 30) as u8, 10, 255])
    }));
    let acquisition = Acquisition {
        before: source.clone(),
        after: source.clone(),
        rendered_owner: source.clone(),
        render_generation: u64::MAX,
        buffer_generation: u64::MAX,
        input_changed: false,
        interval: [
            Duration::new((1u64 << 53) + 1, 123_456_789),
            Duration::new(u64::MAX - 1, 999_999_999),
        ],
        viewport_revision: "fixture-viewport".into(),
        conversion_procedure: "fixture-rgba8".into(),
        procedure_revision: "fixture-generation-check".into(),
        native_png: png(&native),
        native_transform: Affine::new([0.25, -0.0, 0.0, 1.0 / 6.0, -0.0, 0.0]).unwrap(),
        valid_source_region: SourceRegion {
            x: -0.0,
            y: 0.0,
            width: 4.0,
            height: 6.0,
        },
        target: Some([-0.0, 0.5]),
    };
    let request = CaptureRequest::new(
        remarkable_open_sdk::OperationId(sdk(Uuid::new_v4())),
        source,
        Duration::new(u64::MAX, 0),
        false,
    );
    let images = vec![
        remarkable_open_sdk::capture::synthetic::ImageInput {
            role: ImageRole::Overview,
            crop: PixelRect {
                x: 0,
                y: 0,
                width: 4,
                height: 6,
            },
            output_dimensions: [2, 3],
            filter: ResizeFilter::Nearest,
            png: png(&native.resize_exact(2, 3, image::imageops::FilterType::Nearest)),
        },
        remarkable_open_sdk::capture::synthetic::ImageInput {
            role: ImageRole::Detail(7),
            crop: PixelRect {
                x: 0,
                y: 2,
                width: 4,
                height: 2,
            },
            output_dimensions: [4, 2],
            filter: ResizeFilter::Nearest,
            png: png(&native.crop_imm(0, 2, 4, 2)),
        },
    ];
    validate_batch(&request, acquisition, images, CaptureLimits::default()).unwrap()
}

fn development_capture_fixture() -> (
    remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture,
    Vec<u8>,
    Vec<u8>,
) {
    use remarkable_open_sdk::development_capture::*;
    let expected = ExpectedCaptureBinding {
        nonce: "0123456789abcdef0123456789abcdef".into(),
        attempt_pid: "42".into(),
        attempt_start: "100".into(),
        root_device: "19".into(),
        root_inode: "200".into(),
        document_id: "00000000-0000-4000-8000-000000000001".into(),
        expected_order: vec!["00000000-0000-4000-8000-000000000002".into()],
    };
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 3)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let png = encoded.into_inner();
    let raw = serde_json::to_vec_pretty(&serde_json::json!({
        "kind":"development-capture-observation", "version":2,
        "nonce":expected.nonce, "attempt_pid":expected.attempt_pid, "attempt_start":expected.attempt_start,
        "root_device":expected.root_device, "root_inode":expected.root_inode, "setup_profile":"main-dev-facts-120s",
        "setup_budget_ms":120000, "capture_budget_ms":5000,
        "accepted_ms":100, "baseline_ms":101, "grab_start_ms":102, "grab_end_ms":103, "post_read_ms":104, "completed_ms":105,
        "document_id":expected.document_id, "page_id":expected.expected_order[0], "page_index":0,
        "begin_epoch":"1", "end_epoch":"1", "width":2, "height":3, "dpr":1,
        "image_width":2, "image_height":3, "png_bytes":png.len(), "png_sha256":crate::storage::digest(&png),
        "image_status":"available", "gui_callback_completed":true, "scope_current":true,
        "atomic_snapshot":false, "native_authority":false, "render_authority":false, "ui_acknowledged":false,
        "observed_order":false, "discovery_scope":DISCOVERY_SCOPE
    })).unwrap();
    let capture =
        ReadOnlyDevelopmentCapture::from_collected_v11(&expected, raw.clone(), png.clone())
            .unwrap();
    (capture, raw, png)
}

fn populated_variant(f: &Fixture, variant: &str) -> (Uuid, Uuid) {
    let store = Arc::new(Store::open(f.paths.clone()).unwrap());
    let ledger = Ledger::new(store);
    let exchange = Uuid::new_v4();
    let mut u = user(f.conversation, exchange);
    let turn_id = u.id;
    let source_id = if variant == "image" {
        let bytes = png(&image::DynamicImage::new_rgba8(2, 3));
        let image = storage::Media {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
            media_type: "image/png".into(),
        };
        ledger.stage_image(&image, bytes.as_slice()).unwrap();
        let id = Uuid::new_v4();
        u.sources = vec![id];
        let source = ImageUse {
            id,
            conversation: f.conversation,
            turn: u.id,
            image,
            observation: observation(),
            width: 2,
            height: 3,
            purpose: "synthetic variant".into(),
            ordinal: 0,
            parent: None,
            parent_dimensions: None,
            crop: None,
            transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            target: None,
            captured_ms: 1,
        };
        ledger
            .append(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                u,
                vec![source],
            )
            .unwrap();
        id
    } else {
        u.outcome = Outcome::Prepared;
        u.text = None;
        let id = match variant {
            "legacy" => {
                let bytes = png(&image::DynamicImage::new_rgba8(2, 3));
                let id = Uuid::new_v4();
                let input = LegacyImageInput {
                    descriptor: LegacyImage {
                        media: storage::Media {
                            sha256: digest(&bytes),
                            bytes: bytes.len() as u64,
                            media_type: "image/png".into(),
                        },
                        dimensions: [2, 3],
                        role: LegacyImageRole::Overview,
                        provider_ordinal: Some(0),
                    },
                    bytes,
                };
                ledger
                    .prepare_legacy(
                        Uuid::new_v4(),
                        ledger
                            .expected(Namespace::Conversation, f.conversation)
                            .unwrap(),
                        u,
                        id,
                        None,
                        vec![input],
                    )
                    .unwrap();
                id
            }
            "sdk" => {
                let batch = batch(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
                ledger
                    .prepare_sdk_fixture(
                        Uuid::new_v4(),
                        ledger
                            .expected(Namespace::Conversation, f.conversation)
                            .unwrap(),
                        u,
                        &batch,
                    )
                    .unwrap()
                    .capture
            }
            "development" => {
                let (capture, _, _) = development_capture_fixture();
                let id = Uuid::new_v4();
                ledger
                    .prepare_development(
                        Uuid::new_v4(),
                        ledger
                            .expected(Namespace::Conversation, f.conversation)
                            .unwrap(),
                        u,
                        id,
                        &capture,
                    )
                    .unwrap();
                id
            }
            _ => panic!("unknown synthetic variant"),
        };
        let e = ledger
            .store()
            .value(Namespace::Conversation, turn_id)
            .unwrap()
            .unwrap();
        let Record::Turn(mut interpreted) = serde_json::from_value(e.payload).unwrap() else {
            panic!("turn")
        };
        interpreted.outcome = Outcome::Interpreted;
        interpreted.text = Some("synthetic interpreted capture".into());
        ledger
            .advance(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                ledger.expected(Namespace::Conversation, turn_id).unwrap(),
                interpreted,
            )
            .unwrap();
        id
    };
    let text = "synthetic completed capture";
    let a = Turn {
        id: Uuid::new_v4(),
        conversation: f.conversation,
        exchange,
        sequence: 0,
        role: Role::Assistant,
        mode: Mode::Reader,
        outcome: Outcome::Completed,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 2,
        updated_ms: 2,
        completion: Some(CompletionEvidence {
            native_operation: Uuid::new_v4(),
            exact_text: text.into(),
            observation: observation(),
            procedure: "synthetic only".into(),
        }),
    };
    ledger
        .append(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, f.conversation)
                .unwrap(),
            a,
            vec![],
        )
        .unwrap();
    (source_id, turn_id)
}
#[test]
fn every_supported_source_variant_keeps_original_provenance_and_media() {
    for variant in ["image", "legacy", "sdk", "development"] {
        let f = Fixture::new();
        let _ = populated_variant(&f, variant);
        let before = f.state();
        let intent = f.prepared();
        assert_eq!(intent.summary.source_records, 1);
        let r = f.confirm(Operation::Apply, &intent);
        assert_eq!(r.status, Status::Acknowledged, "variant{variant}");
        drop(r);
        let after = f.state();
        for (p, b) in before {
            assert_eq!(
                after.get(&p),
                Some(&b),
                "variant{variant} rewrote historical bytes"
            );
        }
    }
}
#[test]
fn every_source_variant_refuses_orphan_foreign_and_changed_turn_links() {
    for variant in ["image", "legacy", "sdk", "development"] {
        for defect in ["orphan", "foreign", "turn"] {
            let f = Fixture::new();
            let (source, turn) = populated_variant(&f, variant);
            let store = Store::open(f.paths.clone()).unwrap();
            let (namespace, id) = if defect == "orphan" {
                (Namespace::Conversation, turn)
            } else {
                (Namespace::Source, source)
            };
            let mut e = store.value(namespace, id).unwrap().unwrap();
            let parent = e.revision_id;
            e.revision_id = Uuid::new_v4();
            e.operation_id = Uuid::new_v4();
            e.actor_id = store.actor_id;
            e.parents = BTreeSet::from([parent]);
            match defect {
                "orphan" => e.payload["record"]["sources"] = serde_json::json!([]),
                "foreign" => {
                    e.payload["record"]["conversation"] = serde_json::json!(Uuid::new_v4())
                }
                _ => e.payload["record"]["turn"] = serde_json::json!(Uuid::new_v4()),
            };
            store.commit(vec![e], BTreeMap::new()).unwrap();
            drop(store);
            let before = f.state();
            let r = f.call(
                Operation::Prepare,
                Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
            );
            assert_eq!(r.status, Status::Conflict, "variant{variant}/{defect}");
            drop(r);
            assert_eq!(before, f.state());
        }
    }
}

#[test]
fn matching_tombstone_without_original_receipt_is_unresolved_not_acknowledged() {
    let f = Fixture::new();
    let intent = f.prepared();
    let report = f.confirm(Operation::Apply, &intent);
    assert_eq!(report.status, Status::Acknowledged);
    drop(report);
    let plan = storage::conversation_cleanup::Plan::new(&intent).unwrap();
    let commits = f
        .paths
        .data
        .join("generations")
        .join(&intent.generation)
        .join("commits");
    fs::remove_file(commits.join(format!("{}.json", plan.manifest.transaction_id))).unwrap();
    // Separate deliberate fixture publication: a matching tombstone is evidence of
    // current state, never proof of the original deterministic transaction.
    let root = &plan.manifest.records[0];
    let other = Manifest {
        format: 1,
        transaction_id: Uuid::new_v4(),
        scope: Scope::LocalTransaction,
        records: vec![root.clone()],
        record_namespaces: BTreeMap::from([(root.sha256.clone(), Namespace::Conversation)]),
        media: vec![],
        media_coverage: vec![],
    };
    fs::write(
        commits.join(format!("{}.json", other.transaction_id)),
        serde_json::to_vec(&other).unwrap(),
    )
    .unwrap();
    let before = f.state();
    for op in [Operation::Status, Operation::Apply] {
        let r = f.confirm(op, &intent);
        assert_eq!(r.status, Status::Unresolved);
        assert!(r.receipt.is_none());
        assert!(r.possible_effect);
        drop(r);
        assert_eq!(before, f.state());
    }
}

#[test]
fn already_used_operation_refuses_preparation_before_any_publication() {
    let f = Fixture::new();
    let store = Store::open(f.paths.clone()).unwrap();
    let receipts = store
        .snapshot_heads_matching(&[Namespace::Conversation], 4096, |e| {
            e.payload["record_kind"] == "receipt"
        })
        .unwrap();
    let operation = receipts[0].payload["record"]["acknowledgment"]["operation"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(store);
    let before = f.state();
    let report = f.call(
        Operation::Prepare,
        Request::Prepare(operation, f.conversation.to_string()),
    );
    assert_eq!(report.status, Status::Conflict);
    assert!(report.intent.is_none());
    drop(report);
    assert_eq!(before, f.state());
}

#[test]
fn complete_closed_request_shapes_and_intent_mutations_refuse() {
    let id = "11111111-1111-4111-8111-111111111111";
    for body in [
        serde_json::json!({"request_version":2,"conversation":id}),
        serde_json::json!({"request_version":1,"conversation":"00000000-0000-0000-0000-000000000000"}),
        serde_json::json!({"request_version":1,"conversation":"ABCDEFAB-1111-4111-8111-111111111111"}),
        serde_json::json!({"request_version":1,"conversation":id,"unknown":"synthetic-private-value"}),
        serde_json::json!({"request_version":1.0,"conversation":id}),
        serde_json::json!({"request_version":true,"conversation":id}),
    ] {
        assert!(parse(Operation::Inspect, &serde_json::to_vec(&body).unwrap()).is_err());
    }
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/conversation_cleanup_digest_vectors.json"
    ))
    .unwrap();
    let mut original = vectors["cases"][1]["original_fields"].clone();
    original["intent_sha256"] = vectors["cases"][1]["intent_sha256"].clone();
    for key in ["binding", "root", "summary"] {
        for value in [serde_json::json!([]), serde_json::json!({"unknown":1})] {
            let mut changed = original.clone();
            changed[key] = value;
            assert!(parse(
                Operation::Apply,
                &serde_json::to_vec(&serde_json::json!({"intent":changed})).unwrap()
            )
            .is_err());
        }
    }
    for key in ["binding", "summary", "root", "intent_sha256"] {
        let mut changed = original.clone();
        changed.as_object_mut().unwrap().remove(key);
        assert!(parse(
            Operation::Status,
            &serde_json::to_vec(&serde_json::json!({"intent":changed})).unwrap()
        )
        .is_err());
    }
    let mut changed = original.clone();
    changed["summary"]["turns"] = serde_json::json!(4097);
    assert!(parse(
        Operation::Apply,
        &serde_json::to_vec(&serde_json::json!({"intent":changed})).unwrap()
    )
    .is_err());
    let mut changed: Intent = serde_json::from_value(original).unwrap();
    changed.root.sha256 = "1".repeat(64);
    assert!(changed.validate().is_err());
}
#[test]
fn existing_only_missing_markers_and_unknown_layouts_never_initialize_or_repair() {
    for kind in 0..9 {
        let f = Fixture::new();
        let generation: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
        let g = f
            .paths
            .data
            .join("generations")
            .join(generation.to_string());
        match kind {
            0 => fs::remove_file(f.paths.data.join("identity.json")).unwrap(),
            1 => fs::remove_file(f.paths.data.join("CURRENT")).unwrap(),
            2 => fs::remove_file(f.paths.cache.join("owner.json")).unwrap(),
            3 => fs::remove_file(f.paths.credentials.join("owner.json")).unwrap(),
            4 => fs::remove_file(f.paths.data.join("store.lock")).unwrap(),
            5 => fs::write(g.join("format.json"), b"2").unwrap(),
            6 => fs::write(g.join("selection-feature.json"), b"{}").unwrap(),
            7 => fs::write(g.join("unknown-entry"), b"retain").unwrap(),
            _ => fs::write(g.join("objects").join("unknown-object"), b"retain").unwrap(),
        }
        let before = f.state();
        let r = f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        );
        assert!(matches!(
            r.status,
            Status::Unavailable | Status::Unsupported | Status::Conflict
        ));
        assert!(!r.possible_effect && r.intent.is_none());
        drop(r);
        assert_eq!(before, f.state(), "layout{kind} repaired");
    }
}
#[test]
fn explicit_local_mode_is_supported_and_changed_mode_or_generation_is_not() {
    use storage::sync::mode::SyncModeChoice;
    let f = Fixture::new();
    let store = Store::open(f.paths.clone()).unwrap();
    let mode = store.sync_mode_snapshot().unwrap();
    store
        .set_sync_mode(&mode.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
        .unwrap();
    drop(store);
    let i = f.prepared();
    drop(f.confirm(Operation::Apply, &i));
    let f = Fixture::new();
    let i = f.prepared();
    let store = Store::open(f.paths.clone()).unwrap();
    let mode = store.sync_mode_snapshot().unwrap();
    store
        .set_sync_mode(&mode.token, Uuid::new_v4(), SyncModeChoice::LocalOnly)
        .unwrap();
    drop(store);
    let before = f.state();
    assert_eq!(f.confirm(Operation::Apply, &i).status, Status::Conflict);
    assert_eq!(before, f.state());
    let f = Fixture::new();
    let i = f.prepared();
    let g = Uuid::new_v4();
    let root = f.paths.data.join("generations").join(g.to_string());
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("objects")).unwrap();
    fs::create_dir(root.join("commits")).unwrap();
    fs::write(root.join("format.json"), b"1").unwrap();
    fs::write(
        f.paths.data.join("CURRENT"),
        serde_json::to_vec(&g).unwrap(),
    )
    .unwrap();
    let before = f.state();
    assert_eq!(f.confirm(Operation::Apply, &i).status, Status::Conflict);
    assert_eq!(before, f.state());
}
#[test]
fn retained_schema_two_is_refused_even_with_current_schema_one() {
    let f = Fixture::new();
    let store = Store::open(f.paths.clone()).unwrap();
    let mut e = store
        .value(Namespace::Conversation, f.conversation)
        .unwrap()
        .unwrap();
    e.parents = BTreeSet::from([e.revision_id]);
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.domain_schema_version = 2;
    store.commit(vec![e.clone()], BTreeMap::new()).unwrap();
    e.parents = BTreeSet::from([e.revision_id]);
    e.revision_id = Uuid::new_v4();
    e.operation_id = Uuid::new_v4();
    e.domain_schema_version = 1;
    store.commit(vec![e], BTreeMap::new()).unwrap();
    drop(store);
    let before = f.state();
    let r = f.call(
        Operation::Prepare,
        Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
    );
    assert_eq!(r.status, Status::Conflict);
    drop(r);
    assert_eq!(before, f.state());
}
#[test]
fn ordinary_staging_and_unrelated_original_documents_remain_unchanged() {
    let f = Fixture::new();
    let g: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
    let root = f.paths.data.join("generations").join(g.to_string());
    let stage = root
        .join("objects")
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&stage, b"retain incomplete bytes").unwrap();
    fs::write(f.root.join("original.pdf"), b"synthetic unrelated original").unwrap();
    let before = f.state();
    let i = f.prepared();
    drop(f.confirm(Operation::Apply, &i));
    let after = f.state();
    for (p, b) in before {
        assert_eq!(after.get(&p), Some(&b));
    }
    assert_eq!(fs::read(stage).unwrap(), b"retain incomplete bytes");
}

#[test]
fn settled_exchange_variants_and_ambiguous_or_pending_states_are_distinct() {
    for terminal in [Outcome::Failed, Outcome::Canceled] {
        let f = Fixture::new();
        let store = Arc::new(Store::open(f.paths.clone()).unwrap());
        let ledger = Ledger::new(store);
        let exchange = Uuid::new_v4();
        let u = user(f.conversation, exchange);
        ledger
            .append(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                u,
                vec![],
            )
            .unwrap();
        let mut a = user(f.conversation, exchange);
        a.id = Uuid::new_v4();
        a.role = Role::Assistant;
        a.outcome = Outcome::Generated;
        a.text = Some("synthetic draft".into());
        let id = a.id;
        ledger
            .append(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                a,
                vec![],
            )
            .unwrap();
        let e = ledger
            .store()
            .value(Namespace::Conversation, id)
            .unwrap()
            .unwrap();
        let Record::Turn(mut a) = serde_json::from_value(e.payload).unwrap() else {
            panic!("turn")
        };
        a.outcome = terminal;
        ledger
            .record_attempt_outcome(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, f.conversation)
                    .unwrap(),
                ledger.expected(Namespace::Conversation, id).unwrap(),
                a,
                AttemptReason::ProviderFailure,
            )
            .unwrap();
        drop(ledger);
        let i = f.prepared();
        assert_eq!(i.summary.turns, 2);
    }
    for defect in [
        "duplicate-user",
        "duplicate-assistant",
        "wrong-exchange",
        "wrong-mode",
        "pending",
        "uncertain",
    ] {
        let f = Fixture::new();
        append_pair(&f, true);
        let store = Store::open(f.paths.clone()).unwrap();
        let mut turns = store
            .snapshot_heads_matching(&[Namespace::Conversation], 4096, |e| {
                e.payload["record_kind"] == "turn"
            })
            .unwrap();
        turns.sort_by_key(|e| e.payload["record"]["sequence"].as_u64());
        let mut e = if defect == "duplicate-user" {
            turns[0].clone()
        } else {
            turns[1].clone()
        };
        let old = e.revision_id;
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        e.parents = BTreeSet::from([old]);
        match defect {
            "duplicate-user" => {
                let mut root = store
                    .value(Namespace::Conversation, f.conversation)
                    .unwrap()
                    .unwrap();
                root.parents = BTreeSet::from([root.revision_id]);
                root.revision_id = Uuid::new_v4();
                root.operation_id = Uuid::new_v4();
                root.payload["record"]["next_sequence"] = serde_json::json!(3);
                e.record_id = Uuid::new_v4();
                e.payload["record"]["id"] = serde_json::json!(e.record_id);
                e.parents.clear();
                e.payload["record"]["sequence"] = serde_json::json!(2);
                store.commit(vec![root, e], BTreeMap::new()).unwrap();
            }
            "duplicate-assistant" => {
                let mut root = store
                    .value(Namespace::Conversation, f.conversation)
                    .unwrap()
                    .unwrap();
                root.parents = BTreeSet::from([root.revision_id]);
                root.revision_id = Uuid::new_v4();
                root.operation_id = Uuid::new_v4();
                root.payload["record"]["next_sequence"] = serde_json::json!(3);
                e.record_id = Uuid::new_v4();
                e.payload["record"]["id"] = serde_json::json!(e.record_id);
                e.parents.clear();
                e.payload["record"]["sequence"] = serde_json::json!(2);
                store.commit(vec![root, e], BTreeMap::new()).unwrap();
            }
            "wrong-exchange" => {
                e.payload["record"]["exchange"] = serde_json::json!(Uuid::new_v4());
                store.commit(vec![e], BTreeMap::new()).unwrap();
            }
            "wrong-mode" => {
                e.payload["record"]["mode"] = serde_json::json!("writer");
                store.commit(vec![e], BTreeMap::new()).unwrap();
            }
            _ => {
                e.payload["record"]["outcome"] = serde_json::json!(if defect == "pending" {
                    "generated"
                } else {
                    "reconcile-required"
                });
                e.payload["record"]["completion"] = serde_json::Value::Null;
                store.commit(vec![e], BTreeMap::new()).unwrap();
            }
        }
        drop(store);
        let before = f.state();
        let r = f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        );
        assert_eq!(r.status, Status::Conflict, "defect{defect}");
        drop(r);
        assert_eq!(before, f.state());
    }
}
#[cfg(unix)]
#[test]
fn symlink_and_hardlinked_owned_payloads_refuse_without_following_or_repair() {
    for hardlink in [false, true] {
        let f = Fixture::new();
        let g: Uuid = storage::files::json(&f.paths.data.join("CURRENT"), 128).unwrap();
        let objects = f
            .paths
            .data
            .join("generations")
            .join(g.to_string())
            .join("objects");
        let path = fs::read_dir(&objects)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let other = f.root.join("outside-owned-payload");
        if hardlink {
            fs::hard_link(&path, &other).unwrap();
        } else {
            fs::rename(&path, &other).unwrap();
            std::os::unix::fs::symlink(&other, &path).unwrap();
        }
        let content = fs::read(&other).unwrap();
        let r = f.call(
            Operation::Prepare,
            Request::Prepare(Uuid::new_v4().to_string(), f.conversation.to_string()),
        );
        assert!(matches!(r.status, Status::Unsupported | Status::Conflict));
        assert!(!r.possible_effect);
        drop(r);
        assert_eq!(fs::read(other).unwrap(), content);
    }
}
