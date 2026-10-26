use remarkable_reader_buddy::{
    config::Personalization,
    conversation::{Ledger, Mode, Outcome, Record, Role, TextScope, Turn},
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::{Fault, Namespace, Store, StorePaths, Uuid},
    writer::{self, Request, Status},
    LLMEngine, OpenAI,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
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

struct Local(PathBuf);
impl Local {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("rmb-writer-{}", Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
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
impl Drop for Local {
    fn drop(&mut self) {
        let p = self.0.canonicalize().unwrap();
        assert_eq!(
            p.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rmb-writer-"));
        std::fs::remove_dir_all(p).unwrap();
    }
}
struct Reply {
    body: Option<String>,
    hook: Option<Box<dyn FnOnce() + Send>>,
}
struct Api {
    origin: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Api {
    fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let mut replies: VecDeque<_> = replies.into();
            while !stopped.load(Ordering::Acquire) {
                let (mut socket, _) = listener.accept().unwrap();
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 4096];
                let end = loop {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    assert!(bytes.len() < 1024 * 1024);
                };
                let header = String::from_utf8_lossy(&bytes[..end]);
                assert!(header.starts_with("POST /v1/chat/completions "));
                let size: usize = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < end + size {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(&bytes[end..end + size]).unwrap());
                let Reply { body, hook } = replies
                    .pop_front()
                    .expect("unexpected additional provider dispatch");
                if let Some(hook) = hook {
                    hook()
                }
                if let Some(body) = body {
                    let response = json!({"choices":[{"message":{"content":body}}]}).to_string();
                    let _=write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response);
                }
            }
        });
        Self {
            origin,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn model(&self) -> OpenAI {
        OpenAI::new(
            "fixture-model".into(),
            "fixture-key".into(),
            Some(self.origin.clone()),
        )
        .with_timeout(Duration::from_secs(5))
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.origin.trim_start_matches("http://"));
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap()
            }
        }
    }
}
fn reply(body: &str) -> Reply {
    Reply {
        body: Some(body.into()),
        hook: None,
    }
}
fn request(conversation: Uuid) -> Request {
    Request::Generate {
        conversation,
        operation: Uuid::new_v4(),
        request: "Keep x = 7 exactly.  Do not invent a deadline.\nMake study notes.".into(),
        document: NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "fixture-text-v1".into(),
            units: vec![
                SourceUnit {
                    page: Some(1),
                    section: None,
                    text: "Equation: x = 7. Review on Tuesday.".into(),
                },
                SourceUnit {
                    page: Some(2),
                    section: None,
                    text: "Second page: z = 9. No deadline.".into(),
                },
            ],
        },
        selection: TextScope::SuppliedNotebook,
        guidance: Some("Preserve equations; use useful page citations.".into()),
    }
}
fn preference() -> remarkable_reader_buddy::config::PersonalizationSnapshot {
    Personalization::default().snapshot().unwrap()
}
fn payload(api: &Api, n: usize) -> Value {
    let lock = api.requests.lock().unwrap();
    let parts = lock[n]["messages"][0]["content"].as_array().unwrap();
    serde_json::from_str(parts.last().unwrap()["text"].as_str().unwrap()).unwrap()
}

