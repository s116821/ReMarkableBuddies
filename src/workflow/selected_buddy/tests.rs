use super::*;
use crate::conversation::{
    Binding, BindingReceipt, Ledger, Record, Root, SelectedAdmission, SourceObservation, SCHEMA,
};
use crate::device::backend::{AcquisitionKind, DeviceBackend, Frame, NavigationCompletion};
use crate::storage::selection::{SelectionChange, SelectionScope};
use crate::storage::{
    digest, Envelope, Kind, Manifest, Namespace, ObjectRef, Scope, Store, StorePaths, FORMAT,
    MAX_ITEMS,
};
use crate::workflow::selected_backend::SelectedBackendFacade;
use crate::workflow::xochitl_integration::NavigationDirection;
use remarkable_open_sdk::{
    mock::{MockPlatform, NavigationFault},
    navigation::{LogicalDirection, NavigationRequest},
    OperationId, Platform,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn id(n: u8) -> Uuid {
    Uuid::from_bytes([n; 16])
}
fn sdk(n: u8) -> remarkable_open_sdk::Uuid {
    remarkable_open_sdk::Uuid::parse(&id(n).to_string()).unwrap()
}
fn platform() -> MockPlatform {
    MockPlatform::new(sdk(9), sdk(1), vec![sdk(3), sdk(2), sdk(4)], sdk(2))
}

struct Fixture {
    path: PathBuf,
    scope: SelectionScope,
}
impl Fixture {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!("selected-buddy-{}", Uuid::new_v4())),
            scope: SelectionScope {
                group: Uuid::new_v4(),
                key_sha256: digest(b"selected-buddy-document"),
                binding_sha256: digest(b"selected-buddy-provider"),
            },
        }
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.path.join("data"),
                cache: self.path.join("cache"),
                credentials: self.path.join("credentials"),
            })
            .unwrap(),
        )
    }
    fn initialize(
        &self,
        store: &Arc<Store>,
        selected: &[Envelope],
        retained: &[Vec<Envelope>],
    ) -> (Arc<SelectedAdmission>, SelectionToken) {
        let admission =
            Arc::new(SelectedAdmission::new(store.clone(), self.scope.clone()).unwrap());
        let (change, objects) = change(selected, retained);
        let publication = admission.initialize(change, objects).unwrap();
        (admission, publication.token)
    }
    fn current(&self, store: &Store) -> SelectionToken {
        store
            .selected_snapshot(&self.scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token
    }
    fn inspect(
        &self,
        store: &Store,
        reader: &Reader,
        token: &SelectionToken,
        conversation: Uuid,
        original: &PageObservation,
        fresh: &PageObservation,
    ) -> Result<BuddyBindingInspection> {
        let current = self.current(store);
        let before = disk(&self.path);
        let result =
            reader
                .workflow
                .inspect_current_buddy_binding(token, conversation, original, fresh);
        assert!(
            reader.calls.borrow().is_empty(),
            "device calls: {:?}",
            reader.calls.borrow()
        );
        assert_eq!(self.current(store), current, "read published a selection");
        assert_eq!(disk(&self.path), before, "read changed durable Store files");
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn disk(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else if path == root.join("data").join("store.lock") {
                // Windows refuses reads of this exclusive OS lease's range.
                // Retain its inventory/length; hash every other durable file.
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    format!("exclusive-store-lease:{}", path.metadata().unwrap().len()),
                );
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    digest(&std::fs::read(&path).unwrap()),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}
fn envelope(store: &Store, record: Record) -> Envelope {
    let record_id = match &record {
        Record::Root(root) => root.id,
        Record::Binding(binding) => binding.id,
        _ => panic!("fixture record"),
    };
    Envelope {
        envelope_version: FORMAT,
        namespace: Namespace::Conversation,
        domain_schema_version: SCHEMA,
        record_id,
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(record).unwrap(),
        media_descriptors: vec![],
    }
}
fn receipt(conversation: Uuid, document: Uuid, page: Uuid) -> BindingReceipt {
    BindingReceipt {
        operation: Uuid::new_v4(),
        conversation,
        request_fingerprint: "synthetic exact request".into(),
        adapter_fingerprint: "synthetic adapter".into(),
        source: SourceObservation {
            document,
            page: id(3),
            session: "historical session".into(),
            visit: "historical visit".into(),
            content_revision: Some("historical content".into()),
            order_revision: Some("historical order".into()),
            capability_revision: "synthetic".into(),
            evidence_procedure: "synthetic fixture".into(),
        },
        intended_target: Some(page),
        observed_document: document,
        observed_page: page,
        expected_order: vec![id(3), id(4)],
        observed_order: vec![id(3), page, id(4)],
        persisted_revision: "historical persisted revision".into(),
        observed_ms: 1,
        evidence_origin: "synthetic".into(),
    }
}
fn root(store: &Store, conversation: Uuid, binding: Option<Uuid>) -> Envelope {
    envelope(
        store,
        Record::Root(Root {
            id: conversation,
            next_sequence: 0,
            binding,
            created_ms: 1,
            updated_ms: 1,
        }),
    )
}
fn binding(store: &Store, conversation: Uuid, document: Uuid, page: Uuid) -> Envelope {
    envelope(
        store,
        Record::Binding(Binding {
            id: Ledger::binding_id(document, page),
            conversation,
            receipt: receipt(conversation, document, page),
        }),
    )
}
fn values(store: &Store) -> Vec<Envelope> {
    vec![
        root(store, id(6), Some(Ledger::binding_id(id(1), id(2)))),
        binding(store, id(6), id(1), id(2)),
    ]
}
fn manifest(records: &[Envelope]) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut objects = BTreeMap::new();
    let mut references = vec![];
    let mut namespaces = BTreeMap::new();
    for record in records {
        let bytes = serde_json::to_vec(record).unwrap();
        let sha256 = digest(&bytes);
        references.push(ObjectRef {
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(sha256.clone(), record.namespace);
        objects.insert(sha256, bytes);
    }
    (
        Manifest {
            format: FORMAT,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: references,
            record_namespaces: namespaces,
            media: vec![],
            media_coverage: vec![],
        },
        objects,
    )
}
fn change(
    selected: &[Envelope],
    retained: &[Vec<Envelope>],
) -> (SelectionChange, BTreeMap<String, Vec<u8>>) {
    let (selected, mut objects) = manifest(selected);
    let retained = retained
        .iter()
        .map(|records| {
            let (manifest, more) = manifest(records);
            objects.extend(more);
            manifest
        })
        .collect();
    (
        SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"selected-buddy-base"),
            selected,
            retained,
        },
        objects,
    )
}
fn revise(prior: &Envelope) -> Envelope {
    let mut next = prior.clone();
    next.revision_id = Uuid::new_v4();
    next.operation_id = Uuid::new_v4();
    next.parents = BTreeSet::from([prior.revision_id]);
    next
}
fn deleted(prior: &Envelope) -> Envelope {
    let mut next = revise(prior);
    next.kind = Kind::Tombstone;
    next.payload = serde_json::Value::Null;
    next
}

struct Reader {
    workflow: Workflow,
    calls: Rc<RefCell<Vec<&'static str>>>,
}
impl Reader {
    fn new(admission: Arc<SelectedAdmission>) -> Self {
        let calls = Rc::new(RefCell::new(vec![]));
        let facade = SelectedBackendFacade::new(admission, Box::new(Recorder(calls.clone())));
        Self {
            workflow: Workflow::with_selected(facade, false),
            calls,
        }
    }
}
struct Recorder(Rc<RefCell<Vec<&'static str>>>);
impl Recorder {
    fn record(&self, call: &'static str) {
        self.0.borrow_mut().push(call);
    }
    fn unexpected<T>(&self, call: &'static str) -> Result<T> {
        self.record(call);
        anyhow::bail!("unexpected device call {call}")
    }
}
impl DeviceBackend for Recorder {
    fn acquisition_kind(&self) -> AcquisitionKind {
        self.record("acquisition");
        AcquisitionKind::Unsupported
    }
    fn capture_sdk(&mut self) -> Result<remarkable_open_sdk::capture::CapturedBatch> {
        self.unexpected("sdk capture")
    }
    fn capture_development(
        &mut self,
    ) -> Result<remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture> {
        self.unexpected("development capture")
    }
    fn history_snapshot(
        &mut self,
        _: Option<&str>,
    ) -> Result<Option<crate::workflow::history::PageState>> {
        self.unexpected("history snapshot")
    }
    fn history_mutate(
        &mut self,
        _: crate::workflow::history::Command,
        _: &str,
    ) -> Result<crate::workflow::history::PageState> {
        self.unexpected("history mutate")
    }
    fn history_discard(&mut self) {
        self.record("history discard");
    }
    fn wait_for_interactions(
        &mut self,
        _: Option<Duration>,
    ) -> Result<Vec<crate::device::interaction::Interaction>> {
        self.unexpected("interactions")
    }
    fn capture(&mut self) -> Result<Frame> {
        self.unexpected("capture")
    }
    fn detail_images(&self) -> Result<Vec<String>> {
        self.unexpected("details")
    }
    fn check_request_guard(&mut self) -> Result<()> {
        self.unexpected("check guard")
    }
    fn finish_request_guard(&mut self) -> Result<()> {
        self.unexpected("finish guard")
    }
    fn record_failure(&mut self, _: crate::workflow::indicator::Failure) {
        self.record("failure");
    }
    fn wait_for_trigger(&mut self) -> Result<()> {
        self.unexpected("trigger")
    }
    fn prepare_reader_trigger(&mut self) -> Result<()> {
        self.unexpected("prepare trigger")
    }
    fn navigate(&mut self, _: NavigationDirection) -> Result<NavigationCompletion> {
        self.unexpected("navigate")
    }
    fn render_text(&mut self, _: &str) -> Result<()> {
        self.unexpected("text")
    }
    fn body_mode(&mut self) -> Result<()> {
        self.unexpected("body")
    }
    fn line(&mut self, _: (i32, i32), _: (i32, i32)) -> Result<()> {
        self.unexpected("line")
    }
    fn erase(&mut self, _: (i32, i32), _: (i32, i32)) -> Result<()> {
        self.unexpected("erase")
    }
    fn bitmap(&mut self, _: &[Vec<bool>]) -> Result<()> {
        self.unexpected("bitmap")
    }
    fn progress(&mut self, _: Option<&str>) -> Result<()> {
        self.unexpected("progress")
    }
    fn status_stroke(&mut self, _: crate::workflow::indicator::Stroke) -> Result<()> {
        self.unexpected("status stroke")
    }
    fn status_clear(&mut self, _: &[crate::workflow::indicator::Stroke]) -> Result<()> {
        self.unexpected("status clear")
    }
    fn status_style_begin(&mut self) -> Result<bool> {
        self.unexpected("status begin")
    }
    fn status_style_end(&mut self) -> Result<()> {
        self.unexpected("status end")
    }
    fn monotonic(&self) -> Duration {
        self.record("clock");
        Duration::ZERO
    }
    fn status_suppressed(&mut self) {
        self.record("status suppressed");
    }
    fn load_header(&self) -> Option<image::DynamicImage> {
        self.record("load header");
        None
    }
    fn save_header(&mut self, _: &image::DynamicImage) -> Result<()> {
        self.unexpected("save header")
    }
    fn delay(&mut self, _: Duration) {
        self.record("delay");
    }
}

#[test]
fn actual_reader_current_binding_retains_exact_provenance_after_store_reopen() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let records = values(&store);
    let (admission, token) = fixture.initialize(&store, &records, &[]);
    let observation = platform().observe_page().unwrap();
    let mut reader = Reader::new(admission);
    let result = fixture
        .inspect(&store, &reader, &token, id(6), &observation, &observation)
        .unwrap();
    let BuddyBindingInspection::HistoricalCandidate(candidate) = result else {
        panic!("current candidate");
    };
    assert_eq!(candidate.observation(), &observation);
    assert_eq!(candidate.selected().selection(), &token);
    assert_eq!(candidate.selected().root_revision(), records[0].revision_id);
    assert_eq!(
        candidate.selected().binding_revision(),
        records[1].revision_id
    );
    let Record::Binding(expected) =
        serde_json::from_value::<Record>(records[1].payload.clone()).unwrap()
    else {
        panic!("binding");
    };
    assert_eq!(candidate.selected().receipt(), &expected.receipt);
    // The historical candidate does not supply the actual Reader effect context.
    assert!(reader
        .workflow
        .dispatch_reader(None, 0, &crate::conversation::ReaderHandoff::NextPage)
        .is_err());
    assert!(reader.calls.borrow().is_empty());
    drop(reader);
    drop(store);
    let reopened = fixture.open();
    let admission =
        Arc::new(SelectedAdmission::new(reopened.clone(), fixture.scope.clone()).unwrap());
    let reader = Reader::new(admission);
    assert_eq!(
        fixture
            .inspect(
                &reopened,
                &reader,
                &token,
                id(6),
                &observation,
                &observation
            )
            .unwrap(),
        BuddyBindingInspection::HistoricalCandidate(candidate)
    );
}

#[test]
fn actual_reader_uses_selected_binding_not_retained_or_all_history_competitor() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let records = values(&store);
    let other = root(&store, id(7), None);
    let loser = binding(&store, id(7), id(1), id(2));
    store
        .commit(
            vec![
                records[0].clone(),
                records[1].clone(),
                other.clone(),
                loser.clone(),
            ],
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(
        store
            .heads(Namespace::Conversation, records[1].record_id)
            .unwrap()
            .len(),
        2
    );
    let (admission, token) = fixture.initialize(&store, &records, &[vec![other, loser]]);
    let reader = Reader::new(admission);
    let observation = platform().observe_page().unwrap();
    let result = fixture
        .inspect(&store, &reader, &token, id(6), &observation, &observation)
        .unwrap();
    let BuddyBindingInspection::HistoricalCandidate(candidate) = result else {
        panic!("selected candidate");
    };
    assert_eq!(
        candidate.selected().binding_revision(),
        records[1].revision_id
    );
    assert_eq!(candidate.selected().receipt().conversation, id(6));
}

#[test]
fn actual_reader_selected_omissions_never_fill_from_retained_or_ordinary_store() {
    for missing in 0..3 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let history = values(&store);
        store.commit(history.clone(), BTreeMap::new()).unwrap();
        let selected = match missing {
            0 => vec![root(&store, id(7), None)],
            1 => vec![root(&store, id(6), None)],
            _ => vec![history[0].clone()], // Broken current link despite retained exact binding.
        };
        let (admission, token) = fixture.initialize(&store, &selected, &[history]);
        let reader = Reader::new(admission);
        let observation = platform().observe_page().unwrap();
        let result = fixture.inspect(&store, &reader, &token, id(6), &observation, &observation);
        match missing {
            0 => assert_eq!(
                result.unwrap(),
                BuddyBindingInspection::Unresolved(SelectedBindingUnresolved::RootNotSelected)
            ),
            1 => assert_eq!(
                result.unwrap(),
                BuddyBindingInspection::Unresolved(SelectedBindingUnresolved::BindingNotSelected)
            ),
            _ => assert!(result
                .unwrap_err()
                .to_string()
                .contains("selected binding reference missing")),
        }
    }
}

