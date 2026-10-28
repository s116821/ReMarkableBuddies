use remarkable_reader_buddy::{
    conversation::{Ledger, Outcome},
    note_export::{
        self, mem::MemDestination, Budget, CorrelationLocation, Discovery, ExportStatus,
        NoteDestination, PreparedNote, RemoteNote, Result as ExportResult,
    },
    storage::{Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tokio::sync::watch;

struct LocalStore(PathBuf);
impl LocalStore {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rmb-export-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            })
            .unwrap(),
        )
    }
    fn ledger(&self) -> (Ledger, Uuid) {
        let ledger = Ledger::new(self.open());
        let id = Uuid::new_v4();
        ledger.create(id, Uuid::new_v4(), 1).unwrap();
        (ledger, id)
    }
}
impl Drop for LocalStore {
    fn drop(&mut self) {
        let root = self.0.canonicalize().unwrap();
        assert_eq!(
            root.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rmb-export-"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
fn prepared(conversation: Uuid) -> PreparedNote {
    PreparedNote {
        conversation,
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
        source_scope: "notebook:page:fixture".into(),
        source_revision: "r1".into(),
        title: "Prepared study note".into(),
        markdown: "Original equation and source coordinates (12,34).".into(),
    }
}

#[test]
fn notebook_joined_flow_uses_real_mem_http_for_create_reopen_refinement_update_and_lost_ack_reconciliation(
) {
    use remarkable_reader_buddy::{
        config::Personalization,
        conversation::{
            notebook::NotebookAdmission, ArchivedNormalizedSource, Mode, Record, Role, TextScope,
            Turn,
        },
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
        storage::{digest, Namespace},
        writer::notebook::{self, FrozenNotebookPayloads, PayloadContext, ValidatedNotebook},
        writer_flow, LLMEngine,
    };
    struct Model(usize);
    impl LLMEngine for Model {
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, _: &str) {
            panic!("text only")
        }
        fn clear_content(&mut self) {}
        fn execute(&mut self) -> anyhow::Result<String> {
            self.0 += 1;
            Ok(if self.0 % 2 == 1 {
                "x = 7 (page 1)."
            } else if self.0 >= 4 {
                "# Study\n\nx = 7 (page 1). Refined."
            } else {
                "# Study\n\nx = 7 (page 1)."
            }
            .into())
        }
    }
    for lose_ack in [false, true] {
        let local = LocalStore::new();
        let (ledger, conversation) = local.ledger();
        let paths = ledger.store().paths.clone();
        drop(ledger);
        let mut ledger = Ledger::new(Arc::new(Store::prepare_notebook(paths.clone()).unwrap()));
        let raw = serde_json::to_vec(&NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "http-notebook-fixture-v1".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "x = 7".into(),
            }],
        })
        .unwrap();
        let source = ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap();
        let preference = Personalization::default().snapshot().unwrap();
        let initial_operation = Uuid::new_v4();
        let initial_user = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:writer:v1:{initial_operation}:user").as_bytes(),
        );
        let archive = ArchivedNormalizedSource::from_notebook(
            conversation,
            initial_user,
            &source,
            TextScope::SuppliedNotebook,
            None,
        )
        .unwrap();
        let input = |ledger: &Ledger, operation, prior: Option<Turn>| {
            let reference = prior.as_ref().map(|turn| {
                ledger
                    .store()
                    .notebook_revision_reference(
                        &ledger
                            .store()
                            .value(Namespace::Conversation, turn.id)
                            .unwrap()
                            .unwrap(),
                    )
                    .unwrap()
                    .sha256
            });
            let payloads = FrozenNotebookPayloads::new(
                &source,
                &archive,
                PayloadContext {
                    operation,
                    request: "Keep x = 7 and page 1",
                    personalization: &preference,
                    history: &[],
                    history_range: None,
                    prior: prior.as_ref().zip(reference),
                    max_model_calls: 2,
                },
            )
            .unwrap();
            let user = Turn {
                id: Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("urn:rmb:writer:v1:{operation}:user").as_bytes(),
                ),
                conversation,
                exchange: operation,
                sequence: 0,
                role: Role::User,
                mode: Mode::Writer,
                outcome: Outcome::Interpreted,
                text: Some(payloads.plan().request.clone()),
                sources: vec![archive.id],
                correction_of: prior.as_ref().map(|turn| turn.id),
                created_ms: 0,
                updated_ms: 0,
                completion: None,
            };
            let guards = prior
                .as_ref()
                .map(|turn| {
                    ledger
                        .store()
                        .value(Namespace::Conversation, turn.id)
                        .unwrap()
                        .unwrap()
                })
                .into_iter()
                .collect();
            notebook::runner::Input {
                admission: NotebookAdmission {
                    plan: payloads.plan().clone(),
                    archive: archive.clone(),
                    canonical: source.canonical_bytes().to_vec(),
                    user,
                    expected: ledger
                        .expected(Namespace::Conversation, conversation)
                        .unwrap(),
                    guards,
                },
                payloads,
            }
        };
        let api = Api::new();
        api.state.lock().unwrap().lose_ack = lose_ack;
        let destination = api.client();
        let mut model = Model(0);
        let identity = notebook::flow::Identity {
            outer_operation: Uuid::new_v4(),
            correlation: Uuid::new_v4(),
            effect_operation: Uuid::new_v4(),
        };
        let original = input(&ledger, initial_operation, None);
        let replay = original.clone();
        let (_tx, rx) = watch::channel(false);
        let first = notebook::flow::run(&ledger, &mut model, &destination, original, identity, rx);
        assert_eq!(model.0, 2);
        assert_eq!(api.count("POST"), 1);
        if lose_ack {
            assert_eq!(first.status, writer_flow::Status::ReconcileRequired);
            let (_tx, rx) = watch::channel(false);
            let again =
                notebook::flow::run(&ledger, &mut model, &destination, replay, identity, rx);
            assert_eq!(again.status, writer_flow::Status::ReconcileRequired);
            assert_eq!(model.0, 2);
            assert_eq!(api.count("POST"), 1);
            let request = writer_flow::Request::ReconcileExport {
                conversation,
                correlation: identity.correlation,
            };
            let (_tx, rx) = watch::channel(false);
            let reconciled =
                writer_flow::run(&ledger, &mut model, &destination, &request, &preference, rx);
            assert_eq!(reconciled.status, writer_flow::Status::Exported);
            assert_eq!(api.count("POST"), 1);
        } else {
            assert_eq!(first.status, writer_flow::Status::Exported);
            drop(ledger);
            ledger = Ledger::new(local.open());
            let Record::Turn(prior) = Ledger::decode(
                &ledger
                    .store()
                    .value(Namespace::Conversation, first.draft.unwrap())
                    .unwrap()
                    .unwrap(),
            )
            .unwrap() else {
                panic!("draft required")
            };
            let next = notebook::flow::Identity {
                outer_operation: Uuid::new_v4(),
                effect_operation: Uuid::new_v4(),
                ..identity
            };
            let (_tx, rx) = watch::channel(false);
            let refined = notebook::flow::run(
                &ledger,
                &mut model,
                &destination,
                input(&ledger, Uuid::new_v4(), Some(prior)),
                next,
                rx,
            );
            assert_eq!(refined.status, writer_flow::Status::Exported);
            assert_eq!(model.0, 4);
            assert_eq!(api.count("POST"), 1);
            assert_eq!(api.count("PATCH"), 1);
            assert_eq!(api.state.lock().unwrap().notes.len(), 1);
        }
    }
}
fn wire(id: &str, content: &str, version: u64) -> Value {
    json!({"id":id,"title":content.lines().next().unwrap_or("").trim_start_matches("# "),
        "content":content,"version":version,"trashed_at":null})
}
#[derive(Default)]
struct RemoteState {
    notes: BTreeMap<String, Value>,
    requests: Vec<(String, String, Value)>,
    lose_ack: bool,
    incomplete: bool,
    conflict_patch: bool,
    cancel_post: Option<watch::Sender<bool>>,
    block_post: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
}
struct Api {
    origin: String,
    state: Arc<Mutex<RemoteState>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Api {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(RemoteState::default()));
        let shared = state.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let (mut socket, _) = listener.accept().unwrap();
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let (method, path, body) = request(&mut socket);
                let mut state = shared.lock().unwrap();
                state
                    .requests
                    .push((method.clone(), path.clone(), body.clone()));
                let (status, reply) = if method == "GET" && path.starts_with("/v2/notes?") {
                    if state.incomplete {
                        (200, json!({"results":[],"total":2001,"next_page":"more"}))
                    } else {
                        let query = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
                        let pairs = query.query_pairs().collect::<BTreeMap<_, _>>();
                        assert_eq!(pairs.get("limit").map(|v| v.as_ref()), Some("100"));
                        assert_eq!(
                            pairs.get("order_by").map(|v| v.as_ref()),
                            Some("created_at")
                        );
                        let offset = pairs.get("page").map_or(0, |value| {
                            value
                                .strip_prefix("fixture /+? ")
                                .unwrap()
                                .parse::<usize>()
                                .unwrap()
                        });
                        let next_page = if offset + 100 < state.notes.len() {
                            Some(format!("fixture /+? {}", offset + 100))
                        } else {
                            None
                        };
                        (
                            200,
                            json!({"results":state.notes.values().map(|note|
                            json!({"id":note["id"],"title":note["title"]})).skip(offset).take(100).collect::<Vec<_>>(),
                            "total":state.notes.len(),"next_page":next_page}),
                        )
                    }
                } else if method == "GET" {
                    match path
                        .strip_prefix("/v2/notes/")
                        .and_then(|id| state.notes.get(id))
                    {
                        Some(note) => (200, note.clone()),
                        None => (404, json!({"error":"not_found"})),
                    }
                } else if method == "POST" && path == "/v2/notes" {
                    let id = body["id"].as_str().unwrap();
                    let content = body["content"].as_str().unwrap();
                    assert!(
                        !state.notes.contains_key(id),
                        "fixture refuses duplicate create"
                    );
                    let note = wire(id, content, 1);
                    state.notes.insert(id.into(), note.clone());
                    (200, note)
                } else if method == "PATCH" {
                    let id = path.strip_prefix("/v2/notes/").unwrap();
                    if state.conflict_patch {
                        let old = state.notes.get_mut(id).unwrap();
                        old["version"] = json!(old["version"].as_u64().unwrap() + 1);
                        old["content"] =
                            json!(format!("{}\nUser edit", old["content"].as_str().unwrap()));
                        state.conflict_patch = false;
                    }
                    let current = state.notes.get(id).unwrap();
                    if current["version"] != body["version"] {
                        (409, json!({"error":"version_conflict"}))
                    } else {
                        let next = wire(
                            id,
                            body["content"].as_str().unwrap(),
                            current["version"].as_u64().unwrap() + 1,
                        );
                        state.notes.insert(id.into(), next.clone());
                        (200, next)
                    }
                } else {
                    panic!("unexpected fixture request: {method} {path}");
                };
                let drop_reply = method == "POST" && std::mem::take(&mut state.lose_ack);
                let cancel = if method == "POST" {
                    state.cancel_post.take()
                } else {
                    None
                };
                let block = if method == "POST" {
                    state.block_post.take()
                } else {
                    None
                };
                drop(state);
                if let Some((ready, release)) = block {
                    ready.send(()).unwrap();
                    if release.recv_timeout(Duration::from_secs(5)).is_err() {
                        continue;
                    }
                }
                if let Some(cancel) = cancel {
                    cancel.send(true).unwrap();
                    continue;
                }
                if drop_reply {
                    continue;
                }
                let bytes = serde_json::to_vec(&reply).unwrap();
                let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len());
                let _ = socket
                    .write_all(header.as_bytes())
                    .and_then(|_| socket.write_all(&bytes));
            }
        });
        Self {
            origin,
            state,
            stop,
            thread: Some(worker),
        }
    }
    fn client(&self) -> MemDestination {
        MemDestination::loopback_fixture(&self.origin, "fixture-token").unwrap()
    }
    fn count(&self, method: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(m, _, _)| m == method)
            .count()
    }
    fn lists(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(_, p, _)| p.starts_with("/v2/notes?"))
            .count()
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(
            self.origin
                .trim_start_matches("http://")
                .trim_end_matches('/'),
        );
        let result = self.thread.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
