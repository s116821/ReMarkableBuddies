//! Actual HTTP and Reader/Ledger, with modeled ownership loss; no native input claim.
use super::*;
use crate::conversation::{AttemptReason, Outcome, Record, Role};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Server {
    endpoint: String,
    model: String,
    accepted: Option<mpsc::Receiver<()>>,
    release: mpsc::Sender<()>,
    released: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<serde_json::Value>>>,
}
impl Server {
    fn start(withhold: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let model = format!("reader-cancel-{}", crate::storage::Uuid::new_v4());
        let expected_model = model.clone();
        let (accepted, receipt) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let released = Arc::new(AtomicBool::new(false));
        let released_worker = released.clone();
        let worker = thread::spawn(move || {
            let replies = [
                "QUESTION: G unc.?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\n---\nANSWER: Preliminary answer must never be published.",
                "TRANSCRIPTION: G unc.?",
                "ANSWER: Withheld answer must never be published.",
            ];
            let mut requests = vec![];
            for (ordinal, reply) in replies.iter().take(withhold).enumerate() {
                let deadline = Instant::now() + Duration::from_secs(15);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if barrier.try_recv().is_ok() {
                                return requests;
                            }
                            assert!(
                                Instant::now() < deadline,
                                "Reader did not dispatch expected request"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with("POST /v1/chat/completions "));
                let mut length = None;
                let mut headers = line.len();
                let mut authorized = false;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    headers += line.len();
                    assert!(headers < 65_536);
                    if line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                    authorized |= lower.trim() == "authorization: bearer dummy-reader-cancellation";
                }
                assert!(authorized);
                let length = length.unwrap();
                assert!((1..10_000_000).contains(&length));
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(request["model"], expected_model);
                requests.push(request);
                if ordinal + 1 == withhold {
                    accepted.send(()).unwrap();
                    let _ = barrier.recv_timeout(Duration::from_secs(15));
                    released_worker.store(true, Ordering::SeqCst);
                }
                let response =
                    serde_json::json!({"choices":[{"message":{"content":reply}}]}).to_string();
                // Peer closure after cancellation is expected; never convert it into success.
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response);
            }
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
                "Unexpected extra provider connection after request ownership loss"
            );
            requests
        });
        Self {
            endpoint,
            model,
            accepted: Some(receipt),
            release,
            released,
            worker: Some(worker),
        }
    }
    fn finish(&mut self) -> Vec<serde_json::Value> {
        let _ = self.release.send(());
        self.worker.take().unwrap().join().unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct GuardedProvider {
    inner: OpenAI,
    state: Shared,
    accepted: mpsc::Receiver<()>,
    rejection: Option<Instant>,
    return_latency: Rc<RefCell<Option<Duration>>>,
}
impl LLMEngine for GuardedProvider {
    fn add_text_content(&mut self, text: &str) {
        self.inner.add_text_content(text);
    }
    fn add_image_content(&mut self, image: &str) {
        self.inner.add_image_content(image);
    }
    fn clear_content(&mut self) {
        self.inner.clear_content();
    }
    fn execute(&mut self) -> Result<String> {
        self.execute_with_progress(&mut || Ok(()))
    }
    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        let Self {
            inner,
            state,
            accepted,
            rejection,
            return_latency,
        } = self;
        let result = inner.execute_with_progress(&mut || {
            if accepted.try_recv().is_ok() {
                let mut state = state.borrow_mut();
                state.visit += 1;
                state.event(
                    "test_ownership_loss",
                    "after complete owned HTTP request receipt",
                );
            }
            let result = progress();
            if result.is_err() && rejection.is_none() {
                *rejection = Some(Instant::now());
            }
            result
        });
        if let Some(rejected) = rejection {
            *return_latency.borrow_mut() = Some(rejected.elapsed());
        }
        result
    }
}

