use super::*;
use remarkable_reader_buddy::{
    conversation::receiving::{
        answer as a,
        memory::{self as cm, research as rr},
    },
    llm::{openai::OpenAI, LLMEngine},
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct Research(Cell<usize>);
impl rr::Provider for Research {
    fn research(&self, r: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
        self.0.set(self.0.get() + 1);
        Ok(rr::ResearchResult {
            identity: r.identity.clone(),
            status: rr::Status::Complete,
            basis: rr::EvidenceBasis::Synthetic,
            diagnostic: None,
            queries: vec!["authored acceleration query".into()],
            sources: vec![rr::Citation {
                url: "https://example.org/acceleration?fixture=1".into(),
                title: "Authored source".into(),
                excerpt: "Explicit synthetic research assertion".into(),
                published_ms: None,
            }],
        })
    }
}
fn context(f: &Fixture, disabled: bool) -> rr::FrozenResearchContext {
    let (prior, paths, _, _) = super::saved_policy::frozen(f);
    let file = f.0.join("answer-policy.json");
    fs::write(
        &file,
        serde_json::to_vec(
            &json!({"paths":paths,"research_policy":if disabled{"disabled"}else{"required"}}),
        )
        .unwrap(),
    )
    .unwrap();
    let config = remarkable_reader_buddy::config::Config::load(Some(file)).unwrap();
    let research = Research(Cell::new(0));
    let result = prior
        .with_research(
            rr::ResearchInput {
                policy: config.research_policy_snapshot().unwrap().policy(),
                directive: rr::ResearchDirective::UsePolicy,
                provider: Some(&research),
                options: rr::ResearchOptions {
                    now_ms: 100,
                    max_queries: 2,
                    max_sources: 2,
                    max_evidence_bytes: 16384,
                },
            },
            super::saved_policy::limits(),
            counter,
        )
        .unwrap();
    assert_eq!(research.0.get(), usize::from(!disabled));
    result
}
fn output(text: &str, ids: &[&str]) -> String {
    json!({"status":"answered","text":text,"citations":ids}).to_string()
}
struct Engine {
    content: Vec<Value>,
    sent: Vec<Vec<Value>>,
    reply: String,
    calls: usize,
    clears: usize,
    fail: bool,
    panic: bool,
    ignore_progress: bool,
}
impl Engine {
    fn new(reply: String) -> Self {
        Self {
            content: vec![
                json!({"type":"text","text":"OLD PRIVATE PROMPT"}),
                json!({"type":"image_url","image_url":"OLD IMAGE"}),
            ],
            sent: vec![],
            reply,
            calls: 0,
            clears: 0,
            fail: false,
            panic: false,
            ignore_progress: false,
        }
    }
}
impl LLMEngine for Engine {
    fn add_text_content(&mut self, text: &str) {
        self.content.push(json!({"type":"text","text":text}));
    }
    fn add_image_content(&mut self, image: &str) {
        self.content
            .push(json!({"type":"image_url","image_url":image}));
    }
    fn clear_content(&mut self) {
        self.clears += 1;
        self.content.clear();
    }
    fn execute(&mut self) -> Result<String> {
        self.calls += 1;
        self.sent.push(self.content.clone());
        assert!(!self.panic, "authored engine panic");
        if self.fail {
            bail!("PRIVATE ENGINE ERROR")
        };
        Ok(self.reply.clone())
    }
    fn execute_with_progress(&mut self, p: &mut dyn FnMut() -> Result<()>) -> Result<String> {
        let r = p();
        if !self.ignore_progress {
            r?;
        }
        self.execute()
    }
}
fn run(c: &rr::FrozenResearchContext, e: &mut dyn LLMEngine) -> Result<a::AnswerDraft> {
    a::answer(c, e, a::AnswerOptions::default(), counter, &mut |_| Ok(()))
}
#[test]
fn actual_composed_history_and_captured_citations_enter_one_engine_without_prompt_bleed_or_store_writes(
) {
    let f = Fixture::new();
    let c = context(&f, false);
    let original = c.encoded_json().to_owned();
    let ledger = f.ledger();
    let request = c.source_memory_context().source_context().request();
    let before = heads(&ledger, request.conversation);
    let mut e = Engine::new(output(
        "Source [S1], assertion [M1], synthetic research [R1]; again [S1].",
        &["R1", "S1", "M1"],
    ));
    for _ in 0..2 {
        let draft = run(&c, &mut e).unwrap();
        let data: Value = serde_json::from_str(draft.encoded_json()).unwrap();
        assert_eq!(draft.status(), a::Status::Answered);
        assert_eq!(
            data["context_sha256"],
            remarkable_reader_buddy::storage::digest(original.as_bytes())
        );
        assert_eq!(
            data["citations"][0]["url"],
            "https://example.org/acceleration?fixture=1"
        );
        assert_eq!(data["citations"][0]["basis"], "synthetic");
        assert_eq!(
            data["citations"][2]["authority"],
            "author-asserted-not-source-verified"
        );
        let retrieved = c
            .source_memory_context()
            .source_context()
            .retrieval()
            .unwrap();
        assert_eq!(
            data["citations"][1]["range"],
            json!(retrieved.passages[0].range)
        );
        assert_eq!(
            data["citations"][1]["source"],
            serde_json::to_value(&retrieved.source).unwrap()
        );
        assert!(e.content.is_empty());
        assert_eq!(e.sent.last().unwrap().len(), 2);
        assert_eq!(e.sent.last().unwrap()[1]["text"], original);
        assert_eq!(e.sent.last().unwrap()[0]["type"], "text");
        assert_eq!(
            draft.prompt_sha256(),
            digest(&serde_json::to_vec(e.sent.last().unwrap()).unwrap())
        );
    }
    assert_eq!(e.calls, 2);
    assert_eq!(e.clears, 4);
    assert_eq!(e.sent[0], e.sent[1]);
    assert_eq!(heads(&ledger, request.conversation), before);
    assert_eq!(c.encoded_json(), original);
}
#[test]
fn reserved_marker_refusals_do_not_reject_ordinary_prose_or_math_brackets() {
    let f = Fixture::new();
    let c = context(&f, false);
    for text in [
        "[Source] [Memory] [Research] [R, S] [S x] [0, 1] [S]",
        "[S1] [M1] [R1] and interval [0, 1]",
    ] {
        let ids = if text.contains("[S1]") {
            vec!["S1", "M1", "R1"]
        } else {
            vec![]
        };
        assert!(run(&c, &mut Engine::new(output(text, &ids))).is_ok());
    }
    for (text, ids) in [
        ("bad [S0]", vec!["S0"]),
        ("bad [S01]", vec!["S01"]),
        ("bad [R-1]", vec!["R-1"]),
        ("bad [M+1]", vec!["M+1"]),
        ("unknown [R999]", vec!["R999"]),
        ("unterminated [S1", vec![]),
        ("absent", vec!["R1"]),
        ("unlisted [M1]", vec![]),
        ("duplicate [S1]", vec!["S1", "S1"]),
        ("wrong [S1]", vec!["R1"]),
        ("bad [S1 x]", vec!["S1 x"]),
        (
            "overflow [S999999999999999999999]",
            vec!["S999999999999999999999"],
        ),
    ] {
        let mut e = Engine::new(output(text, &ids));
        assert!(run(&c, &mut e).is_err(), "{text}");
        assert!(e.content.is_empty());
        assert_eq!(e.calls, 1);
    }
}
#[test]
fn strict_generated_shape_and_content_refusals_preserve_engine_reuse() {
    let f = Fixture::new();
    let c = context(&f, false);
    let cases = vec![
        "NONE".into(),
        "{}".into(),
        "```json\n{}\n```".into(),
        r#"{"status":"answered","text":"x","text":"y","citations":[]}"#.into(),
        json!({"status":"answered","text":"x","citations":[],"url":"https://invented.example"})
            .to_string(),
        output("", &[]),
        output("NONE", &[]),
        output("a\0b", &[]),
        json!({"status":"clarification","text":"Which? [S1]","citations":["S1"]}).to_string(),
        json!({"status":"wrong","text":"x","citations":[]}).to_string(),
    ];
    let mut e = Engine::new(String::new());
    for reply in cases {
        e.reply = reply;
        let err = run(&c, &mut e).err().unwrap().to_string();
        assert!(!err.contains("invented"));
        assert!(e.content.is_empty());
    }
    e.reply =
        json!({"status":"clarification","text":"Which missing detail do you mean?","citations":[]})
            .to_string();
    assert_eq!(run(&c, &mut e).unwrap().status(), a::Status::Clarification);
    assert!(e.content.is_empty());
}
#[test]
fn whole_prompt_counter_exact_byte_token_raw_text_and_result_boundaries() {
    let f = Fixture::new();
    let c = context(&f, false);
    let response = output("bounded [S1]", &["S1"]);
    let mut e = Engine::new(response.clone());
    let observed = RefCell::new(String::new());
    let calls = Cell::new(0);
    let result = a::answer(
        &c,
        &mut e,
        a::AnswerOptions::default(),
        |s| {
            calls.set(calls.get() + 1);
            *observed.borrow_mut() = s.into();
            Ok(7)
        },
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    let prompt = observed.into_inner();
    assert_eq!(
        serde_json::from_str::<Value>(&prompt).unwrap(),
        json!(e.sent[0])
    );
    let options = a::AnswerOptions {
        max_input_bytes: prompt.len(),
        max_tokens: 7,
        max_response_bytes: response.len(),
        max_text_bytes: 12,
        max_citations: 1,
        max_result_bytes: result.encoded_json().len(),
    };
    assert!(a::answer(&c, &mut e, options, |_| Ok(7), &mut |_| Ok(())).is_ok());
    for (changed, preflight) in [
        (
            a::AnswerOptions {
                max_input_bytes: prompt.len() - 1,
                ..options
            },
            true,
        ),
        (
            a::AnswerOptions {
                max_tokens: 6,
                ..options
            },
            true,
        ),
        (
            a::AnswerOptions {
                max_response_bytes: response.len() - 1,
                ..options
            },
            false,
        ),
        (
            a::AnswerOptions {
                max_text_bytes: 11,
                ..options
            },
            false,
        ),
        (
            a::AnswerOptions {
                max_citations: 0,
                ..options
            },
            false,
        ),
        (
            a::AnswerOptions {
                max_result_bytes: result.encoded_json().len() - 1,
                ..options
            },
            false,
        ),
    ] {
        let before = e.calls;
        assert!(a::answer(&c, &mut e, changed, |_| Ok(7), &mut |_| Ok(())).is_err());
        assert_eq!(e.calls - before, usize::from(!preflight));
        assert!(e.content.is_empty());
    }
    for count in [0, 8] {
        let before = e.calls;
        assert!(a::answer(&c, &mut e, options, |_| Ok(count), &mut |_| Ok(())).is_err());
        assert_eq!(e.calls, before);
    }
    assert!(a::answer(
        &c,
        &mut e,
        options,
        |_| bail!("PRIVATE COUNTER"),
        &mut |_| Ok(())
    )
    .err()
    .unwrap()
    .to_string()
    .contains("counter unavailable"));
    assert!(a::answer(
        &c,
        &mut e,
        a::AnswerOptions {
            max_tokens: 1,
            ..a::AnswerOptions::default()
        },
        |_| Ok(1),
        &mut |_| Ok(())
    )
    .is_ok());
}
#[test]
fn all_progress_failures_and_counter_engine_callback_panics_clear_even_ignored_errors() {
    let f = Fixture::new();
    let c = context(&f, false);
    for phase in [
        a::Progress::BeforeDispatch,
        a::Progress::EngineProgress,
        a::Progress::AfterResponse,
        a::Progress::BeforeReturn,
    ] {
        let mut e = Engine::new(output("answer", &[]));
        e.ignore_progress = true;
        assert!(
            a::answer(&c, &mut e, a::AnswerOptions::default(), counter, &mut |p| {
                if p == phase {
                    bail!("PRIVATE PROGRESS")
                }
                Ok(())
            })
            .is_err()
        );
        assert!(e.content.is_empty());
        let mut e = Engine::new(output("answer", &[]));
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _ = a::answer(&c, &mut e, a::AnswerOptions::default(), counter, &mut |p| {
                assert_ne!(p, phase, "authored callback panic");
                Ok(())
            });
        }))
        .is_err());
        assert!(e.content.is_empty());
    }
    let mut e = Engine::new(output("answer", &[]));
    assert!(catch_unwind(AssertUnwindSafe(|| {
        let _ = a::answer(
            &c,
            &mut e,
            a::AnswerOptions::default(),
            |_| panic!("authored counter panic"),
            &mut |_| Ok(()),
        );
    }))
    .is_err());
    assert!(e.content.is_empty());
    e.panic = true;
    assert!(catch_unwind(AssertUnwindSafe(|| {
        let _ = run(&c, &mut e);
    }))
    .is_err());
    assert!(e.content.is_empty());
    e.panic = false;
    e.fail = true;
    assert_eq!(
        run(&c, &mut e).err().unwrap().to_string(),
        "answering engine failed"
    );
    assert!(e.content.is_empty());
}
#[test]
fn absent_source_memory_and_disabled_no_internet_cannot_invent_references() {
    let f = Fixture::new();
    let ledger = f.ledger();
    let id = create(&ledger);
    let turn = append(&ledger, user(id, "  blank start exact?\n"), vec![]);
    let prior = ledger
        .source_request_context(
            id,
            turn.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap()
        .with_memory(
            cm::MemoryInput::Disabled,
            super::saved_policy::limits(),
            counter,
        )
        .unwrap();
    let no_calls = Research(Cell::new(0));
    let c = prior
        .with_research(
            rr::ResearchInput {
                policy: rr::ResearchPolicy::Required,
                directive: rr::ResearchDirective::NoInternet,
                provider: Some(&no_calls),
                options: rr::ResearchOptions {
                    now_ms: 100,
                    max_queries: 1,
                    max_sources: 1,
                    max_evidence_bytes: 16384,
                },
            },
            super::saved_policy::limits(),
            counter,
        )
        .unwrap();
    assert_eq!(no_calls.0.get(), 0);
    assert_eq!(
        c.research().disabled_reason(),
        Some(rr::DisabledReason::RequestNoInternet)
    );
    for id in ["S1", "M1", "R1"] {
        assert!(run(
            &c,
            &mut Engine::new(output(&format!("invented [{id}]"), &[id]))
        )
        .is_err());
    }
    assert!(run(&c, &mut Engine::new(output("What scope do you mean?", &[]))).is_ok());
    drop(ledger);
    let f = Fixture::new();
    let c = context(&f, true);
    assert!(run(&c, &mut Engine::new(output("invented [R1]", &["R1"]))).is_err());
    assert!(run(
        &c,
        &mut Engine::new(output("local [S1] and memory [M1]", &["S1", "M1"]))
    )
    .is_ok());
}

enum Reply {
    Text(String),
    Hold(bool),
    Error,
}
struct Server {
    base: String,
    received: mpsc::Receiver<Value>,
    partial: mpsc::Receiver<()>,
    finished: Arc<AtomicBool>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<usize>>,
}
impl Server {
    fn new(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, received) = mpsc::channel();
        let (ptx, partial) = mpsc::channel();
        let (stop, rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let held = finished.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut count = 0;
            loop {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        s.set_nonblocking(false).unwrap();
                        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                        count += 1;
                        assert!(count <= 4);
                        let mut header = vec![];
                        while !header.ends_with(b"\r\n\r\n") {
                            assert!(header.len() < 16384);
                            let mut b = [0];
                            s.read_exact(&mut b).unwrap();
                            header.push(b[0]);
                        }
                        let h = std::str::from_utf8(&header).unwrap();
                        assert!(h.starts_with("POST /v1/chat/completions "));
                        let size = h
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        assert!(size <= 1024 * 1024);
                        let mut bytes = vec![0; size];
                        s.read_exact(&mut bytes).unwrap();
                        tx.send(serde_json::from_slice(&bytes).unwrap()).unwrap();
                        match &reply {
                            Reply::Error => {
                                s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 20\r\nConnection: close\r\n\r\nPRIVATE_PROVIDER_ERR").unwrap();
                            }
                            Reply::Text(text) => {
                                let body =
                                    json!({"choices":[{"message":{"content":text}}]}).to_string();
                                s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).unwrap();
                            }
                            Reply::Hold(body) => {
                                if *body {
                                    s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\n{\r\n").unwrap();
                                    ptx.send(()).unwrap();
                                }
                                let _ = rx.recv_timeout(Duration::from_secs(3));
                                held.store(true, Ordering::Release);
                                break;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if rx.try_recv().is_ok() {
                            break;
                        }
                        assert!(Instant::now() < deadline);
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            }
            count
        });
        Self {
            base,
            received,
            partial,
            finished,
            stop,
            worker: Some(worker),
        }
    }
    fn finish(&mut self) -> usize {
        let _ = self.stop.send(());
        self.worker.take().unwrap().join().unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
fn real_engine(s: &Server) -> OpenAI {
    OpenAI::new(
        "answer-fixture".into(),
        "dummy_answer_key".into(),
        Some(s.base.clone()),
    )
    .with_timeout(Duration::from_secs(2))
}
#[test]
fn actual_existing_engine_http_uses_two_exact_text_parts_and_clears_prior_images_on_reuse() {
    let f = Fixture::new();
    let c = context(&f, false);
    let mut s = Server::new(Reply::Text(output(
        "real HTTP [S1] and [R1]",
        &["S1", "R1"],
    )));
    let mut e = real_engine(&s);
    e.add_image_content("OLD_PRIVATE_IMAGE");
    e.add_text_content("OLD_PRIVATE_TEXT");
    let mut prior = None;
    for _ in 0..2 {
        assert!(run(&c, &mut e).is_ok());
        let body = s.received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(body["model"], "answer-fixture");
        assert_eq!(body["max_completion_tokens"], 4000);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["text"], c.encoded_json());
        assert!(parts.iter().all(|p| p["type"] == "text"));
        if let Some(p) = prior {
            assert_eq!(body, p);
        }
        prior = Some(body);
    }
    assert_eq!(s.finish(), 2);
}
#[test]
fn actual_existing_engine_cancellation_and_panic_after_partial_write_return_before_release_and_join(
) {
    for (body, panic) in [(false, false), (true, false), (true, true)] {
        let f = Fixture::new();
        let c = context(&f, false);
        let mut s = Server::new(Reply::Hold(body));
        let mut e = real_engine(&s);
        let mut seen = false;
        let mut partial = false;
        let mut canceled = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            a::answer(&c, &mut e, a::AnswerOptions::default(), counter, &mut |p| {
                if p == a::Progress::EngineProgress {
                    if let Ok(request) = s.received.try_recv() {
                        assert_eq!(
                            request["messages"][0]["content"][1]["text"],
                            c.encoded_json()
                        );
                        seen = true;
                    }
                    partial |= s.partial.try_recv().is_ok();
                    if seen && (!body || partial) {
                        canceled = Some(Instant::now());
                        assert!(!panic, "authored actual-engine callback panic");
                        bail!("authored cancel");
                    }
                }
                Ok(())
            })
        }));
        assert_eq!(result.is_err(), panic);
        if !panic {
            assert!(result.unwrap().is_err());
        }
        assert!(canceled.unwrap().elapsed() < Duration::from_millis(500));
        assert!(!s.finished.load(Ordering::Acquire));
        assert!(seen && (!body || partial));
        assert_eq!(s.finish(), 1);
    }
}