#[test]
fn actual_transport_generate_reopen_refine_and_prepare_export_keep_exact_sources() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let req = request(id);
    let op = req.operation();
    let api = Api::new(vec![
        reply("# Study\n\nx = 7 (page 1); z = 9 (page 2)."),
        reply("# Study\n\nx = 7; z = 9. No invented deadline. (pages 1–2)"),
    ]);
    let first = writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
        false
    });
    assert_eq!(first.status, Status::Generated);
    let draft = first.draft.unwrap();
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            false
        })
        .status,
        Status::HistoricalGenerated
    );
    assert_eq!(api.count(), 1);
    let p = payload(&api, 0);
    if let Request::Generate { request, .. } = &req {
        assert_eq!(p["request"], *request)
    }
    assert_eq!(
        p["source_units"][0]["text"],
        "Equation: x = 7. Review on Tuesday."
    );
    assert_eq!(p["source_units"].as_array().unwrap().len(), 2);
    let records = ledger.inspect(id, false).unwrap();
    let source = records
        .iter()
        .find_map(|r| {
            if let Record::NormalizedText(s) = r {
                Some(s.clone())
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(source.document.units.len(), 2);
    let heads = ledger.store().heads(Namespace::Source, source.id).unwrap();
    assert_eq!(heads[0].domain_schema_version, 2);
    drop(ledger);
    let reopened = Ledger::new(local.open());
    let refinement = Request::Refine {
        conversation: id,
        operation: Uuid::new_v4(),
        request: "Keep my original wording; explicitly say no invented deadline.".into(),
        draft,
    };
    let refined = writer::run(
        &reopened,
        &mut api.model(),
        &refinement,
        &preference(),
        &mut || false,
    );
    assert_eq!(refined.status, Status::Generated);
    let p = payload(&api, 1);
    assert_eq!(
        p["selected_draft"],
        "# Study\n\nx = 7 (page 1); z = 9 (page 2)."
    );
    assert_eq!(p["history"].as_array().unwrap().len(), 1);
    assert_eq!(p["history"][0]["mode"], "writer");
    let records = reopened.inspect(id, false).unwrap();
    let turns: Vec<_> = records
        .iter()
        .filter_map(|r| {
            if let Record::Turn(t) = r {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(turns.len(), 4);
    assert!(turns.iter().all(|t| t.outcome != Outcome::Completed));
    assert_eq!(turns[2].correction_of, Some(draft));
    assert_eq!(turns[2].sources, turns[0].sources);
    let export = Request::PrepareExport {
        conversation: id,
        draft: refined.draft.unwrap(),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
    };
    let artifact = writer::run(
        &reopened,
        &mut api.model(),
        &export,
        &preference(),
        &mut || false,
    );
    assert_eq!(artifact.status, Status::PreparedExport);
    assert_eq!(
        artifact.prepared_note.unwrap().markdown,
        "x = 7; z = 9. No invented deadline. (pages 1–2)"
    );
    assert_eq!(api.count(), 2);
    assert!(
        reopened
            .context(
                id,
                &remarkable_reader_buddy::conversation::ContextBudget {
                    max_text_bytes: 10000,
                    max_turns: 10,
                    provider_token_limit: Some(10000)
                },
                |_| Ok(100)
            )
            .unwrap()
            .normalized_text
            .len()
            == 1
    );
    // Same old receipt after later refinement remains historical, no third call.
    assert_eq!(
        writer::run(
            &reopened,
            &mut api.model(),
            &req,
            &preference(),
            &mut || false
        )
        .status,
        Status::HistoricalGenerated
    );
    assert_eq!(api.count(), 2);
    assert_eq!(draft, writer::stage(op, "draft"));
}

#[test]
fn page_scope_guidance_personalization_and_preflight_bounds_are_explicit() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let mut req = request(id);
    if let Request::Generate { selection, .. } = &mut req {
        *selection = TextScope::Page { page: 2 }
    }
    let api = Api::new(vec![reply("# Page\n\nz = 9 (page 2).")]);
    let pref = Personalization {
        enabled: true,
        text: "Use concise equations.".into(),
        ..Default::default()
    }
    .snapshot()
    .unwrap();
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &req, &pref, &mut || false).status,
        Status::Generated
    );
    let p = payload(&api, 0);
    assert_eq!(p["source_units"].as_array().unwrap().len(), 1);
    assert_eq!(p["source_units"][0]["page"], 2);
    assert_eq!(
        p["explicit_guidance"],
        "Preserve equations; use useful page citations."
    );
    assert!(api.requests.lock().unwrap()[0]
        .to_string()
        .contains("Use concise equations."));
    let mut oversized = request(id);
    if let Request::Generate { document, .. } = &mut oversized {
        document.units[0].text = "x".repeat(60 * 1024)
    }
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &oversized, &pref, &mut || false).status,
        Status::Refused
    );
    let mut absent = request(id);
    if let Request::Generate { selection, .. } = &mut absent {
        *selection = TextScope::Page { page: 99 }
    }
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &absent, &pref, &mut || false).status,
        Status::Refused
    );
    assert_eq!(api.count(), 1);
}

