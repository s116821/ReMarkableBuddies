use remarkable_reader_buddy::simulator::scenario::Scenario;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn fixture() -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut value: Value = serde_json::from_slice(
        &std::fs::read(root.join("docs/simulator/scenarios/blank-answer.json")).unwrap(),
    )
    .unwrap();
    value["pages"][0]["image"] = root
        .join("docs/validation/images/rem13/final-input.png")
        .to_string_lossy()
        .to_string()
        .into();
    value["llm"] = json!({"mode":"live","model":"local-fixture","max_calls":2,"timeout_seconds":2});
    value["replies"] = json!([]);
    value["output"] = "result".into();
    value
}

fn directory() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!(
            "live-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn invoke(path: &Path, value: &Value, endpoint: &str, key: &str) -> std::process::Output {
    let scenario = path.join("scenario.json");
    std::fs::write(&scenario, serde_json::to_vec(value).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .current_dir(path)
        .args(["--simulate", scenario.to_str().unwrap()])
        .env("OPENAI_API_KEY", key)
        .env("OPENAI_BASE_URL", endpoint)
        .output()
        .unwrap()
}

fn server(replies: Vec<String>, stall: bool) -> (String, thread::JoinHandle<Vec<Value>>) {
    server_with_delay(replies, stall, Duration::ZERO)
}

fn server_with_delay(
    replies: Vec<String>,
    stall: bool,
    delay: Duration,
) -> (String, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut bodies = Vec::new();
        for reply in replies {
            let deadline = Instant::now() + Duration::from_secs(15);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "No local fixture request");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /v1/chat/completions "));
            let mut length = 0;
            let mut authorized = false;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
                if lower.trim() == "authorization: bearer local-test-secret-not-real" {
                    authorized = true;
                }
            }
            assert!(authorized);
            assert!((1..10_000_000).contains(&length));
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            bodies.push(serde_json::from_slice(&body).unwrap());
            if stall {
                thread::sleep(Duration::from_secs(3));
                continue;
            }
            thread::sleep(delay);
            let body = json!({"model":"local-fixture","choices":[{"message":{"content":reply}}]})
                .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        bodies
    });
    (endpoint, handle)
}