fn request(socket: &mut TcpStream) -> (String, String, Value) {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let end = loop {
        let count = socket.read(&mut chunk).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() <= 256 * 1024 + 64 * 1024);
        if let Some(at) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..end]).unwrap();
    let mut line = headers.lines().next().unwrap().split_whitespace();
    let method = line.next().unwrap().to_owned();
    let path = line.next().unwrap().to_owned();
    assert!(headers
        .lines()
        .any(|line| line.eq_ignore_ascii_case("authorization: Bearer fixture-token")));
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(length <= 256 * 1024);
    while bytes.len() < end + length {
        let count = socket.read(&mut chunk).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[end..end + length]).unwrap()
    };
    (method, path, body)
}

#[tokio::test]
async fn real_mem_create_find_update_reopen_and_old_retry_keep_one_note() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let client = api.client();
    let original = prepared(conversation);
    // Exercise actual chronological pagination and reserved cursor bytes before
    // creation and identity discovery, rather than a one-page absence shortcut.
    for index in 0..101 {
        let id = Uuid::new_v4().to_string();
        api.state.lock().unwrap().notes.insert(
            id.clone(),
            wire(&id, &format!("# Unrelated fixture {index}\n\nSample"), 1),
        );
    }
    let (_sender, canceled) = watch::channel(false);
    let first = note_export::export(&ledger, &client, &original, canceled.clone()).await;
    assert_eq!(first.status, ExportStatus::VerifiedCurrent);
    let native = first.native_note.unwrap();
    assert!(matches!(
        client
            .discover(original.correlation, Budget::default())
            .await
            .unwrap(),
        Discovery::Unique(_)
    ));
    assert_eq!(
        api.lists(),
        4,
        "both create discovery and explicit find must finish two pages"
    );
    drop(ledger);
    let ledger = Ledger::new(local.open());
    let mut next = original.clone();
    next.effect_operation = Uuid::new_v4();
    next.source_revision = "r2".into();
    next.markdown.push_str("\nNew source text.");
    let lists = api.lists();
    let second = note_export::export(&ledger, &client, &next, canceled.clone()).await;
    assert_eq!(second.status, ExportStatus::VerifiedCurrent);
    assert_eq!(second.native_note.as_deref(), Some(native.as_str()));
    assert_eq!(api.lists(), lists, "known-ID update must not scan account");
    let mut unchanged = next.clone();
    unchanged.effect_operation = Uuid::new_v4();
    unchanged.source_revision = "r3-same-prepared-body".into();
    assert_eq!(
        note_export::export(&ledger, &client, &unchanged, canceled.clone())
            .await
            .status,
        ExportStatus::VerifiedCurrent
    );
    assert_eq!(
        api.count("PATCH"),
        1,
        "already-current body must not cause another write"
    );
    assert!(api.state.lock().unwrap().notes[&native]["content"]
        .as_str()
        .unwrap()
        .ends_with(&next.markdown));
    let calls = api.state.lock().unwrap().requests.len();
    assert_eq!(
        note_export::export(&ledger, &client, &original, canceled)
            .await
            .status,
        ExportStatus::HistoricalCompleted
    );
    assert_eq!(api.state.lock().unwrap().requests.len(), calls);
    let mut changed_retry = original.clone();
    changed_retry
        .markdown
        .push_str("different operation content");
    let (_retry_sender, retry_cancel) = watch::channel(false);
    assert_eq!(
        note_export::export(&ledger, &client, &changed_retry, retry_cancel)
            .await
            .status,
        ExportStatus::Conflict
    );
    assert_eq!(api.state.lock().unwrap().requests.len(), calls);
    assert_eq!(api.count("POST"), 1);
    assert_eq!(api.count("PATCH"), 1);
    let association = ledger
        .export_association(conversation, original.correlation)
        .unwrap()
        .unwrap();
    assert_eq!(association.source_revision, "r3-same-prepared-body");
    assert_eq!(
        association.title_marker(),
        format!("[RMB:{}]", original.correlation)
    );
    assert_eq!(
        ledger
            .historical_export_attempt(
                conversation,
                original.correlation,
                original.effect_operation
            )
            .unwrap()
            .unwrap()
            .outcome,
        Outcome::Completed
    );
}