#[test]
fn lost_local_prepare_ack_never_dispatches_on_retry_or_reopen() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let req = request(id);
    let api = Api::new(vec![]);
    ledger.store().set_fault(Fault::AfterCommit).unwrap();
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            false
        })
        .status,
        Status::HistoricalPending
    );
    assert_eq!(api.count(), 0);
    drop(ledger);
    let ledger = Ledger::new(local.open());
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            false
        })
        .status,
        Status::HistoricalPending
    );
    assert_eq!(api.count(), 0);
}

#[test]
fn shared_store_excludes_overlapping_provider_invocations() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let store = ledger.store().clone();
    let other = Ledger::new(store);
    let req = request(id);
    let second = request(id);
    let (entered, waiting) = mpsc::channel();
    let (release, go) = mpsc::channel();
    let api = Api::new(vec![Reply {
        body: Some("# Note\n\nx = 7 (page 1).".into()),
        hook: Some(Box::new(move || {
            entered.send(()).unwrap();
            go.recv_timeout(Duration::from_secs(10)).unwrap();
        })),
    }]);
    let mut model = api.model();
    let worker =
        thread::spawn(move || writer::run(&ledger, &mut model, &req, &preference(), &mut || false));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        writer::run(
            &other,
            &mut api.model(),
            &second,
            &preference(),
            &mut || false
        )
        .status,
        Status::Busy
    );
    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap().status, Status::Generated);
    assert_eq!(api.count(), 1);
}

#[test]
fn cancellation_and_invalid_or_lost_provider_replies_never_become_drafts() {
    for case in 0..4 {
        let local = Local::new();
        let (ledger, id) = local.ledger();
        let req = request(id);
        let canceled = Arc::new(AtomicBool::new(false));
        let flag = canceled.clone();
        let body = match case {
            0 => Some("# Late\n\nMust not publish.".into()),
            1 => Some("NONE".into()),
            2 => Some(format!("# Huge\n\n{}", "x".repeat(25 * 1024))),
            _ => None,
        };
        let api = Api::new(vec![Reply {
            body,
            hook: if case == 0 {
                Some(Box::new(move || flag.store(true, Ordering::Release)))
            } else {
                None
            },
        }]);
        let report = writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            canceled.load(Ordering::Acquire)
        });
        assert_eq!(
            report.status,
            if case == 0 {
                Status::Canceled
            } else {
                Status::Failed
            }
        );
        assert!(ledger
            .inspect(id, false)
            .unwrap()
            .iter()
            .all(|r| !matches!(r,Record::Turn(t) if t.outcome==Outcome::Generated)));
        assert_eq!(
            writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
                false
            })
            .status,
            report.status
        );
        assert_eq!(api.count(), 1);
    }
}

#[test]
fn unrelated_root_change_refuses_a_late_result() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let req = request(id);
    let other = Ledger::new(ledger.store().clone());
    let api = Api::new(vec![Reply {
        body: Some("# Stale\n\nMust not publish.".into()),
        hook: Some(Box::new(move || {
            let t = Turn {
                id: Uuid::new_v4(),
                conversation: id,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::User,
                mode: Mode::Reader,
                outcome: Outcome::Interpreted,
                text: Some("Concurrent exact request".into()),
                sources: vec![],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            };
            other
                .append(
                    Uuid::new_v4(),
                    other.expected(Namespace::Conversation, id).unwrap(),
                    t,
                    vec![],
                )
                .unwrap();
        })),
    }]);
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            false
        })
        .status,
        Status::Stale
    );
    assert_eq!(api.count(), 1);
    assert!(ledger
        .inspect(id, false)
        .unwrap()
        .iter()
        .all(|r| !matches!(r,Record::Turn(t) if t.outcome==Outcome::Generated)));
}