#[test]
fn actual_reader_current_tombstones_never_revive_valid_ancestors() {
    for deleted_index in 0..2 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let mut records = values(&store);
        records.push(deleted(&records[deleted_index]));
        let (admission, token) = fixture.initialize(&store, &records, &[]);
        let reader = Reader::new(admission);
        let observation = platform().observe_page().unwrap();
        assert_eq!(
            fixture
                .inspect(&store, &reader, &token, id(6), &observation, &observation)
                .unwrap(),
            BuddyBindingInspection::Unresolved(if deleted_index == 0 {
                SelectedBindingUnresolved::RootDeleted
            } else {
                SelectedBindingUnresolved::BindingDeleted
            })
        );
    }
}

#[test]
fn actual_reader_unbound_elsewhere_and_foreign_current_claims_remain_unresolved() {
    for case in 0..3 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let mut records = values(&store);
        let expected = match case {
            0 => {
                records[0] = root(&store, id(6), None);
                SelectedBindingUnresolved::RootUnbound
            }
            1 => {
                let elsewhere = binding(&store, id(6), id(1), id(8));
                records[0] = root(&store, id(6), Some(elsewhere.record_id));
                records.push(elsewhere);
                SelectedBindingUnresolved::RootBoundElsewhere
            }
            _ => {
                records[1] = binding(&store, id(7), id(1), id(2));
                records[0] = root(&store, id(6), None);
                records.push(root(&store, id(7), Some(records[1].record_id)));
                SelectedBindingUnresolved::OtherConversation
            }
        };
        let (admission, token) = fixture.initialize(&store, &records, &[]);
        let reader = Reader::new(admission);
        let observation = platform().observe_page().unwrap();
        assert_eq!(
            fixture
                .inspect(&store, &reader, &token, id(6), &observation, &observation)
                .unwrap(),
            BuddyBindingInspection::Unresolved(expected)
        );
    }
}