#[tokio::test]
async fn lost_ack_and_post_dispatch_cancel_reconcile_known_id_without_resending() {
    for cancel_after_send in [false, true] {
        let local = LocalStore::new();
        let (ledger, conversation) = local.ledger();
        let api = Api::new();
        let client = api.client();
        let note = prepared(conversation);
        let (sender, canceled) = watch::channel(false);
        if cancel_after_send {
            api.state.lock().unwrap().cancel_post = Some(sender.clone());
        } else {
            api.state.lock().unwrap().lose_ack = true;
        }
        assert_eq!(
            note_export::export(&ledger, &client, &note, canceled)
                .await
                .status,
            ExportStatus::ReconcileRequired
        );
        let mut premature = note.clone();
        premature.effect_operation = Uuid::new_v4();
        premature.source_revision = "later".into();
        let (_fresh_sender, fresh_cancel) = watch::channel(false);
        assert_eq!(
            note_export::export(&ledger, &client, &premature, fresh_cancel.clone())
                .await
                .status,
            ExportStatus::ReconcileRequired
        );
        assert_eq!(api.count("POST"), 1);
        drop(ledger);
        let ledger = Ledger::new(local.open());
        let lists = api.lists();
        assert_eq!(
            note_export::reconcile(
                &ledger,
                &client,
                conversation,
                note.correlation,
                fresh_cancel
            )
            .await
            .status,
            ExportStatus::VerifiedCurrent
        );
        assert_eq!(api.lists(), lists);
        assert_eq!(api.count("POST"), 1);
    }
}