#[test]
fn actual_reader_cancels_recognition_and_context_wait_without_page_output() {
    for withhold in [1, 3] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
        let scenario: Scenario =
            serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap())
                .unwrap();
        let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
        let workflow = Workflow::with_device(Box::new(SimDevice(state.clone())), false);
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open().unwrap();
        let store = ledger.store().clone();
        let mut server = Server::start(withhold);
        let latency = Rc::new(RefCell::new(None));
        let model = GuardedProvider {
            inner: OpenAI::new(
                server.model.clone(),
                "dummy-reader-cancellation".into(),
                Some(server.endpoint.clone()),
            )
            .with_timeout(Duration::from_secs(5)),
            state: state.clone(),
            accepted: server.accepted.take().unwrap(),
            rejection: None,
            return_latency: latency.clone(),
        };
        let mut reader = Orchestrator::new(workflow, model, ledger);
        reader.set_trigger_enabled(false);
        reader.set_personalization(
            crate::config::Personalization {
                enabled: true,
                text: "Explain briefly.".into(),
                ..Default::default()
            }
            .snapshot()
            .unwrap(),
        );
        let result = reader.run_iteration();
        let repeated = reader.run_iteration();
        let returned_while_withheld = !server.released.load(Ordering::SeqCst);
        let requests = server.finish(); // Release and join before assertions, including failures.
        assert!(result.is_err());
        assert!(
            repeated.is_err(),
            "Sticky lost input admitted another Reader iteration"
        );
        assert!(
            returned_while_withheld,
            "Reader waited for withheld server release"
        );
        let duration = latency
            .borrow()
            .expect("Actual progress rejection was not witnessed");
        assert!(
            duration < Duration::from_millis(250),
            "Provider cancellation return took {duration:?}"
        );
        assert_eq!(requests.len(), withhold);
        let state = state.borrow();
        assert!(state.request_lost);
        assert_eq!(state.pages[0].image(), state.initial[0]);
        assert!(state.pages.iter().all(|page| page.text.is_empty()));
        assert!(!state
            .events
            .iter()
            .any(|event| matches!(event.action.as_str(), "next" | "previous" | "text" | "body")));
        let records: Vec<Record> = store
            .snapshot_heads_matching(
                &[
                    crate::storage::Namespace::Conversation,
                    crate::storage::Namespace::Source,
                ],
                4096,
                |_| true,
            )
            .unwrap()
            .into_iter()
            .filter_map(|entry| serde_json::from_value(entry.payload).ok())
            .collect();
        let user = records
            .iter()
            .find_map(|record| match record {
                Record::Turn(turn) if turn.role == Role::User => Some(turn),
                _ => None,
            })
            .unwrap();
        assert_eq!(user.outcome, Outcome::Canceled);
        assert_eq!(user.text.as_deref(), (withhold == 3).then_some("G unc.?"));
        assert!(records.iter().any(|record| matches!(record, Record::OutcomeFact(fact) if fact.turn == user.id && fact.outcome == Outcome::Canceled && fact.reason == AttemptReason::Canceled)));
        assert!(!records
            .iter()
            .any(|record| matches!(record, Record::Turn(turn) if turn.role == Role::Assistant)));
        let capture = records
            .iter()
            .find_map(|record| match record {
                Record::LegacyCapture(capture) => Some(capture),
                _ => None,
            })
            .unwrap();
        let stored = crate::conversation::Ledger::new(store)
            .stored_legacy_images(capture.id)
            .unwrap();
        let sent_images: Vec<_> = requests[0]["messages"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|part| part["image_url"]["url"].as_str())
            .collect();
        assert_eq!(stored.images.len(), sent_images.len());
        for (bytes, image) in stored.images.iter().zip(sent_images) {
            assert_eq!(
                bytes,
                &STANDARD
                    .decode(image.strip_prefix("data:image/png;base64,").unwrap())
                    .unwrap()
            );
        }
        eprintln!("reader-stage={withhold} provider-cancel-return-ms={} withheld-at-reader-return=true requests={}", duration.as_millis(), requests.len());
    }
}