#[test]
fn actual_reader_current_foreign_binding_is_not_replaced_by_own_ancestor() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut records = values(&store);
    let mut foreign = revise(&records[1]);
    foreign.payload["record"]["conversation"] = serde_json::json!(id(7));
    foreign.payload["record"]["receipt"]["conversation"] = serde_json::json!(id(7));
    records.push(foreign);
    records.push(root(&store, id(7), Some(records[1].record_id)));
    let (admission, token) = fixture.initialize(&store, &records, &[]);
    let reader = Reader::new(admission);
    let observation = platform().observe_page().unwrap();
    assert_eq!(
        fixture
            .inspect(&store, &reader, &token, id(6), &observation, &observation)
            .unwrap(),
        BuddyBindingInspection::Unresolved(SelectedBindingUnresolved::OtherConversation)
    );
}

#[test]
fn actual_reader_malformed_imported_receipts_refuse_despite_valid_object_hashes() {
    for case in 0..10 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let mut records = values(&store);
        let Record::Binding(mut imported) =
            serde_json::from_value::<Record>(records[1].payload.clone()).unwrap()
        else {
            panic!("binding");
        };
        match case {
            0 => imported.receipt.intended_target = None,
            1 => imported.receipt.request_fingerprint.clear(),
            2 => imported.receipt.adapter_fingerprint.clear(),
            3 => imported.receipt.persisted_revision.clear(),
            4 => imported.receipt.evidence_origin.clear(),
            5 => imported.receipt.expected_order.push(id(3)),
            6 => imported.receipt.expected_order = vec![id(4)],
            7 => imported.receipt.expected_order.push(id(2)),
            8 => imported.receipt.expected_order.push(Uuid::nil()),
            _ => imported.receipt.observed_order.swap(0, 1),
        }
        records[1] = envelope(&store, Record::Binding(imported));
        let (admission, token) = fixture.initialize(&store, &records, &[]);
        let reader = Reader::new(admission);
        let observation = platform().observe_page().unwrap();
        assert!(
            fixture
                .inspect(&store, &reader, &token, id(6), &observation, &observation)
                .is_err(),
            "malformed receipt {case}"
        );
    }
}