#[tokio::test]
async fn before_send_cancellation_records_no_effect_and_allows_new_deliberate_operation() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let client = api.client();
    let note = prepared(conversation);
    let (sender, canceled) = watch::channel(true);
    assert_eq!(
        note_export::export(&ledger, &client, &note, canceled)
            .await
            .status,
        ExportStatus::CanceledNoEffect
    );
    let old = ledger
        .export_association(conversation, note.correlation)
        .unwrap()
        .unwrap();
    assert!(old.attempt.unwrap().no_send);
    assert_eq!(api.count("GET") + api.count("POST"), 0);
    sender.send(false).unwrap_err(); // The first invocation released its receiver.
    let (_next_sender, next_cancel) = watch::channel(false);
    let mut next = note.clone();
    next.effect_operation = Uuid::new_v4();
    assert_eq!(
        note_export::export(&ledger, &client, &next, next_cancel.clone())
            .await
            .status,
        ExportStatus::VerifiedCurrent
    );
    assert_eq!(
        note_export::export(&ledger, &client, &note, next_cancel)
            .await
            .status,
        ExportStatus::CanceledNoEffect
    );
    assert_eq!(api.count("POST"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_ledger_over_same_store_is_busy_until_live_sender_releases() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let store = ledger.store().clone();
    let api = Api::new();
    let client = Arc::new(api.client());
    let note = prepared(conversation);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    api.state.lock().unwrap().block_post = Some((ready_tx, release_rx));
    let (_sender, canceled) = watch::channel(false);
    let first_cancel = canceled.clone();
    let first = note_export::export(&ledger, &*client, &note, first_cancel);
    tokio::pin!(first);
    let ready =
        tokio::task::spawn_blocking(move || ready_rx.recv_timeout(Duration::from_secs(3)).unwrap());
    tokio::select! {
        ready = ready => { ready.unwrap(); }
        result = &mut first => { panic!("sender ended before held response: {:?}", result.status); }
    }
    let second = Ledger::new(store);
    assert_eq!(
        note_export::reconcile(
            &second,
            &*client,
            conversation,
            note.correlation,
            canceled.clone()
        )
        .await
        .status,
        ExportStatus::Busy
    );
    assert_eq!(
        note_export::export(&second, &*client, &note, canceled)
            .await
            .status,
        ExportStatus::Busy
    );
    assert_eq!(api.count("POST"), 1);
    release_tx.send(()).unwrap();
    assert_eq!(first.await.status, ExportStatus::VerifiedCurrent);
}

#[tokio::test]
async fn exact_marker_duplicates_incomplete_listing_and_missing_marker_never_write() {
    for incomplete in [false, true] {
        let local = LocalStore::new();
        let (ledger, conversation) = local.ledger();
        let api = Api::new();
        let client = api.client();
        let note = prepared(conversation);
        if incomplete {
            api.state.lock().unwrap().incomplete = true;
        } else {
            for _ in 0..2 {
                let id = Uuid::new_v4().to_string();
                api.state.lock().unwrap().notes.insert(
                    id.clone(),
                    wire(&id, &note.render(CorrelationLocation::Title).unwrap(), 1),
                );
            }
        }
        let (_sender, canceled) = watch::channel(false);
        assert_eq!(
            note_export::export(&ledger, &client, &note, canceled)
                .await
                .status,
            if incomplete {
                ExportStatus::Incomplete
            } else {
                ExportStatus::Conflict
            }
        );
        assert_eq!(api.count("POST"), 0);
        assert!(
            ledger
                .export_association(conversation, note.correlation)
                .unwrap()
                .unwrap()
                .attempt
                .unwrap()
                .no_send
        );
    }
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let client = api.client();
    let mut note = prepared(conversation);
    let (_sender, canceled) = watch::channel(false);
    let created = note_export::export(&ledger, &client, &note, canceled.clone()).await;
    api.state
        .lock()
        .unwrap()
        .notes
        .get_mut(&created.native_note.unwrap())
        .unwrap()["title"] = json!("Unrelated title");
    note.effect_operation = Uuid::new_v4();
    note.source_revision = "r2".into();
    assert_eq!(
        note_export::export(&ledger, &client, &note, canceled)
            .await
            .status,
        ExportStatus::Conflict
    );
    assert_eq!(api.count("PATCH"), 0);
}

#[tokio::test]
async fn remote_edits_and_conditional_version_races_preserve_external_content() {
    for between_read_and_patch in [false, true] {
        let local = LocalStore::new();
        let (ledger, conversation) = local.ledger();
        let api = Api::new();
        let client = api.client();
        let mut note = prepared(conversation);
        let (_sender, canceled) = watch::channel(false);
        let first = note_export::export(&ledger, &client, &note, canceled.clone()).await;
        let native = first.native_note.unwrap();
        if between_read_and_patch {
            api.state.lock().unwrap().conflict_patch = true;
        } else {
            let mut state = api.state.lock().unwrap();
            let remote = state.notes.get_mut(&native).unwrap();
            remote["content"] = json!(format!(
                "{}\nUser edit",
                remote["content"].as_str().unwrap()
            ));
            remote["version"] = json!(2);
        }
        note.effect_operation = Uuid::new_v4();
        note.source_revision = "r2".into();
        note.markdown = "replacement must not erase user edit".into();
        let result = note_export::export(&ledger, &client, &note, canceled.clone()).await;
        assert_eq!(
            result.status,
            if between_read_and_patch {
                ExportStatus::ReconcileRequired
            } else {
                ExportStatus::Conflict
            }
        );
        assert!(api.state.lock().unwrap().notes[&native]["content"]
            .as_str()
            .unwrap()
            .ends_with("User edit"));
        if between_read_and_patch {
            assert_eq!(
                note_export::reconcile(&ledger, &client, conversation, note.correlation, canceled)
                    .await
                    .status,
                ExportStatus::Conflict
            );
        }
        assert_eq!(api.count("PATCH"), usize::from(between_read_and_patch));
    }
}

#[derive(Default)]
struct MetadataDestination(Mutex<BTreeMap<String, RemoteNote>>);
impl NoteDestination for MetadataDestination {
    fn backend(&self) -> &str {
        "metadata-fixture"
    }
    fn correlation_location(&self) -> CorrelationLocation {
        CorrelationLocation::Metadata
    }
    async fn discover(&self, correlation: Uuid, _budget: Budget) -> ExportResult<Discovery> {
        let matches = self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|n| {
                n.metadata.get("remarkable_buddies_export_id") == Some(&correlation.to_string())
            })
            .cloned()
            .collect::<Vec<_>>();
        Ok(match matches.len() {
            0 => Discovery::Missing,
            1 => Discovery::Unique(matches[0].clone()),
            _ => Discovery::Duplicate,
        })
    }
    async fn read(&self, id: &str, _budget: Budget) -> ExportResult<Option<RemoteNote>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    async fn create(
        &self,
        id: &str,
        content: &str,
        correlation: Uuid,
        _budget: Budget,
    ) -> ExportResult<RemoteNote> {
        let note = RemoteNote {
            id: id.into(),
            title: "Plain metadata title".into(),
            content: content.into(),
            version: 1,
            trashed: false,
            metadata: BTreeMap::from([(
                "remarkable_buddies_export_id".into(),
                correlation.to_string(),
            )]),
        };
        self.0.lock().unwrap().insert(id.into(), note.clone());
        Ok(note)
    }
    async fn update(
        &self,
        id: &str,
        content: &str,
        version: u64,
        _correlation: Uuid,
        _budget: Budget,
    ) -> ExportResult<RemoteNote> {
        let mut notes = self.0.lock().unwrap();
        let note = notes.get_mut(id).unwrap();
        assert_eq!(note.version, version);
        note.version += 1;
        note.content = content.into();
        Ok(note.clone())
    }
}
#[tokio::test]
async fn replaceable_metadata_destination_roundtrip_keeps_title_plain() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let destination = MetadataDestination::default();
    let mut note = prepared(conversation);
    let (_sender, canceled) = watch::channel(false);
    let created = note_export::export(&ledger, &destination, &note, canceled.clone()).await;
    assert_eq!(created.status, ExportStatus::VerifiedCurrent);
    assert!(matches!(
        destination
            .discover(note.correlation, Budget::default())
            .await
            .unwrap(),
        Discovery::Unique(_)
    ));
    drop(ledger);
    let ledger = Ledger::new(local.open());
    note.effect_operation = Uuid::new_v4();
    note.source_revision = "r2".into();
    assert_eq!(
        note_export::export(&ledger, &destination, &note, canceled)
            .await
            .native_note,
        created.native_note
    );
    assert_eq!(destination.0.lock().unwrap().len(), 1);
    assert!(!destination
        .0
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .title
        .contains("[RMB:"));
}