#[test]
fn delayed_success_refreshes_staged_paths_and_cleans_both_pages() {
    let path = directory();
    let (endpoint, handle) = server_with_delay(replies(), false, Duration::from_millis(1100));
    let mut value = fixture();
    value["llm"]["timeout_seconds"] = 5.into();
    let output = invoke(&path, &value, &endpoint, "local-test-secret-not-real");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(handle.join().unwrap().len(), 2);
    let report: Value =
        serde_json::from_slice(&std::fs::read(path.join("result/report.json")).unwrap()).unwrap();
    let source_ticks = report["trace"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["page"] == 0 && event["action"] == "statusstroke")
        .count();
    assert!(source_ticks >= 4);
    assert!(report["pages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|page| page["indicator_visible"] == false));
    assert_eq!(report["pages"][0]["unchanged"], true);
    std::fs::remove_dir_all(path).unwrap();
}

fn replies() -> Vec<String> {
    let value: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs/simulator/scenarios/blank-answer.json"),
        )
        .unwrap(),
    )
    .unwrap();
    value["replies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["text"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn live_schema_rejects_mixed_or_unbounded_configuration() {
    for config in [
        json!({"mode":"live","max_calls":0}),
        json!({"mode":"live","max_calls":201}),
        json!({"mode":"live","timeout_seconds":0}),
        json!({"mode":"live","timeout_seconds":301}),
        json!({"mode":"live","model":" "}),
    ] {
        let mut value = fixture();
        value["llm"] = config;
        assert!(serde_json::from_value::<Scenario>(value)
            .unwrap()
            .validate()
            .is_err());
    }
    let mut value = fixture();
    value["llm"]["api_key"] = "must-not-be-in-scenario".into();
    assert!(serde_json::from_value::<Scenario>(value).is_err());
    let mut value = fixture();
    value["replies"] = json!([{"text":"NONE"}]);
    assert!(serde_json::from_value::<Scenario>(value)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn blank_credentials_fail_before_device_or_network() {
    let path = directory();
    let mut value = fixture();
    value["pages"][0]["image"] = "nonexistent.png".into();
    let output = invoke(&path, &value, "http://127.0.0.1:1", "");
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("OPENAI_API_KEY is empty"), "{error}");
    assert!(!path.join("result").exists());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn live_cli_delegates_real_content_and_exports_without_credentials() {
    let path = directory();
    let (endpoint, handle) = server(replies(), false);
    let mut value = fixture();
    value["expect"]["text_contains"] = json!({"1":["6.674215","0.000092"]});
    let output = invoke(&path, &value, &endpoint, "local-test-secret-not-real");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bodies = handle.join().unwrap();
    assert_eq!(bodies.len(), 2);
    for body in bodies {
        assert_eq!(body["model"], "local-fixture");
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content
                .iter()
                .filter(|part| part["type"] == "image_url")
                .count(),
            4
        );
        assert!(content.iter().any(|part| part["type"] == "text"));
    }
    let report = std::fs::read_to_string(path.join("result/report.json")).unwrap();
    assert!(!report.contains("local-test-secret-not-real"));
    assert!(!report.contains(&endpoint));
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["model_mode"], "live-provider");
    assert_eq!(report["model_calls"], 2);
    assert_eq!(report["pages"][0]["unchanged"], true);
    assert!(path.join("result/page-1.png").exists());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn call_limit_prevents_second_request_and_retains_report() {
    let path = directory();
    let (endpoint, handle) = server(vec![replies().remove(0)], false);
    let mut value = fixture();
    value["llm"]["max_calls"] = 1.into();
    let output = invoke(&path, &value, &endpoint, "local-test-secret-not-real");
    assert!(!output.status.success());
    assert_eq!(handle.join().unwrap().len(), 1);
    let report: Value =
        serde_json::from_slice(&std::fs::read(path.join("result/report.json")).unwrap()).unwrap();
    assert_eq!(report["model_calls"], 1);
    assert_eq!(report["active_page"], 0);
    assert!(report["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["action"] == "model_limit"));
    assert_eq!(report["pages"][1]["text"], "");
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn live_timeout_is_bounded_and_retains_failure_evidence() {
    let path = directory();
    let (endpoint, handle) = server(vec!["NONE".into()], true);
    let mut value = fixture();
    value["llm"]["timeout_seconds"] = 1.into();
    let output = invoke(&path, &value, &endpoint, "local-test-secret-not-real");
    assert!(!output.status.success());
    assert_eq!(handle.join().unwrap().len(), 1);
    let report: Value =
        serde_json::from_slice(&std::fs::read(path.join("result/report.json")).unwrap()).unwrap();
    assert_eq!(report["model_calls"], 1);
    assert_eq!(report["active_page"], 0);
    assert_eq!(report["pages"][0]["indicator_visible"], false);
    assert!(report["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["action"] == "statusclear"));
    assert!(String::from_utf8_lossy(&output.stderr)
        .to_lowercase()
        .contains("timeout"));
    assert!(report["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["detail"] == "live error"));
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn pending_http_ticks_on_caller_thread_and_callback_failure_stops_ticks() {
    use remarkable_reader_buddy::{LLMEngine, OpenAI};
    let (endpoint, handle) = server(vec!["NONE".into()], true);
    let mut model = OpenAI::new(
        "local-fixture".into(),
        "local-test-secret-not-real".into(),
        Some(endpoint),
    )
    .with_timeout(Duration::from_millis(1200));
    model.add_text_content("bounded progress test");
    let caller = thread::current().id();
    let mut ticks = 0;
    let start = Instant::now();
    let result = model.execute_with_progress(&mut || {
        assert_eq!(thread::current().id(), caller);
        ticks += 1;
        if ticks == 2 {
            anyhow::bail!("progress fixture failed");
        }
        Ok(())
    });
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("progress fixture failed"));
    assert_eq!(ticks, 2);
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(handle.join().unwrap().len(), 1);
}