#[test]
fn reader_request_is_preserved_and_oversized_history_refuses_without_call() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let exact = "Reader question: why is x != 3?\n  Keep {braces} and spacing.";
    let add = |text: String| {
        let t = Turn {
            id: Uuid::new_v4(),
            conversation: id,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some(text),
            sources: vec![],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        };
        ledger
            .append(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, id).unwrap(),
                t,
                vec![],
            )
            .unwrap();
    };
    add(exact.into());
    let api = Api::new(vec![reply("# Writer\n\nx = 7 (page 1).")]);
    let req = request(id);
    assert_eq!(
        writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            false
        })
        .status,
        Status::Generated
    );
    assert_eq!(payload(&api, 0)["history"][0]["text"], exact);
    add("large exact prior user request ".repeat(1000));
    let next = request(id);
    let result = writer::run(&ledger, &mut api.model(), &next, &preference(), &mut || {
        false
    });
    assert_eq!(result.status, Status::Refused);
    assert_eq!(result.failure, Some(writer::Failure::TooLarge));
    assert_eq!(api.count(), 1);
}

#[test]
fn schema_one_text_and_tampered_source_identity_refuse_without_provider_call() {
    use std::collections::BTreeMap;
    for domain in [1, 2] {
        let local = Local::new();
        let (ledger, id) = local.ledger();
        let req = request(id);
        let api = Api::new(vec![reply("# Original\n\nx = 7 (page 1).")]);
        let generated = writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
            false
        });
        assert_eq!(generated.status, Status::Generated);
        let source = ledger
            .inspect(id, false)
            .unwrap()
            .into_iter()
            .find_map(|r| {
                if let Record::NormalizedText(s) = r {
                    Some(s)
                } else {
                    None
                }
            })
            .unwrap();
        let mut e = ledger
            .store()
            .heads(Namespace::Source, source.id)
            .unwrap()
            .pop()
            .unwrap();
        e.parents = [e.revision_id].into();
        e.revision_id = Uuid::new_v4();
        e.operation_id = Uuid::new_v4();
        e.domain_schema_version = domain;
        if domain == 2 {
            e.payload["record"]["document"]["units"][0]["text"] =
                json!("tampered normalized source");
        }
        ledger.store().commit(vec![e], BTreeMap::new()).unwrap();
        let refine = Request::Refine {
            conversation: id,
            operation: Uuid::new_v4(),
            request: "Keep exact source.".into(),
            draft: generated.draft.unwrap(),
        };
        assert_eq!(
            writer::run(
                &ledger,
                &mut api.model(),
                &refine,
                &preference(),
                &mut || false
            )
            .status,
            Status::Refused
        );
        assert_eq!(api.count(), 1);
    }
}

#[test]
fn explicit_command_missing_credentials_has_no_reader_startup_or_content_echo() {
    use std::process::{Command, Stdio};
    let local = Local::new();
    let config = remarkable_reader_buddy::config::Config {
        paths: StorePaths {
            data: local.0.join("data"),
            cache: local.0.join("cache"),
            credentials: local.0.join("credentials"),
        },
        ..Default::default()
    };
    let path = local.0.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let req = request(Uuid::new_v4());
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .arg("--writer-note")
        .env("REMARKABLE_BUDDIES_CONFIG", &path)
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&req).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["failure"], "missing-credentials");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Keep x = 7"));
    assert!(output.stderr.is_empty());
    assert!(!local.0.join("data").exists());
}