#[test]
fn command_reports_missing_credential_without_starting_reader_or_echoing_content() {
    use std::process::{Command, Stdio};
    let secret_content = "private-command-note-sentinel";
    let mut note = prepared(Uuid::new_v4());
    note.markdown = secret_content.into();
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .arg("--export-note")
        .env_remove("MEM_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::to_vec(&json!({"command":"export","note":note}))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["failure"], "missing-credentials");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret_content));
    assert!(
        output.stderr.is_empty(),
        "export command must not start Reader/provider logging"
    );
}

#[tokio::test]
async fn export_domain_upgrade_refuses_other_namespaces_and_cross_schema_payloads() {
    use remarkable_reader_buddy::storage::Namespace;
    for domain in [1, 2] {
        let local = LocalStore::new();
        let (ledger, conversation) = local.ledger();
        let api = Api::new();
        let client = api.client();
        let mut note = prepared(conversation);
        let (_sender, canceled) = watch::channel(true);
        assert_eq!(
            note_export::export(&ledger, &client, &note, canceled)
                .await
                .status,
            ExportStatus::CanceledNoEffect
        );
        let original = ledger
            .store()
            .heads(Namespace::ExportAssociation, note.correlation)
            .unwrap()
            .pop()
            .unwrap();
        let mut unsupported = original.clone();
        unsupported.namespace = Namespace::SubjectMemory;
        unsupported.record_id = Uuid::new_v4();
        unsupported.revision_id = Uuid::new_v4();
        unsupported.operation_id = Uuid::new_v4();
        unsupported.parents.clear(); // Isolate version refusal from lineage errors.
        assert!(ledger
            .store()
            .commit(vec![unsupported], BTreeMap::new())
            .is_err());
        let mut wrong_shape = original.clone();
        wrong_shape.domain_schema_version = domain;
        wrong_shape.revision_id = Uuid::new_v4();
        wrong_shape.operation_id = Uuid::new_v4();
        wrong_shape.parents = [original.revision_id].into();
        if domain == 2 {
            wrong_shape.payload["record"]
                .as_object_mut()
                .unwrap()
                .remove("attempt");
        }
        // Generic storage remains payload-opaque; the domain adapter must refuse.
        ledger
            .store()
            .commit(vec![wrong_shape], BTreeMap::new())
            .unwrap();
        assert!(ledger
            .export_association(conversation, note.correlation)
            .is_err());
        drop(ledger);
        let ledger = Ledger::new(local.open());
        note.effect_operation = Uuid::new_v4();
        let (_next_sender, fresh) = watch::channel(false);
        assert_eq!(
            note_export::export(&ledger, &client, &note, fresh)
                .await
                .status,
            ExportStatus::Refused
        );
        assert_eq!(api.count("GET") + api.count("POST") + api.count("PATCH"), 0);
    }
}

#[tokio::test]
async fn export_successor_keeps_expected_heads_and_old_stage_receipts() {
    use remarkable_reader_buddy::{
        conversation::{ExpectedHeads, ExportAction},
        storage::Namespace,
    };
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let client = api.client();
    let note = prepared(conversation);
    let (_sender, canceled) = watch::channel(true);
    assert_eq!(
        note_export::export(&ledger, &client, &note, canceled)
            .await
            .status,
        ExportStatus::CanceledNoEffect
    );
    let old = ledger
        .export_association(conversation, note.correlation)
        .unwrap()
        .unwrap();
    let before = ledger.store().manifests().unwrap().len();
    let mut next = old.clone();
    next.source_revision = "r2".into();
    next.outcome = Outcome::Prepared;
    let attempt = next.attempt.as_mut().unwrap();
    attempt.id = Uuid::new_v4();
    attempt.no_send = false;
    attempt.target = Uuid::new_v4().to_string();
    assert_eq!(attempt.action, ExportAction::Create);
    next.operation = attempt.stage_id("prepared");
    let root = ledger
        .expected(Namespace::Conversation, conversation)
        .unwrap();
    assert!(ledger
        .associate_export(next.operation, root, ExpectedHeads::default(), next)
        .is_err());
    assert_eq!(ledger.store().manifests().unwrap().len(), before);
    let historical = ledger
        .associate_export(
            old.operation,
            ExpectedHeads::default(),
            ExpectedHeads::default(),
            old.clone(),
        )
        .unwrap();
    assert_eq!(historical.historical.operation, old.operation);
    assert_eq!(
        ledger
            .export_association(conversation, note.correlation)
            .unwrap()
            .unwrap(),
        old
    );
    assert_eq!(api.count("POST"), 0);
}

#[tokio::test]
async fn lost_local_prepare_ack_never_becomes_fresh_dispatch_on_retry_or_restart() {
    use remarkable_reader_buddy::storage::Fault;
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let api = Api::new();
    let client = api.client();
    let note = prepared(conversation);
    ledger.store().set_fault(Fault::AfterCommit).unwrap();
    let (_sender, canceled) = watch::channel(false);
    assert_eq!(
        note_export::export(&ledger, &client, &note, canceled.clone())
            .await
            .status,
        ExportStatus::ReconcileRequired
    );
    assert_eq!(api.count("POST"), 0);
    let manifests = ledger.store().manifests().unwrap().len();
    let pending = ledger
        .export_association(conversation, note.correlation)
        .unwrap()
        .unwrap();
    assert_eq!(pending.outcome, Outcome::Prepared);
    assert!(
        !pending.attempt.unwrap().no_send,
        "missing send flag is not a no-send proof"
    );
    drop(ledger);
    let ledger = Ledger::new(local.open());
    assert_eq!(
        note_export::export(&ledger, &client, &note, canceled)
            .await
            .status,
        ExportStatus::ReconcileRequired
    );
    assert_eq!(api.count("POST"), 0);
    assert_eq!(api.lists(), 0);
    assert_eq!(ledger.store().manifests().unwrap().len(), manifests);
}

struct CancelBeforePoll<'a> {
    destination: &'a MetadataDestination,
    cancel: &'a watch::Sender<bool>,
    polled: &'a AtomicBool,
}
impl NoteDestination for CancelBeforePoll<'_> {
    fn backend(&self) -> &str {
        self.destination.backend()
    }
    fn correlation_location(&self) -> CorrelationLocation {
        CorrelationLocation::Metadata
    }
    async fn discover(&self, id: Uuid, budget: Budget) -> ExportResult<Discovery> {
        self.destination.discover(id, budget).await
    }
    async fn read(&self, id: &str, budget: Budget) -> ExportResult<Option<RemoteNote>> {
        self.destination.read(id, budget).await
    }
    fn create(
        &self,
        id: &str,
        body: &str,
        correlation: Uuid,
        budget: Budget,
    ) -> impl std::future::Future<Output = ExportResult<RemoteNote>> + Send {
        // Deterministically select cancellation between coordinator precheck
        // and first future poll. No destination write occurs in construction.
        self.cancel.send(true).unwrap();
        async move {
            self.polled.store(true, Ordering::Release);
            self.destination.create(id, body, correlation, budget).await
        }
    }
    async fn update(
        &self,
        id: &str,
        body: &str,
        version: u64,
        correlation: Uuid,
        budget: Budget,
    ) -> ExportResult<RemoteNote> {
        self.destination
            .update(id, body, version, correlation, budget)
            .await
    }
}