#[test]
fn actual_reader_incomplete_chronology_ambiguous_document_and_bad_link_refuse() {
    for case in 1..4 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let mut records = values(&store);
        match case {
            1 => records[0].payload["record"]["next_sequence"] = serde_json::json!(1),
            2 => records.push(binding(&store, id(6), id(10), id(8))),
            _ => records[0].payload["record"]["binding"] = serde_json::json!(Uuid::new_v4()),
        }
        let (admission, token) = fixture.initialize(&store, &records, &[]);
        let reader = Reader::new(admission);
        let observation = platform().observe_page().unwrap();
        assert!(
            fixture
                .inspect(&store, &reader, &token, id(6), &observation, &observation)
                .is_err(),
            "invalid closure {case}"
        );
    }
}

#[test]
fn real_store_refuses_fork_before_reader_can_obtain_selected_binding() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut records = values(&store);
    let mut fork = records[1].clone();
    fork.revision_id = Uuid::new_v4();
    fork.operation_id = Uuid::new_v4();
    records.push(fork);
    let admission = SelectedAdmission::new(store.clone(), fixture.scope.clone()).unwrap();
    let (update, objects) = change(&records, &[]);
    let before = disk(&fixture.path);
    let error = admission
        .initialize(update, objects)
        .err()
        .expect("fork must refuse publication");
    assert!(error
        .to_string()
        .contains("conflicted selected record membership"));
    assert!(store
        .selected_snapshot(&fixture.scope, MAX_ITEMS)
        .unwrap()
        .is_none());
    let after = disk(&fixture.path);
    for (path, hash) in &before {
        assert_eq!(after.get(path), Some(hash));
    }
    // Publication may leave immutable orphan objects before refusing membership.
    // They are not a selected transaction, receipt, candidate or device action.
    for (path, hash) in after.iter().filter(|(path, _)| !before.contains_key(*path)) {
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "objects");
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), hash);
    }
}

