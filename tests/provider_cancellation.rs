//! Owned provider readiness controls; original scoped-join failure is retained in evidence.
use remarkable_reader_buddy::llm::{openai::OpenAI, LLMEngine};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
struct OwnedThreads {
    release: mpsc::Sender<()>,
    provider: Option<thread::JoinHandle<usize>>,
    client: Option<thread::JoinHandle<()>>,
}
impl OwnedThreads {
    fn finish(&mut self) -> usize {
        let _ = self.release.send(());
        let client = self.client.take().unwrap();
        let provider = self.provider.take().unwrap();
        let client_result = client.join();
        let provider_result = provider.join();
        client_result.expect("owned client panic");
        provider_result.expect("owned provider panic")
    }
}
impl Drop for OwnedThreads {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(client) = self.client.take() {
            let _ = client.join();
        }
        if let Some(provider) = self.provider.take() {
            let _ = provider.join();
        }
    }
}
fn request(stream: &mut TcpStream) -> serde_json::Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 16 * 1024, "owned header bound");
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
    let text = std::str::from_utf8(&header).unwrap();
    assert!(text.starts_with("POST /v1/chat/completions "));
    let length = text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .expect("owned bounded content length");
    assert!(length > 0 && length <= 16 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["model"], "owned-fixture");
    value
}
#[test]
fn canceled_client_returns_before_withheld_provider_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(1);
    let (cancel_tx, cancel_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let provider = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "owned accept deadline");
                    // Bounded fixture accept polling; elapsed time never proves request receipt.
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("owned accept failure: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        request(&mut stream);
        accepted_tx.send(()).unwrap();
        release_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("owned provider release");
        let body = r#"{"choices":[{"message":{"content":"owned late answer"}}]}"#;
        let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
        let _ = stream.write_all(response.as_bytes());
        1
    });
    let client = thread::spawn(move || {
        let mut engine = OpenAI::new(
            "owned-fixture".into(),
            "owned-not-a-credential".into(),
            Some(format!("http://{address}")),
        )
        .with_timeout(Duration::from_secs(5));
        engine.add_text_content("owned synthetic request");
        let mut witnessed = false;
        let result = engine.execute_with_progress(&mut || {
            if !witnessed && accepted_rx.try_recv().is_ok() {
                witnessed = true;
                cancel_tx.send(Instant::now()).unwrap();
                anyhow::bail!("owned cancellation witness");
            }
            Ok(())
        });
        result_tx.send((Instant::now(), result)).unwrap();
    });
    let mut threads = OwnedThreads {
        release: release_tx,
        provider: Some(provider),
        client: Some(client),
    };
    let canceled = cancel_rx
        .recv_timeout(Duration::from_secs(4))
        .expect("exact cancellation after request receipt");
    let ready = result_rx.recv_timeout(Duration::from_millis(250));
    let prompt = ready.is_ok();
    let requests = threads.finish();
    let (returned, result) =
        ready.unwrap_or_else(|_| result_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    assert_eq!(requests, 1);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("owned cancellation witness"));
    eprintln!(
        "owned request_count={requests} cancel_to_return_ms={} readiness_deadline_expired={}",
        returned.duration_since(canceled).as_millis(),
        !prompt
    );
    assert!(prompt,"client readiness waited for provider response despite witnessed cancellation; original correctness gate remains failed");
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "owned accept deadline");
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("owned accept failure: {e}"),
        }
    }
}
fn reply(stream: &mut TcpStream, text: &str) {
    let body = serde_json::json!({"choices":[{"message":{"content":text}}]}).to_string();
    let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
    let _ = stream.write_all(response.as_bytes());
}
#[test]
fn next_request_uses_fresh_content_while_previous_reply_is_still_withheld() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(1);
    let (cancel_tx, cancel_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let provider = thread::spawn(move || {
        thread::scope(|scope| {
            let mut first = accept(&listener);
            let first_body = request(&mut first);
            assert_eq!(
                first_body["messages"][0]["content"][0]["text"],
                "original request"
            );
            accepted_tx.send(()).unwrap();
            let original = scope.spawn(move || {
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                reply(&mut first, "stale original answer");
            });
            let mut next = accept(&listener);
            let next_body = request(&mut next);
            assert_eq!(
                next_body["messages"][0]["content"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                next_body["messages"][0]["content"][0]["text"],
                "fresh next request"
            );
            reply(&mut next, "fresh next answer");
            original.join().unwrap();
            2
        })
    });
    let client = thread::spawn(move || {
        let mut engine = OpenAI::new(
            "owned-fixture".into(),
            "owned-not-a-credential".into(),
            Some(format!("http://{address}")),
        )
        .with_timeout(Duration::from_secs(5));
        engine.add_text_content("original request");
        let first = engine.execute_with_progress(&mut || {
            if accepted_rx.try_recv().is_ok() {
                cancel_tx.send(Instant::now()).unwrap();
                anyhow::bail!("owned first cancellation");
            }
            Ok(())
        });
        assert!(first
            .unwrap_err()
            .to_string()
            .contains("owned first cancellation"));
        engine.clear_content();
        engine.add_text_content("fresh next request");
        let second = engine.execute();
        result_tx.send(second).unwrap();
    });
    let mut owned = OwnedThreads {
        release: release_tx,
        provider: Some(provider),
        client: Some(client),
    };
    cancel_rx.recv_timeout(Duration::from_secs(4)).unwrap();
    let ready = result_rx.recv_timeout(Duration::from_millis(500));
    let prompt = ready.is_ok();
    assert_eq!(owned.finish(), 2);
    let second = ready
        .unwrap_or_else(|_| result_rx.recv_timeout(Duration::from_secs(1)).unwrap())
        .unwrap();
    assert!(prompt, "next request awaited old response");
    assert_eq!(second, "fresh next answer");
}
#[test]
fn initial_rejection_sends_no_http_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut engine = OpenAI::new(
        "owned-fixture".into(),
        "owned-not-a-credential".into(),
        Some(format!("http://{}", listener.local_addr().unwrap())),
    );
    let error = engine
        .execute_with_progress(&mut || anyhow::bail!("owned predispatch refusal"))
        .unwrap_err();
    assert_eq!(error.to_string(), "owned predispatch refusal");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[test]
fn synchronous_provider_api_works_inside_current_thread_tokio_caller() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept(&listener);
        request(&mut stream);
        reply(&mut stream, "tokio caller answer");
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let mut engine = OpenAI::new(
            "owned-fixture".into(),
            "owned-not-a-credential".into(),
            Some(format!("http://{address}")),
        )
        .with_timeout(Duration::from_secs(5));
        engine.execute()
    });
    server.join().unwrap();
    assert_eq!(result.unwrap(), "tokio caller answer");
}
#[test]
fn callback_panic_cancels_and_joins_before_withheld_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(1);
    let (panic_tx, panic_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let provider = thread::spawn(move || {
        let mut stream = accept(&listener);
        request(&mut stream);
        accepted_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        reply(&mut stream, "discarded answer");
        1
    });
    let client = thread::spawn(move || {
        let mut engine = OpenAI::new(
            "owned-fixture".into(),
            "owned-not-a-credential".into(),
            Some(format!("http://{address}")),
        )
        .with_timeout(Duration::from_secs(5));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.execute_with_progress(&mut || {
                if accepted_rx.try_recv().is_ok() {
                    panic_tx.send(()).unwrap();
                    panic!("owned callback panic");
                }
                Ok(())
            })
        }))
        .is_err();
        result_tx.send(panicked).unwrap();
    });
    let mut owned = OwnedThreads {
        release: release_tx,
        provider: Some(provider),
        client: Some(client),
    };
    panic_rx.recv_timeout(Duration::from_secs(4)).unwrap();
    let returned = result_rx.recv_timeout(Duration::from_millis(250));
    let prompt = returned.is_ok();
    owned.finish();
    let panicked =
        returned.unwrap_or_else(|_| result_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    assert!(panicked && prompt, "callback panic joined blocked provider");
}
