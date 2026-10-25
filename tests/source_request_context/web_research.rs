//! Actual local HTTP and real Store/source/memory; all remote-style bodies here are authored fixtures.
use super::*;
use remarkable_reader_buddy::{
    config::Config,
    conversation::receiving::memory::{self as cm, research as rr},
};
use rr::openai::{OpenAIWebResearch, Progress, TransportOptions};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
#[derive(Clone)]
enum Reply {
    Body(Value),
    Raw(Vec<u8>),
    HoldHeaders,
    HoldBody,
    Sequence(Vec<Reply>),
}
struct Capture {
    body: Value,
    at: Instant,
}
struct Server {
    endpoint: String,
    received: mpsc::Receiver<Capture>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<usize>>,
}
impl Server {
    fn new(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let (tx, received) = mpsc::channel();
        let (stop, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut count = 0;
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        count += 1;
                        assert!(count <= 16);
                        let body = request(&mut stream);
                        tx.send(Capture {
                            body,
                            at: Instant::now(),
                        })
                        .unwrap();
                        let reply = match &reply {
                            Reply::Sequence(items) => &items[count - 1],
                            other => other,
                        };
                        match reply {
                            Reply::Sequence(_) => panic!("nested fixture sequence unsupported"),
                            Reply::Body(v) => {
                                let _ = stream.write_all(&http(&serde_json::to_vec(v).unwrap()));
                            }
                            Reply::Raw(v) => {
                                let _ = stream.write_all(v);
                            }
                            Reply::HoldHeaders => {
                                let _ = rx.recv_timeout(Duration::from_secs(3));
                                break;
                            }
                            Reply::HoldBody => {
                                let _=stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\n{\r\n");
                                let _ = rx.recv_timeout(Duration::from_secs(3));
                                break;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if rx.try_recv().is_ok() {
                            break;
                        }
                        assert!(Instant::now() < deadline);
                        // Bounded socket observation; time alone proves neither request receipt nor cancellation.
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            }
            count
        });
        Self {
            endpoint,
            received,
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
fn request(s: &mut TcpStream) -> Value {
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut h = Vec::new();
    while !h.ends_with(b"\r\n\r\n") {
        assert!(h.len() < 16384);
        let mut b = [0];
        s.read_exact(&mut b).unwrap();
        h.push(b[0]);
    }
    let h = std::str::from_utf8(&h).unwrap();
    assert!(h.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    assert!(h
        .to_ascii_lowercase()
        .contains("authorization: bearer private_fixture_key\r\n"));
    let n = h
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(n <= 1024 * 1024);
    let mut b = vec![0; n];
    s.read_exact(&mut b).unwrap();
    serde_json::from_slice(&b).unwrap()
}
fn http(b: &[u8]) -> Vec<u8> {
    let mut v=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",b.len()).into_bytes();
    v.extend_from_slice(b);
    v
}
fn answer(empty: bool) -> Value {
    let sources = if empty {
        vec![]
    } else {
        vec![
            json!({"url":"https://example.invalid/research","title":"Authored fixture","excerpt":"Unverified provider summary, not a fetched quote."}),
        ]
    };
    json!({"status":"completed","error":null,"incomplete_details":null,"output":[{"type":"web_search_call","status":"completed","action":{"type":"search","queries":["authored acceleration query"],"sources":[{"type":"url","url":"https://example.invalid/research"}]}},{"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":json!({"sources":sources}).to_string()}]}]})
}
fn options() -> rr::ResearchOptions {
    rr::ResearchOptions {
        now_ms: 1000,
        max_queries: 4,
        max_sources: 8,
        max_evidence_bytes: 16384,
    }
}
fn input(p: &dyn rr::Provider) -> rr::ResearchInput<'_> {
    rr::ResearchInput {
        policy: rr::ResearchPolicy::Required,
        directive: rr::ResearchDirective::UsePolicy,
        provider: Some(p),
        options: options(),
    }
}
fn backend<'a>(
    endpoint: &str,
    o: TransportOptions,
    p: &'a mut dyn FnMut(Progress) -> Result<()>,
) -> OpenAIWebResearch<'a> {
    OpenAIWebResearch::new(
        "authored-model".into(),
        endpoint.into(),
        "PRIVATE_FIXTURE_KEY".into(),
        o,
        p,
    )
    .unwrap()
}
fn raw(prior: &cm::FrozenSourceMemoryContext) -> rr::ResearchRequest<'_> {
    let t = prior.source_context().request();
    rr::ResearchRequest {
        identity: rr::ResearchIdentity {
            conversation: t.conversation,
            request_turn: t.id,
            context_sha256: digest(prior.encoded_json().as_bytes()),
            observed_ms: 1000,
        },
        context: prior,
        options: options(),
    }
}
#[test]
fn actual_http_saved_policy_and_full_source_memory_history_roundtrip() {
    for empty in [false, true] {
        let f = Fixture::new();
        let (prior, paths, turn, history) = saved_policy::frozen(&f);
        let file = f.0.join("policy.json");
        fs::write(
            &file,
            serde_json::to_vec(&json!({"paths":paths,"research_policy":"disabled"})).unwrap(),
        )
        .unwrap();
        saved_policy::save(&file, Some("required"));
        let policy = Config::load(Some(file))
            .unwrap()
            .research_policy_snapshot()
            .unwrap();
        let exact = prior.encoded_json().to_owned();
        let before = snapshot(&f.0);
        let mut server = Server::new(Reply::Body(answer(empty)));
        let phases = RefCell::new(Vec::new());
        let mut progress = |p| {
            phases.borrow_mut().push(p);
            Ok(())
        };
        let provider = backend(&server.endpoint, Default::default(), &mut progress);
        let mut i = input(&provider);
        i.policy = policy.policy();
        let result = prior
            .with_research(i, saved_policy::limits(), counter)
            .unwrap();
        assert!(
            provider.take_exchange_observation().is_none(),
            "default-off observation"
        );
        let captured = server
            .received
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(server.finish(), 1);
        assert_eq!(captured.body["input"][0]["content"][0]["text"], exact);
        for (k, v) in [
            ("store", json!(false)),
            ("background", json!(false)),
            ("stream", json!(false)),
            ("truncation", json!("disabled")),
            ("tool_choice", json!("required")),
            ("max_tool_calls", json!(4)),
            ("max_output_tokens", json!(4096)),
            ("parallel_tool_calls", json!(false)),
        ] {
            assert_eq!(captured.body[k], v, "{k}");
        }
        assert_eq!(
            captured.body["tools"],
            json!([{"type":"web_search","external_web_access":true}])
        );
        assert_eq!(captured.body["model"], "authored-model");
        assert_eq!(
            captured.body["include"],
            json!(["web_search_call.action.sources"])
        );
        assert_eq!(captured.body["text"]["format"]["strict"], true);
        assert_eq!(result.source_memory_context().encoded_json(), exact);
        assert_eq!(
            result.source_memory_context().source_context().request(),
            &turn
        );
        assert_eq!(
            result.source_memory_context().source_context().history(),
            [history]
        );
        assert_eq!(
            result.research().status(),
            if empty {
                rr::Status::CompleteEmpty
            } else {
                rr::Status::Complete
            }
        );
        assert_eq!(
            result.research().basis(),
            Some(rr::EvidenceBasis::ProviderAsserted)
        );
        assert_eq!(result.research().queries(), ["authored acceleration query"]);
        if !empty {
            assert_eq!(result.research().sources()[0].published_ms, None);
            assert!(result.research().sources()[0]
                .excerpt
                .contains("not a fetched quote"));
        }
        let phases = phases.borrow();
        assert_eq!(phases.first(), Some(&Progress::BeforeDispatch));
        assert_eq!(phases.last(), Some(&Progress::BeforeReturn));
        assert!(phases.contains(&Progress::ResponseReceived));
        assert_eq!(snapshot(&f.0), before);
    }
}
#[test]
fn optouts_and_invalid_context_or_request_cap_have_zero_network_dispatch() {
    for (policy, directive) in [
        ("disabled", rr::ResearchDirective::UsePolicy),
        ("required", rr::ResearchDirective::NoInternet),
        ("disabled", rr::ResearchDirective::NoInternet),
    ] {
        let f = Fixture::new();
        let (prior, paths, _, _) = saved_policy::frozen(&f);
        let file = f.0.join("policy.json");
        fs::write(&file,serde_json::to_vec(&json!({"paths":paths,"research_policy":if policy=="disabled"{"required"}else{"disabled"}})).unwrap()).unwrap();
        saved_policy::save(&file, Some(policy));
        let policy = Config::load(Some(file))
            .unwrap()
            .research_policy_snapshot()
            .unwrap();
        let mut server = Server::new(Reply::Body(answer(false)));
        let calls = Cell::new(0);
        let mut progress = |_| {
            calls.set(calls.get() + 1);
            Ok(())
        };
        let provider = backend(&server.endpoint, Default::default(), &mut progress);
        let mut i = input(&provider);
        i.policy = policy.policy();
        i.directive = directive;
        assert_eq!(
            prior
                .with_research(i, saved_policy::limits(), counter)
                .unwrap()
                .research()
                .status(),
            rr::Status::Disabled
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(server.finish(), 0);
    }
    for cap in [false, true] {
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(Reply::Body(answer(false)));
        let calls = Cell::new(0);
        let mut progress = |_| {
            calls.set(calls.get() + 1);
            Ok(())
        };
        let provider = backend(
            &server.endpoint,
            TransportOptions {
                max_request_bytes: if cap { 1 } else { 1024 * 1024 },
                ..Default::default()
            },
            &mut progress,
        );
        let mut r = raw(&prior);
        if !cap {
            r.identity.context_sha256 = digest(b"foreign");
        }
        assert!(rr::Provider::research(&provider, &r).is_err());
        assert_eq!(calls.get(), 0);
        assert_eq!(server.finish(), 0);
    }
}
#[test]
fn actual_socket_refuses_invalid_hosted_search_or_structured_evidence() {
    for mode in 0..23 {
        let mut body = answer(false);
        match mode{
 0=>body["status"]=json!("incomplete"),1=>body["error"]=json!({"message":"PRIVATE_PROVIDER"}),2=>body["output"][0]["status"]=json!("failed"),
 3=>{body["output"][0]["action"].as_object_mut().unwrap().remove("queries");},4=>body["output"][0]["action"]["queries"]=json!([]),5=>body["output"][0]["action"]["queries"]=json!(["a","b","c","d","e"]),6=>{body["output"][0]["action"].as_object_mut().unwrap().remove("sources");},7=>body["output"][0]["action"]["sources"][0]["url"]=json!("https://example.invalid/foreign"),8=>body["output"][1]["content"][0]["type"]=json!("refusal"),9=>body["output"][1]["content"][0]["text"]=json!("PRIVATE_INVALID_JSON"),10=>body["output"][1]["content"][0]["text"]=json!("{\"sources\":[],\"sources\":[]}"),11=>body["output"][1]["content"][0]["text"]=json!("{\"sources\":[],\"PRIVATE_EXTRA\":true}"),
 12=>{let mut selected:Value=serde_json::from_str(body["output"][1]["content"][0]["text"].as_str().unwrap()).unwrap();let source=selected["sources"][0].clone();selected["sources"].as_array_mut().unwrap().push(source);body["output"][1]["content"][0]["text"]=json!(selected.to_string());},
 13=>{body["output"][0]["action"]["sources"][0]["url"]=json!("https://example.invalid/research#fragment");body["output"][1]["content"][0]["text"]=json!(json!({"sources":[{"url":"https://example.invalid/research#fragment","title":"x","excerpt":"x"}]}).to_string());},
 14=>body["output"][1]["content"][0]["text"]=json!(json!({"sources":[{"url":"https://example.invalid/research","title":"x","excerpt":"\u{754c}".repeat(1400)}]}).to_string()),
 15=>{let mut items=body["output"].as_array().unwrap().clone();items.extend(vec![json!({"type":"reasoning"});129]);body["output"]=json!(items);},16=>body["output"][0]["action"]["queries"]=json!(["PRIVATE_BAD\u{0}QUERY"]),
 17=>{let call=body["output"][0].clone();let message=body["output"][1].clone();let mut items=vec![call;5];items.push(message);body["output"]=json!(items);},
 18=>body["output"][0]["action"]["queries"]=json!(vec!["same query";65]),
 19=>body["output"][0]["action"]["sources"]=json!(vec![json!({"type":"url","url":"https://example.invalid/research"});1025]),
 20=>body["output"][1]["content"][0]["text"]=json!("x".repeat(128*1024+1)),
 21=>{let mut v:Value=serde_json::from_str(body["output"][1]["content"][0]["text"].as_str().unwrap()).unwrap();v["sources"][0]["extra"]=json!(true);body["output"][1]["content"][0]["text"]=json!(v.to_string());},
 _=>body["output"][1]["content"][0]["text"]=json!("{\"sources\":{}}")}
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(Reply::Body(body));
        let mut progress = |_| Ok(());
        let provider = backend(&server.endpoint, Default::default(), &mut progress);
        let error = rr::Provider::research(&provider, &raw(&prior)).unwrap_err();
        assert!(!format!("{error:#}").contains("PRIVATE_"), "case{mode}");
        assert_eq!(server.finish(), 1, "case{mode}");
    }
}
#[test]
fn real_chunked_and_declared_caps_http_errors_and_redirects_never_retry() {
    let declared =
        b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n".to_vec();
    let status =
        b"HTTP/1.1 500 Failed\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE".to_vec();
    let data = vec![b'x'; 2048];
    let mut chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        data.len()
    )
    .into_bytes();
    chunked.extend(data);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    for bytes in [declared, status, chunked] {
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(Reply::Raw(bytes));
        let mut progress = |_| Ok(());
        let provider = backend(
            &server.endpoint,
            TransportOptions {
                max_response_bytes: 1024,
                ..Default::default()
            },
            &mut progress,
        );
        let error = rr::Provider::research(&provider, &raw(&prior)).unwrap_err();
        assert!(!format!("{error:#}").contains("PRIVATE"));
        assert_eq!(server.finish(), 1);
    }
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut target = Server::new(Reply::Body(answer(false)));
    let bytes = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        target.endpoint
    )
    .into_bytes();
    let mut source = Server::new(Reply::Raw(bytes));
    let mut progress = |_| Ok(());
    let provider = backend(&source.endpoint, Default::default(), &mut progress);
    assert!(rr::Provider::research(&provider, &raw(&prior)).is_err());
    assert_eq!(source.finish(), 1);
    assert_eq!(target.finish(), 0);
}
#[test]
fn witnessed_request_cancel_during_withheld_headers_or_body_joins_owned_threads() {
    for reply in [Reply::HoldHeaders, Reply::HoldBody] {
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(reply);
        let witnessed = Cell::new(None);
        let mut progress = |phase| {
            if phase == Progress::Waiting {
                if let Ok(c) = server.received.try_recv() {
                    witnessed.set(Some(c.at));
                    bail!("PRIVATE_CALLBACK");
                }
            }
            Ok(())
        };
        let provider = backend(&server.endpoint, Default::default(), &mut progress);
        let result = rr::Provider::research(&provider, &raw(&prior));
        let returned = Instant::now();
        drop(provider);
        let received = witnessed.get().expect("actual invocation request witness");
        let count = server.finish();
        assert!(result.is_err());
        assert_eq!(count, 1);
        assert!(
            returned.duration_since(received) < Duration::from_millis(500),
            "local return depended on withheld response"
        );
    }
}
#[test]
fn phase_witnesses_cancel_before_dispatch_after_join_and_before_return_and_panic_cleans_up() {
    for phase in [
        Progress::BeforeDispatch,
        Progress::ResponseReceived,
        Progress::BeforeReturn,
    ] {
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(Reply::Body(answer(false)));
        let hit = Cell::new(false);
        let mut progress = |observed| {
            if observed == phase {
                hit.set(true);
                bail!("PRIVATE_CALLBACK");
            }
            Ok(())
        };
        let provider = backend(&server.endpoint, Default::default(), &mut progress);
        let error = rr::Provider::research(&provider, &raw(&prior)).unwrap_err();
        assert!(hit.get());
        assert!(!format!("{error:#}").contains("PRIVATE"));
        assert_eq!(
            server.finish(),
            usize::from(phase != Progress::BeforeDispatch)
        );
    }
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut server = Server::new(Reply::HoldHeaders);
    let mut progress = |p| {
        if p == Progress::Waiting && server.received.try_recv().is_ok() {
            panic!("authored callback panic");
        }
        Ok(())
    };
    let provider = backend(&server.endpoint, Default::default(), &mut progress);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rr::Provider::research(&provider, &raw(&prior))
    }));
    drop(provider);
    assert_eq!(server.finish(), 1);
    assert!(result.is_err());
}
#[test]
fn explicit_endpoint_validation_and_finite_timeout_do_not_retarget() {
    let mut progress = |_| Ok(());
    for url in [
        "http://example.invalid/v1/responses",
        "http://localhost/v1/responses",
        "https://user:PRIVATE@example.invalid/v1/responses",
        "https://example.invalid/v1/responses?private=yes",
        "https://example.invalid/v1/chat/completions",
    ] {
        let result = OpenAIWebResearch::new(
            "fixture".into(),
            url.into(),
            "PRIVATE".into(),
            Default::default(),
            &mut progress,
        );
        assert!(result.is_err());
        assert!(!result.err().unwrap().to_string().contains("PRIVATE"));
    }
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut server = Server::new(Reply::HoldHeaders);
    let start = Instant::now();
    let provider = backend(
        &server.endpoint,
        TransportOptions {
            timeout: Duration::from_secs(1),
            ..Default::default()
        },
        &mut progress,
    );
    let error = rr::Provider::research(&provider, &raw(&prior)).unwrap_err();
    let elapsed = start.elapsed();
    assert_eq!(server.finish(), 1);
    assert!(error.to_string().contains("timeout"));
    assert!(elapsed < Duration::from_secs(2));
}