#[test]
fn invalid_options_refuse_before_any_engine_invocation_and_clear_old_content() {
    let f = Fixture::new();
    let c = context(&f, false);
    let options = a::AnswerOptions::default();
    for bad in [
        a::AnswerOptions {
            max_input_bytes: 0,
            ..options
        },
        a::AnswerOptions {
            max_input_bytes: 1048577,
            ..options
        },
        a::AnswerOptions {
            max_tokens: 0,
            ..options
        },
        a::AnswerOptions {
            max_response_bytes: 0,
            ..options
        },
        a::AnswerOptions {
            max_response_bytes: 65537,
            ..options
        },
        a::AnswerOptions {
            max_text_bytes: 0,
            ..options
        },
        a::AnswerOptions {
            max_text_bytes: 16385,
            ..options
        },
        a::AnswerOptions {
            max_citations: 33,
            ..options
        },
        a::AnswerOptions {
            max_result_bytes: 0,
            ..options
        },
        a::AnswerOptions {
            max_result_bytes: 131073,
            ..options
        },
    ] {
        let mut e = Engine::new(output("answer", &[]));
        let counted = Cell::new(false);
        assert!(a::answer(
            &c,
            &mut e,
            bad,
            |_| {
                counted.set(true);
                Ok(1)
            },
            &mut |_| Ok(())
        )
        .is_err());
        assert!(!counted.get());
        assert_eq!(e.calls, 0);
        assert!(e.content.is_empty());
        assert_eq!(e.clears, 2);
    }
}
#[test]
fn complete_empty_partial_and_changed_sources_keep_distinct_captured_facts() {
    struct Variant(rr::Status);
    impl rr::Provider for Variant {
        fn research(&self, r: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
            let mut v = rr::Provider::research(&Research(Cell::new(0)), r)?;
            v.status = self.0;
            if self.0 == rr::Status::CompleteEmpty {
                v.sources.clear();
            }
            if self.0 == rr::Status::Partial {
                v.diagnostic = Some(rr::Diagnostic::BudgetLimited);
            }
            Ok(v)
        }
    }
    let mut e = Engine::new(String::new());
    let mut previous = None;
    for status in [rr::Status::CompleteEmpty, rr::Status::Partial] {
        let f = Fixture::new();
        let (prior, _, _, _) = super::saved_policy::frozen(&f);
        let c = prior
            .with_research(
                rr::ResearchInput {
                    policy: rr::ResearchPolicy::Required,
                    directive: rr::ResearchDirective::UsePolicy,
                    provider: Some(&Variant(status)),
                    options: rr::ResearchOptions {
                        now_ms: 100,
                        max_queries: 2,
                        max_sources: 2,
                        max_evidence_bytes: 16384,
                    },
                },
                super::saved_policy::limits(),
                counter,
            )
            .unwrap();
        e.reply = if status == rr::Status::Partial {
            output("partial [S1] and [R1]", &["S1", "R1"])
        } else {
            output("no selected research; source [S1]", &["S1"])
        };
        let draft = run(&c, &mut e).unwrap();
        assert_eq!(e.sent.last().unwrap()[1]["text"], c.encoded_json());
        let sent: Value = serde_json::from_str(c.encoded_json()).unwrap();
        assert_eq!(
            sent["research"]["status"],
            serde_json::to_value(status).unwrap()
        );
        let report: Value = serde_json::from_str(draft.encoded_json()).unwrap();
        let source = report["citations"][0]["source"]["document"].clone();
        if let Some(old) = previous {
            assert_ne!(source, old);
        }
        previous = Some(source);
        if status == rr::Status::CompleteEmpty {
            e.reply = output("invented [R1]", &["R1"]);
            assert!(run(&c, &mut e).is_err());
        }
    }
}
#[test]
fn actual_existing_engine_errors_raw_output_caps_and_finite_timeout_do_not_yield_drafts() {
    let f = Fixture::new();
    let c = context(&f, false);
    for (reply, expected) in [
        (Reply::Error, "answering engine failed"),
        (Reply::Text("x".repeat(65537)), "invalid generated answer"),
    ] {
        let mut s = Server::new(reply);
        let mut e = real_engine(&s);
        assert_eq!(run(&c, &mut e).err().unwrap().to_string(), expected);
        assert_eq!(s.finish(), 1);
    }
    let mut s = Server::new(Reply::Hold(false));
    let mut e = real_engine(&s).with_timeout(Duration::from_millis(150));
    let start = Instant::now();
    assert!(run(&c, &mut e).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!s.finished.load(Ordering::Acquire));
    assert_eq!(s.finish(), 1);
}