#[test]
fn actual_reader_failed_request_refuses_historical_lookup_before_device_calls() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (admission, token) = fixture.initialize(&store, &values(&store), &[]);
    let mut reader = Reader::new(admission);
    reader.workflow.input_failed = true;
    let observation = platform().observe_page().unwrap();
    assert!(fixture
        .inspect(&store, &reader, &token, id(6), &observation, &observation)
        .unwrap_err()
        .to_string()
        .contains("Request input lost"));
}

#[test]
fn actual_reader_bounds_supplied_sdk_order_without_truncation_or_device_calls() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (admission, token) = fixture.initialize(&store, &values(&store), &[]);
    let reader = Reader::new(admission);
    let mut order = vec![sdk(2)];
    order.extend((1..=MAX_ITEMS).map(|n| {
        remarkable_open_sdk::Uuid::parse(&Uuid::from_u128(n as u128).to_string()).unwrap()
    }));
    let observation = MockPlatform::new(sdk(9), sdk(1), order, sdk(2))
        .observe_page()
        .unwrap();
    assert!(fixture
        .inspect(&store, &reader, &token, id(6), &observation, &observation)
        .unwrap_err()
        .to_string()
        .contains("domain order bound"));
}

#[test]
fn actual_reader_replacement_and_foreign_token_refuse_old_candidate() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let records = values(&store);
    let (admission, token) = fixture.initialize(&store, &records, &[]);
    let reader = Reader::new(admission.clone());
    let observation = platform().observe_page().unwrap();
    let initial = fixture
        .inspect(&store, &reader, &token, id(6), &observation, &observation)
        .unwrap();
    assert!(matches!(
        initial,
        BuddyBindingInspection::HistoricalCandidate(_)
    ));
    let (change, objects) = change(
        &[records[0].clone(), records[1].clone(), deleted(&records[1])],
        &[],
    );
    let replacement = admission.activate(&token, change, objects).unwrap();
    assert!(fixture
        .inspect(&store, &reader, &token, id(6), &observation, &observation)
        .unwrap_err()
        .to_string()
        .contains("replaced or stale"));
    assert_eq!(
        fixture
            .inspect(
                &store,
                &reader,
                &replacement.token,
                id(6),
                &observation,
                &observation
            )
            .unwrap(),
        BuddyBindingInspection::Unresolved(SelectedBindingUnresolved::BindingDeleted)
    );
    let foreign_fixture = Fixture::new();
    let foreign_store = foreign_fixture.open();
    let (_, foreign_token) =
        foreign_fixture.initialize(&foreign_store, &values(&foreign_store), &[]);
    assert!(fixture
        .inspect(
            &store,
            &reader,
            &foreign_token,
            id(6),
            &observation,
            &observation
        )
        .unwrap_err()
        .to_string()
        .contains("foreign selected admission scope"));
}