#[tokio::test]
async fn cancellation_before_first_write_poll_is_no_send_and_can_be_deliberately_reexported() {
    let local = LocalStore::new();
    let (ledger, conversation) = local.ledger();
    let destination = MetadataDestination::default();
    let mut note = prepared(conversation);
    let (sender, canceled) = watch::channel(false);
    let polled = AtomicBool::new(false);
    let wrapper = CancelBeforePoll {
        destination: &destination,
        cancel: &sender,
        polled: &polled,
    };
    assert_eq!(
        note_export::export(&ledger, &wrapper, &note, canceled)
            .await
            .status,
        ExportStatus::CanceledNoEffect
    );
    assert!(!polled.load(Ordering::Acquire));
    assert!(destination.0.lock().unwrap().is_empty());
    assert!(
        ledger
            .export_association(conversation, note.correlation)
            .unwrap()
            .unwrap()
            .attempt
            .unwrap()
            .no_send
    );
    note.effect_operation = Uuid::new_v4();
    let (_next_sender, next_cancel) = watch::channel(false);
    assert_eq!(
        note_export::export(&ledger, &destination, &note, next_cancel)
            .await
            .status,
        ExportStatus::VerifiedCurrent
    );
    assert_eq!(destination.0.lock().unwrap().len(), 1);
}