#[test]
fn research_citation_authority_preserves_both_captured_origins() {
    struct Origin(rr::EvidenceBasis);
    impl rr::Provider for Origin {
        fn research(&self, request: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
            let mut result = rr::Provider::research(&Research(Cell::new(0)), request)?;
            result.basis = self.0;
            Ok(result)
        }
    }
    for (basis, authority) in [
        (
            rr::EvidenceBasis::Synthetic,
            "synthetic-unverified-research",
        ),
        (
            rr::EvidenceBasis::ProviderAsserted,
            "provider-asserted-unverified",
        ),
    ] {
        let f = Fixture::new();
        let (prior, _, _, _) = super::saved_policy::frozen(&f);
        let context = prior
            .with_research(
                rr::ResearchInput {
                    policy: rr::ResearchPolicy::Required,
                    directive: rr::ResearchDirective::UsePolicy,
                    provider: Some(&Origin(basis)),
                    options: rr::ResearchOptions {
                        now_ms: 100,
                        max_queries: 2,
                        max_sources: 2,
                        max_evidence_bytes: 16384,
                    },
                },
                super::saved_policy::limits(),
                counter,
            )
            .unwrap();
        let answer = run(&context, &mut Engine::new(output("captured [R1]", &["R1"]))).unwrap();
        let report: Value = serde_json::from_str(answer.encoded_json()).unwrap();
        assert_eq!(
            report["citations"][0]["basis"],
            serde_json::to_value(basis).unwrap()
        );
        assert_eq!(
            report["citations"][0]["authority"], authority,
            "captured basis={basis:?}"
        );
    }
}