#[test]
fn actual_reader_full_opaque_sdk_observation_drift_refuses_without_device_io() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (admission, token) = fixture.initialize(&store, &values(&store), &[]);
    let reader = Reader::new(admission);
    let original = platform().observe_page().unwrap();
    let foreign_instance = platform().observe_page().unwrap();
    assert_eq!(original.page(), foreign_instance.page());
    assert!(fixture
        .inspect(&store, &reader, &token, id(6), &original, &foreign_instance)
        .is_err());
    for fault in [
        NavigationFault::BeforeDispatchInput,
        NavigationFault::BeforeDispatchSession,
        NavigationFault::BeforeDispatchVisit,
        NavigationFault::BeforeDispatchOrder,
    ] {
        let mut model = platform();
        let original = model.observe_page().unwrap();
        let request = NavigationRequest::new(
            OperationId(sdk(12)),
            original.clone(),
            LogicalDirection::Next,
            remarkable_open_sdk::PageKey {
                document: sdk(1),
                page: sdk(4),
            },
        )
        .unwrap();
        model.inject_navigation_fault(fault);
        let _ = model.navigate(&request, false);
        assert_eq!(model.gesture_count(), 0);
        let fresh = model.observe_page().unwrap();
        assert!(
            fixture
                .inspect(&store, &reader, &token, id(6), &original, &fresh)
                .is_err(),
            "SDK drift {fault:?}"
        );
    }
}

#[test]
fn sdk_invalid_identity_and_actual_reader_nil_identity_and_unbound_workflow_refuse() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let (admission, token) = fixture.initialize(&store, &values(&store), &[]);
    let reader = Reader::new(admission);
    for order in [vec![sdk(3), sdk(4)], vec![sdk(3), sdk(2), sdk(2)]] {
        // Public SDK refuses these before an opaque observation can be supplied.
        let invalid = MockPlatform::new(sdk(9), sdk(1), order, sdk(2));
        assert!(invalid.observe_page().is_err());
        assert_eq!(invalid.gesture_count(), 0);
    }
    let observation = platform().observe_page().unwrap();
    assert!(fixture
        .inspect(
            &store,
            &reader,
            &token,
            Uuid::nil(),
            &observation,
            &observation
        )
        .is_err());
    let calls = Rc::new(RefCell::new(vec![]));
    let unbound = Workflow::with_device(Box::new(Recorder(calls.clone())), false);
    assert!(unbound
        .inspect_current_buddy_binding(&token, id(6), &observation, &observation)
        .unwrap_err()
        .to_string()
        .contains("Unbound Workflow"));
    assert!(calls.borrow().is_empty());
}