#[test]
fn changed_preparation_refuses_before_real_provider_dispatch() {
    struct ChangeBeforeDispatch {
        inner: OpenAI,
        change: Option<Box<dyn FnOnce()>>,
    }
    impl LLMEngine for ChangeBeforeDispatch {
        fn add_text_content(&mut self, text: &str) {
            self.inner.add_text_content(text);
            if let Some(change) = self.change.take() {
                change()
            }
        }
        fn add_image_content(&mut self, image: &str) {
            self.inner.add_image_content(image)
        }
        fn clear_content(&mut self) {
            self.inner.clear_content()
        }
        fn execute(&mut self) -> anyhow::Result<String> {
            self.inner.execute()
        }
        fn execute_with_progress(
            &mut self,
            progress: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<String> {
            self.inner.execute_with_progress(progress)
        }
    }
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let other = Ledger::new(ledger.store().clone());
    let req = request(id);
    let api = Api::new(vec![]);
    let mut engine = ChangeBeforeDispatch {
        inner: api.model(),
        change: Some(Box::new(move || {
            let t = Turn {
                id: Uuid::new_v4(),
                conversation: id,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::User,
                mode: Mode::Reader,
                outcome: Outcome::Interpreted,
                text: Some("Changed before provider dispatch".into()),
                sources: vec![],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            };
            other
                .append(
                    Uuid::new_v4(),
                    other.expected(Namespace::Conversation, id).unwrap(),
                    t,
                    vec![],
                )
                .unwrap();
        })),
    };
    assert_eq!(
        writer::run(&ledger, &mut engine, &req, &preference(), &mut || false).status,
        Status::Stale
    );
    assert_eq!(api.count(), 0);
}

#[test]
fn actual_one_shot_command_generates_then_prepares_without_model_credentials() {
    use std::process::{Command, Stdio};
    let local = Local::new();
    let (ledger, id) = local.ledger();
    drop(ledger);
    let config = remarkable_reader_buddy::config::Config {
        model: Some("fixture-model".into()),
        paths: StorePaths {
            data: local.0.join("data"),
            cache: local.0.join("cache"),
            credentials: local.0.join("credentials"),
        },
        ..Default::default()
    };
    let path = local.0.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let api = Api::new(vec![reply("# Command note\n\nx = 7 (page 1).")]);
    let invoke = |request: &Request, with_model: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .arg("--writer-note")
            .env("REMARKABLE_BUDDIES_CONFIG", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if with_model {
            command
                .env("OPENAI_API_KEY", "fixture-key")
                .env("OPENAI_BASE_URL", &api.origin);
        } else {
            command
                .env_remove("OPENAI_API_KEY")
                .env_remove("OPENAI_BASE_URL");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let generated = invoke(&request(id), true);
    assert_eq!(generated["status"], "generated");
    let draft = Uuid::parse_str(generated["draft"].as_str().unwrap()).unwrap();
    let exported = invoke(
        &Request::PrepareExport {
            conversation: id,
            draft,
            correlation: Uuid::new_v4(),
            effect_operation: Uuid::new_v4(),
        },
        false,
    );
    assert_eq!(exported["status"], "prepared-export");
    assert_eq!(exported["prepared_note"]["title"], "Command note");
    assert_eq!(exported["prepared_note"]["markdown"], "x = 7 (page 1).");
    assert_eq!(api.count(), 1);
}

#[test]
fn regeneration_with_changed_text_keeps_same_note_scope_and_advances_revision() {
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let req = request(id);
    let mut changed = req.clone();
    if let Request::Generate {
        operation,
        document,
        ..
    } = &mut changed
    {
        *operation = Uuid::new_v4();
        document.source_sha256 = "b".repeat(64);
        document.units[0].text = "Updated equation: x = 8.".into();
    }
    let api = Api::new(vec![
        reply("# Note\n\nx = 7 (page 1)."),
        reply("# Note\n\nx = 8 (page 1)."),
    ]);
    let correlation = Uuid::new_v4();
    let mut notes = Vec::new();
    for input in [&req, &changed] {
        let generated = writer::run(&ledger, &mut api.model(), input, &preference(), &mut || {
            false
        });
        assert_eq!(generated.status, Status::Generated);
        let prepare = Request::PrepareExport {
            conversation: id,
            draft: generated.draft.unwrap(),
            correlation,
            effect_operation: Uuid::new_v4(),
        };
        let artifact = writer::run(
            &ledger,
            &mut api.model(),
            &prepare,
            &preference(),
            &mut || false,
        );
        assert_eq!(artifact.status, Status::PreparedExport);
        notes.push(artifact.prepared_note.unwrap());
    }
    assert_eq!(notes[0].correlation, notes[1].correlation);
    assert_eq!(notes[0].source_scope, notes[1].source_scope);
    assert_ne!(notes[0].source_revision, notes[1].source_revision);
    assert_ne!(notes[0].markdown, notes[1].markdown);
    assert_eq!(api.count(), 2);
}

#[test]
fn changed_selected_draft_before_preparation_cannot_guard_new_text_and_send_old_text() {
    use std::collections::BTreeMap;
    let local = Local::new();
    let (ledger, id) = local.ledger();
    let req = request(id);
    let api = Api::new(vec![reply("# Original\n\nx = 7 (page 1).")]);
    let original = writer::run(&ledger, &mut api.model(), &req, &preference(), &mut || {
        false
    });
    assert_eq!(original.status, Status::Generated);
    let draft = original.draft.unwrap();
    let refine = Request::Refine {
        conversation: id,
        operation: Uuid::new_v4(),
        request: "Refine exactly the selected draft.".into(),
        draft,
    };
    let mut changed = false;
    let result = writer::run(
        &ledger,
        &mut api.model(),
        &refine,
        &preference(),
        &mut || {
            // The public cancellation poll is after assembly and before preparation.
            // A valid imported successor changes only the selected draft, not root.
            if !changed {
                changed = true;
                let mut e = ledger
                    .store()
                    .heads(Namespace::Conversation, draft)
                    .unwrap()
                    .pop()
                    .unwrap();
                e.parents = [e.revision_id].into();
                e.revision_id = Uuid::new_v4();
                e.operation_id = Uuid::new_v4();
                e.payload["record"]["text"] = json!("# Imported new draft\n\nx = 8 (page 1).");
                ledger.store().commit(vec![e], BTreeMap::new()).unwrap();
            }
            false
        },
    );
    assert!(changed);
    assert_eq!(result.status, Status::Refused);
    assert_eq!(
        api.count(),
        1,
        "stale cached draft must not dispatch a second provider request"
    );
}

#[test]
fn joined_command_requires_destination_credentials_before_store_or_model() {
    use remarkable_reader_buddy::writer_flow;
    use std::process::{Command, Stdio};
    let local = Local::new();
    let id = Uuid::new_v4();
    let api = Api::new(vec![]);
    let config = remarkable_reader_buddy::config::Config {
        model: Some("fixture-model".into()),
        paths: StorePaths {
            data: local.0.join("data"),
            cache: local.0.join("cache"),
            credentials: local.0.join("credentials"),
        },
        ..Default::default()
    };
    let path = local.0.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    for request in [
        writer_flow::Request::GenerateAndExport {
            outer_operation: Uuid::new_v4(),
            generation: request(id),
            correlation: Uuid::new_v4(),
            effect_operation: Uuid::new_v4(),
        },
        writer_flow::Request::ExportDraft {
            outer_operation: Uuid::new_v4(),
            conversation: id,
            draft: Uuid::new_v4(),
            correlation: Uuid::new_v4(),
            effect_operation: Uuid::new_v4(),
        },
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .arg("--writer-note")
            .env("REMARKABLE_BUDDIES_CONFIG", &path)
            .env("OPENAI_API_KEY", "fixture-key")
            .env("OPENAI_BASE_URL", &api.origin)
            .env_remove("MEM_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["failure"], "missing-credentials");
        assert_eq!(report["status"], "refused");
        assert!(!local.0.join("data").exists());
        assert_eq!(api.count(), 0);
    }
}