#[test]
fn exact_complete_body_boundaries_are_enforced_without_dropping_evidence() {
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut first = Server::new(Reply::Body(answer(false)));
    let mut progress = |_| Ok(());
    let provider = backend(&first.endpoint, Default::default(), &mut progress);
    rr::Provider::research(&provider, &raw(&prior)).unwrap();
    let sent = first.received.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(first.finish(), 1);
    drop(provider);
    let request_bytes = serde_json::to_vec(&sent.body).unwrap().len();
    let response_bytes = serde_json::to_vec(&answer(false)).unwrap().len();
    for (request_cap, response_cap, ok, requests) in [
        (request_bytes, response_bytes, true, 1),
        (request_bytes - 1, response_bytes, false, 0),
        (request_bytes, response_bytes - 1, false, 1),
    ] {
        let mut server = Server::new(Reply::Body(answer(false)));
        let provider = backend(
            &server.endpoint,
            TransportOptions {
                max_request_bytes: request_cap,
                max_response_bytes: response_cap,
                ..Default::default()
            },
            &mut progress,
        );
        let result = rr::Provider::research(&provider, &raw(&prior));
        assert_eq!(result.is_ok(), ok);
        assert_eq!(server.finish(), requests);
    }
}

#[test]
fn opt_in_observation_hashes_identity_consuming_take_and_reset() {
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut server = Server::new(Reply::Sequence(vec![
        Reply::Body(answer(false)),
        Reply::Body(answer(false)),
        Reply::Body(json!({"status":"incomplete"})),
        Reply::Body(answer(false)),
    ]));
    let mut progress = |_| Ok(());
    let provider =
        backend(&server.endpoint, Default::default(), &mut progress).with_exchange_observation();
    let r = raw(&prior);
    rr::Provider::research(&provider, &r).unwrap();
    let captured = server
        .received
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let observation = provider.take_exchange_observation().unwrap();
    assert_eq!(
        observation.request_sha256(),
        digest(&serde_json::to_vec(&captured.body).unwrap())
    );
    assert_eq!(
        observation.response_bytes(),
        serde_json::to_vec(&answer(false)).unwrap()
    );
    assert_eq!(
        observation.response_sha256(),
        digest(observation.response_bytes())
    );
    assert_eq!(observation.identity(), &r.identity);
    assert!(provider.take_exchange_observation().is_none());
    rr::Provider::research(&provider, &r).unwrap();
    let mut invalid = raw(&prior);
    invalid.identity.context_sha256 = digest(b"foreign");
    assert!(rr::Provider::research(&provider, &invalid).is_err());
    assert!(provider.take_exchange_observation().is_none());
    assert!(rr::Provider::research(&provider, &r).is_err());
    assert!(provider.take_exchange_observation().is_none());
    rr::Provider::research(&provider, &r).unwrap();
    assert!(provider.take_exchange_observation().is_some());
    assert_eq!(server.finish(), 4);
}
#[test]
fn late_cancel_panic_timeout_and_invalid_attempt_cannot_retain_stale_success() {
    for phase in [
        Progress::BeforeDispatch,
        Progress::ResponseReceived,
        Progress::BeforeReturn,
    ] {
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(Reply::Body(answer(false)));
        let canceled = Cell::new(false);
        let mut progress = |p| {
            if canceled.get() && p == phase {
                bail!("authored cancel");
            }
            Ok(())
        };
        let provider = backend(&server.endpoint, Default::default(), &mut progress)
            .with_exchange_observation();
        rr::Provider::research(&provider, &raw(&prior)).unwrap();
        canceled.set(true);
        assert!(rr::Provider::research(&provider, &raw(&prior)).is_err());
        assert!(provider.take_exchange_observation().is_none());
        assert_eq!(
            server.finish(),
            if phase == Progress::BeforeDispatch {
                1
            } else {
                2
            }
        );
    }
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut server = Server::new(Reply::Body(answer(false)));
    let panic_now = Cell::new(false);
    let mut progress = |p| {
        if panic_now.get() && p == Progress::BeforeReturn {
            panic!("authored late panic");
        }
        Ok(())
    };
    let provider =
        backend(&server.endpoint, Default::default(), &mut progress).with_exchange_observation();
    rr::Provider::research(&provider, &raw(&prior)).unwrap();
    panic_now.set(true);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rr::Provider::research(&provider, &raw(&prior))
    }));
    assert!(result.is_err());
    assert!(provider.take_exchange_observation().is_none());
    assert_eq!(server.finish(), 2);
    let mut server = Server::new(Reply::Sequence(vec![
        Reply::Body(answer(false)),
        Reply::HoldHeaders,
    ]));
    let mut progress = |_| Ok(());
    let provider = backend(
        &server.endpoint,
        TransportOptions {
            timeout: Duration::from_secs(1),
            ..Default::default()
        },
        &mut progress,
    )
    .with_exchange_observation();
    rr::Provider::research(&provider, &raw(&prior)).unwrap();
    assert!(rr::Provider::research(&provider, &raw(&prior)).is_err());
    assert!(provider.take_exchange_observation().is_none());
    assert_eq!(server.finish(), 2);
}
#[test]
fn rejected_reentrant_attempt_does_not_interfere_with_outer_exchange() {
    let f = Fixture::new();
    let (prior, _, _, _) = saved_policy::frozen(&f);
    let mut server = Server::new(Reply::Body(answer(false)));
    let slot: RefCell<Option<&OpenAIWebResearch<'_>>> = RefCell::new(None);
    let reentered = Cell::new(false);
    let mut progress = |phase| {
        if phase == Progress::BeforeReturn {
            let p = slot.borrow();
            let p = p.unwrap();
            let error = rr::Provider::research(p, &raw(&prior)).unwrap_err();
            assert_eq!(error.to_string(), "web research already active");
            reentered.set(true);
        }
        Ok(())
    };
    let provider =
        backend(&server.endpoint, Default::default(), &mut progress).with_exchange_observation();
    *slot.borrow_mut() = Some(&provider);
    rr::Provider::research(&provider, &raw(&prior)).unwrap();
    assert!(reentered.get());
    assert!(provider.take_exchange_observation().is_some());
    assert_eq!(server.finish(), 1);
    *slot.borrow_mut() = None;
}

#[test]
fn current_and_legacy_query_witnesses_preserve_actual_text_and_deduplicate_only_exact_matches() {
    for kind in 0..3 {
        let mut body = answer(false);
        let action = body["output"][0]["action"].as_object_mut().unwrap();
        match kind {
            0 => {
                action.insert("queries".into(), Value::Null);
                action.insert("query".into(), json!("  legacy exact query  "));
            }
            1 => {
                action.insert("queries".into(), json!(["current query", "current query"]));
                action.insert("query".into(), Value::Null);
            }
            _ => {
                action.insert("queries".into(), json!([]));
                action.insert("query".into(), json!("single witnessed query"));
            }
        }
        let f = Fixture::new();
        let (prior, _, _, _) = saved_policy::frozen(&f);
        let mut server = Server::new(Reply::Body(body));
        let mut progress = |_| Ok(());
        let provider = backend(&server.endpoint, Default::default(), &mut progress);
        let result = rr::Provider::research(&provider, &raw(&prior)).unwrap();
        assert_eq!(
            result.queries,
            vec![match kind {
                0 => "  legacy exact query  ",
                1 => "current query",
                _ => "single witnessed query",
            }]
        );
        assert_eq!(server.finish(), 1);
    }
}
