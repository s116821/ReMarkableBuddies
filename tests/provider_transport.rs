//! Linux process-local trust/proxy qualification of the unchanged application client.
#![cfg(target_os = "linux")]
use remarkable_reader_buddy::llm::{openai::OpenAI, LLMEngine};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const CHILD_CONFIG: &str = "BUDDY_OWNED_TRANSPORT_CONFIG";

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("owned JSON file")).expect("owned JSON")
}

fn wait_child(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("owned child status") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("owned child deadline");
        }
        // Poll owned process state; elapsed time is never a completion witness.
        thread::sleep(Duration::from_millis(10));
    }
}

struct Fixture {
    root: PathBuf,
    server: Option<Child>,
    client: Option<Child>,
}
impl Fixture {
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-transport-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::write(root.join(".owned-provider-transport"), b"owned").unwrap();
        let mut fixture = Self {
            root,
            server: None,
            client: None,
        };
        fixture.server = Some(
            Command::new("python3")
                .arg("-I")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/provider_transport_server.py"
                ))
                .arg("--root")
                .arg(&fixture.root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Python3/OpenSSL required for Linux fixture"),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while !fixture.root.join("ready.json").is_file() {
            assert!(
                fixture.server.as_mut().unwrap().try_wait().unwrap().is_none(),
                "owned TLS server exited before readiness; Python3/OpenSSL certificate generation required"
            );
            assert!(Instant::now() < deadline, "owned server startup deadline");
            thread::sleep(Duration::from_millis(10));
        }
        fixture
    }
    fn finish(&mut self) -> Value {
        fs::write(self.root.join("stop"), b"stop").unwrap();
        assert!(wait_child(self.server.as_mut().unwrap(), Duration::from_secs(7)).success());
        self.server = None;
        read_json(&self.root.join("result.json"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.client.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut server) = self.server.take() {
            let _ = fs::write(self.root.join("stop"), b"stop");
            let deadline = Instant::now() + Duration::from_secs(7);
            loop {
                match server.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                    _ => {
                        let _ = server.kill();
                        let _ = server.wait();
                        break;
                    }
                }
            }
        }
        if self.root.join(".owned-provider-transport").is_file() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn transport_client_child() {
    let Some(path) = std::env::var_os(CHILD_CONFIG) else {
        return; // Parent additionally requires an invocation-specific completion file.
    };
    let path = PathBuf::from(path);
    let config = read_json(&path);
    let mut engine = OpenAI::new(
        "owned-transport-fixture".into(),
        "owned-not-a-credential".into(),
        Some(config["url"].as_str().unwrap().into()),
    )
    .with_timeout(Duration::from_secs(5));
    engine.add_text_content("owned synthetic TLS request");
    let result = engine.execute();
    let success = result
        .as_ref()
        .is_ok_and(|s| s == "owned TLS transport answer");
    let rejected = result.is_err();
    fs::write(
        path.with_file_name("witness.json"),
        serde_json::to_vec(
            &json!({"nonce":config["nonce"], "success":success, "rejected":rejected}),
        )
        .unwrap(),
    )
    .unwrap();
    // Do not serialize provider errors or inherited environment.
}

fn qualify(name: &str, trusted: bool, wrong_name: bool, proxy: bool, bypass: bool) {
    let mut fixture = Fixture::start();
    let ready = read_json(&fixture.root.join("ready.json"));
    let port = ready[if wrong_name {
        "wrong_port"
    } else {
        "good_port"
    }]
    .as_u64()
    .unwrap();
    let nonce = uuid::Uuid::new_v4().to_string();
    let config = fixture.root.join("client.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"nonce":nonce, "url":format!("https://127.0.0.1:{port}")}))
            .unwrap(),
    )
    .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .args(["--exact", "transport_client_child", "--nocapture"])
        .env(CHILD_CONFIG, &config)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .env(
            "SSL_CERT_FILE",
            fixture
                .root
                .join(if trusted { "ca.pem" } else { "unrelated.pem" }),
        )
        .env("SSL_CERT_DIR", fixture.root.join("empty-roots"))
        .env("NO_PROXY", if bypass { "127.0.0.1" } else { "" });
    if proxy {
        command.env(
            "HTTPS_PROXY",
            format!(
                "http://owned-proxy:owned-fake-password@127.0.0.1:{}",
                ready["proxy_port"].as_u64().unwrap()
            ),
        );
    }
    fixture.client = Some(command.spawn().expect("owned actual-client child"));
    assert!(wait_child(fixture.client.as_mut().unwrap(), Duration::from_secs(10)).success());
    fixture.client = None;
    let witness = read_json(&fixture.root.join("witness.json"));
    assert_eq!(witness["nonce"], nonce, "exact invocation must complete");
    let result = fixture.finish();
    let owned_root = fixture.root.clone();
    drop(fixture);
    assert!(
        !owned_root.exists(),
        "owned temporary keys/resources removed"
    );
    let success = trusted && !wrong_name;
    assert_eq!(witness["success"], success, "{name}: exact answer");
    assert_eq!(
        witness["rejected"], !success,
        "{name}: result classification"
    );
    assert_eq!(result["failures"], 0, "{name}: fixture failure");
    assert_eq!(
        result["requests"],
        u64::from(success),
        "{name}: application receipt"
    );
    assert_eq!(
        result["valid_requests"],
        u64::from(success),
        "{name}: synthetic request/auth"
    );
    assert_eq!(
        result["tls_accepted"],
        u64::from(success),
        "{name}: TLS acceptance"
    );
    assert_eq!(
        result["tls_rejected"],
        u64::from(!success),
        "{name}: TLS refusal"
    );
    let connects = u64::from(proxy && !bypass);
    assert_eq!(result["connects"], connects, "{name}: route");
    assert_eq!(result["valid_proxy_auth"], connects, "{name}: proxy auth");
    assert_eq!(
        result["origin_proxy_auth"], false,
        "proxy credential must stay out of origin"
    );
    assert_eq!(
        result["plaintext_origin_auth"], false,
        "origin authorization must stay inside TLS"
    );
    eprintln!("{name}: nonce witnessed; {result}");
}
#[test]
fn trusted_root_completes_actual_client_request() {
    qualify("trusted", true, false, false, false);
}
#[test]
fn unrelated_root_rejects_before_application_request() {
    qualify("unrelated-root", false, false, false, false);
}
#[test]
fn trusted_root_wrong_name_rejects_before_application_request() {
    qualify("wrong-name", true, true, false, false);
}
#[test]
fn https_connect_keeps_proxy_and_origin_authorization_separate() {
    qualify("connect", true, false, true, false);
}
#[test]
fn no_proxy_bypasses_configured_https_proxy() {
    qualify("bypass", true, false, true, true);
}
#[test]
fn https_connect_preserves_untrusted_root_rejection() {
    qualify("connect-untrusted", false, false, true, false);
}