/// Both actual HTTP adapters participate in one production Writer flow.
#[test]
fn joined_writer_actual_model_and_mem_http_create_restart_refine_update() {
    use remarkable_reader_buddy::{
        config::Personalization,
        conversation::TextScope,
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
        writer, writer_flow, OpenAI,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let model_worker = thread::spawn(move || {
        for body in [
            "# Study\n\nx = 7 (page 1).",
            "# Study\n\nx = 7 (page 1). Refined without a deadline.",
        ] {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let (method, path, payload) = request(&mut socket);
            assert_eq!(method, "POST");
            assert_eq!(path, "/v1/chat/completions");
            assert!(payload["messages"].is_array());
            let reply = json!({"choices":[{"message":{"content":body}}]}).to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",reply.len(),reply).unwrap();
        }
    });
    let api = Api::new();
    let local = LocalStore::new();
    let (ledger, id) = local.ledger();
    let correlation = Uuid::new_v4();
    let generation = writer_flow::Request::GenerateAndExport {
        outer_operation: Uuid::new_v4(),
        correlation,
        effect_operation: Uuid::new_v4(),
        generation: writer::Request::Generate {
            conversation: id,
            operation: Uuid::new_v4(),
            request: "Preserve the equation and cite page 1.".into(),
            document: NormalizedDocument {
                document: Uuid::new_v4(),
                format: SourceFormat::Notebook,
                source_sha256: "a".repeat(64),
                extractor_revision: "loopback-v1".into(),
                units: vec![SourceUnit {
                    page: Some(1),
                    section: None,
                    text: "x = 7".into(),
                }],
            },
            selection: TextScope::SuppliedNotebook,
            guidance: None,
        },
    };
    let mut engine = OpenAI::new("fixture-model".into(), "fixture-token".into(), Some(origin))
        .with_timeout(Duration::from_secs(5));
    let preference = Personalization::default().snapshot().unwrap();
    let (_tx, rx) = watch::channel(false);
    let first = writer_flow::run(
        &ledger,
        &mut engine,
        &api.client(),
        &generation,
        &preference,
        rx,
    );
    assert_eq!(first.status, writer_flow::Status::Exported);
    let first_id = first.export.unwrap().native_note.unwrap();
    let draft = first.draft.unwrap();
    drop(ledger);
    let reopened = Ledger::new(local.open());
    let refinement = writer_flow::Request::RefineAndExport {
        outer_operation: Uuid::new_v4(),
        correlation,
        effect_operation: Uuid::new_v4(),
        refinement: writer::Request::Refine {
            conversation: id,
            operation: Uuid::new_v4(),
            request: "Explicitly say no deadline.".into(),
            draft,
        },
    };
    let (_tx, rx) = watch::channel(false);
    let second = writer_flow::run(
        &reopened,
        &mut engine,
        &api.client(),
        &refinement,
        &preference,
        rx,
    );
    assert_eq!(second.status, writer_flow::Status::Exported);
    assert_eq!(
        second.export.unwrap().native_note.as_deref(),
        Some(first_id.as_str())
    );
    assert!(second.observation.write_polled);
    assert_eq!(api.count("POST"), 1);
    assert_eq!(api.count("PATCH"), 1);
    assert_eq!(api.lists(), 1);
    let before = api.state.lock().unwrap().requests.len();
    let (_tx, rx) = watch::channel(false);
    let replay = writer_flow::run(
        &reopened,
        &mut engine,
        &api.client(),
        &refinement,
        &preference,
        rx,
    );
    assert_eq!(replay.status, writer_flow::Status::Historical);
    assert_eq!(api.state.lock().unwrap().requests.len(), before);
    model_worker.join().unwrap();
}
