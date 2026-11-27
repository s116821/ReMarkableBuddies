use super::*;
use crate::{
    config::capacities::tests::{grant_fixture, Temp},
    storage::StorePaths,
};
use serde_json::json;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[path = "grant_command_tests.rs"]
mod grant_maintenance;

struct Fixture {
    _temp: Temp,
    path: PathBuf,
    paths: StorePaths,
    binding: Binding,
}

impl Fixture {
    fn new(initialize: bool) -> Self {
        let temp = Temp::new();
        let paths = StorePaths {
            data: temp.path().join("data"),
            cache: temp.path().join("cache"),
            credentials: temp.path().join("credentials"),
        };
        let tablet_id = if initialize {
            Store::open(paths.clone()).unwrap().actor_id
        } else {
            Uuid::new_v4()
        };
        let path = temp.path().join("config.json");
        let bytes = serde_json::to_vec(&json!({
            "paths": paths,
            "product_settings": {"writer_enabled": false, "export_destination": "mem"}
        }))
        .unwrap();
        fs::write(&path, &bytes).unwrap();
        let binding = Binding {
            tablet_id: tablet_id.to_string(),
            connection_id: "1".repeat(64),
            boot_id: Uuid::new_v4(),
            config_sha256: digest(&bytes),
            operation_id: Uuid::new_v4(),
            grant_id: Uuid::new_v4(),
        };
        Self {
            _temp: temp,
            path,
            paths,
            binding,
        }
    }

    async fn act(&self, selector: &str, registration: Option<&Registration>) -> Report {
        self.act_as(selector, registration, &self.binding).await
    }

    async fn act_as(
        &self,
        selector: &str,
        registration: Option<&Registration>,
        binding: &Binding,
    ) -> Report {
        let mut frame = json!({
            "contract_version": 1,
            "selector": selector,
            "budget_ms": 30_000,
            "binding": binding
        });
        if selector == "exchange" {
            frame["proof"] = json!({
                "code": "synthetic+code",
                "verifier": "v".repeat(43),
                "client_id": "fixture-public-client",
                "redirect_uri": "http://127.0.0.1:48761/oauth/capacities/callback",
                "resource": "https://api.capacities.io"
            });
        }
        let (_signal, canceled) = watch::channel(false);
        handle(
            Some(self.path.clone()),
            &serde_json::to_vec(&frame).unwrap(),
            Instant::now(),
            canceled,
            registration,
            Some(self.binding.boot_id),
        )
        .await
    }

    fn grant_path(&self, extension: &str) -> PathBuf {
        self.paths.credentials.join(format!(
            "capacities-grant-{}.{extension}",
            self.binding.grant_id
        ))
    }
}

fn capture_fixture(fixture: &Fixture, selector: &str, boot: Option<Uuid>) -> CaptureReport {
    let frame = serde_json::to_vec(&json!({
        "contract_version": 1,
        "selector": selector,
        "budget_ms": 30000,
        "binding": {
            "connection_id": fixture.binding.connection_id,
            "boot_id": fixture.binding.boot_id
        }
    }))
    .unwrap();
    let (_signal, canceled) = watch::channel(false);
    capture(
        Some(fixture.path.clone()),
        CaptureRequest::parse(&frame).unwrap(),
        Instant::now(),
        canceled,
        boot,
    )
}

#[test]
fn capture_proves_actual_existing_actor_boot_and_config_without_creating_storage() {
    let missing = Fixture::new(false);
    let report = capture_fixture(&missing, "capture", Some(missing.binding.boot_id));
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        json!({
            "contract_version": 1, "status": "unresolved"
        })
    );
    assert!(!missing.paths.data.exists());
    assert!(!missing.paths.credentials.exists());

    let existing = Fixture::new(true);
    assert_eq!(
        capture_fixture(&existing, "capture", Some(Uuid::new_v4())).status,
        "unresolved"
    );
    let report = capture_fixture(&existing, "capture", Some(existing.binding.boot_id));
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        json!({
            "contract_version": 1, "status": "captured",
            "tablet_id": existing.binding.tablet_id,
            "connection_id": existing.binding.connection_id,
            "boot_id": existing.binding.boot_id,
            "config_sha256": existing.binding.config_sha256
        })
    );
    assert!(Store::open_existing(existing.paths.clone()).is_err());
    drop(report);
    assert!(Store::open_existing(existing.paths.clone()).is_ok());
    assert!(!existing.grant_path("lock").exists());
    assert!(config_matches(
        &existing.path,
        &existing.binding.config_sha256
    ));
}

#[test]
fn capture_connection_reports_only_the_saved_selection_under_original_custody() {
    let fixture = Fixture::new(true);
    let read = |selector| capture_fixture(&fixture, selector, Some(fixture.binding.boot_id));
    let mut expected = json!({
        "contract_version": 1,
        "status": "captured",
        "tablet_id": fixture.binding.tablet_id,
        "connection_id": fixture.binding.connection_id,
        "boot_id": fixture.binding.boot_id,
        "config_sha256": fixture.binding.config_sha256,
        "connection": null
    });
    assert_eq!(
        serde_json::to_value(read("capture-connection")).unwrap(),
        expected
    );

    let original = fs::read(&fixture.path).unwrap();
    let mut config: Value = serde_json::from_slice(&original).unwrap();
    let grant = fixture.binding.grant_id;
    let space = Uuid::new_v4();
    config["capacities_export"] = json!({
        "schema_version": 1,
        "grant_id": grant,
        "space_id": space,
        "tags": [Uuid::new_v4()]
    });
    let selected = serde_json::to_vec(&config).unwrap();
    fs::write(&fixture.path, &selected).unwrap();
    expected["config_sha256"] = json!(digest(&selected));
    expected["connection"] = json!({ "grant_id": grant, "space_id": space });
    let report = read("capture-connection");
    assert_eq!(serde_json::to_value(&report).unwrap(), expected);
    assert!(Store::open_existing(fixture.paths.clone()).is_err());
    assert!(!fixture.grant_path("lock").exists());
    assert!(!fixture.grant_path("json").exists());
    assert_eq!(fs::read(&fixture.path).unwrap(), selected);
    drop(report);

    expected.as_object_mut().unwrap().remove("connection");
    assert_eq!(serde_json::to_value(read("capture")).unwrap(), expected);
    config["capacities_export"]["grant_id"] = json!(Uuid::nil());
    fs::write(&fixture.path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(read("capture-connection")).unwrap(),
        json!({ "contract_version": 1, "status": "unresolved" })
    );
}

#[test]
fn attested_actor_admits_only_exact_mutating_handoffs() {
    let fixture = Fixture::new(false);
    let mut request = json!({
        "contract_version": 1,
        "selector": "disconnect",
        "budget_ms": 30_000,
        "binding": fixture.binding,
        "space_id": Uuid::new_v4(),
        "expected_grant_sha256": "a".repeat(64)
    });
    let admits = |body: &Value| admits_attested_request(&serde_json::to_vec(body).unwrap());
    assert!(admits(&request));
    request["selector"] = json!("refresh");
    assert!(!admits(&request));
    request["selector"] = json!("disconnect");
    request
        .as_object_mut()
        .unwrap()
        .remove("expected_grant_sha256");
    assert!(!admits(&request));
    request["selector"] = json!("grant-status");
    assert!(!admits(&request));
    request.as_object_mut().unwrap().remove("space_id");
    request["selector"] = json!("status");
    assert!(!admits(&request));
    request["selector"] = json!("exchange");
    request["proof"] = json!({
        "code": "synthetic-code",
        "verifier": "v".repeat(43),
        "client_id": "fixture-public-client",
        "redirect_uri": "http://127.0.0.1:48761/oauth/capacities/callback",
        "resource": "https://api.capacities.io"
    });
    assert!(admits(&request));
}

#[test]
fn launched_executable_stamp_requires_exact_bounded_bytes_before_readiness() {
    let bytes = b"synthetic executable witness";
    let deadline = Instant::now() + Duration::from_secs(5);
    let size = bytes.len() as u64;
    let mut frame = Vec::new();
    executable_stamp(&bytes[..], size, deadline, &mut frame).unwrap();
    assert_eq!(
        frame,
        format!("capacities-actor-exe-v1 {}\n", digest(bytes)).into_bytes()
    );

    for (bad_size, until) in [
        (0, deadline),
        (size - 1, deadline),
        (size + 1, deadline),
        (MAX_EXECUTABLE + 1, deadline),
        (size, Instant::now()),
    ] {
        frame.clear();
        assert!(executable_stamp(&bytes[..], bad_size, until, &mut frame).is_err());
        assert!(frame.is_empty());
    }
}

#[tokio::test]
async fn unregistered_exchange_and_absent_original_status_do_not_create_storage() {
    let empty = Fixture::new(false);
    let registration = Registration::production();
    assert!(registration.is_none());
    let report = empty.act("exchange", registration.as_ref()).await;
    assert_eq!(report.status, Outcome::NotDispatched);
    assert_eq!(empty.act("status", None).await.status, Outcome::Unresolved);
    assert!(!empty.paths.data.exists());
    assert!(!empty.paths.credentials.exists());

    let ready = Fixture::new(true);
    assert_eq!(
        ready.act("status", None).await.status,
        Outcome::NotDispatched
    );
    assert_eq!(
        ready.act("resume", None).await.status,
        Outcome::NotDispatched
    );
    assert!(!ready.grant_path("lock").exists());
    assert!(!ready.grant_path("json").exists());
    assert!(config_matches(&ready.path, &ready.binding.config_sha256));
}

async fn request(stream: &mut TcpStream) -> (String, String) {
    let mut bytes = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        assert!(count > 0 && bytes.len() + count <= 12 * 1024);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
            if bytes.len() == header_end + 4 + length {
                return (
                    headers.to_owned(),
                    std::str::from_utf8(&bytes[header_end + 4..])
                        .unwrap()
                        .to_owned(),
                );
            }
        }
    }
}

async fn response(stream: &mut TcpStream, body: Value) {
    let bytes = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
    stream.shutdown().await.unwrap();
}

async fn provider(listener: TcpListener, origin: String, space: Option<Uuid>) {
    let (mut stream, _) = listener.accept().await.unwrap();
    let (headers, _) = request(&mut stream).await;
    assert!(headers.starts_with("GET /.well-known/oauth-authorization-server HTTP/1.1"));
    response(
        &mut stream,
        json!({
            "issuer": origin,
            "authorization_endpoint": format!("{origin}/oauth/authorize"),
            "token_endpoint": format!("{origin}/oauth/token"),
            "code_challenge_methods_supported": ["S256"],
            "grant_types_supported": ["authorization_code"],
            "response_types_supported": ["code"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": ["api:read", "api:write", "offline_access"]
        }),
    )
    .await;
    let (mut stream, _) = listener.accept().await.unwrap();
    let (headers, body) = request(&mut stream).await;
    assert!(headers.starts_with("POST /oauth/token HTTP/1.1"));
    assert!(!headers.contains("synthetic+code"));
    assert!(body.contains("code=synthetic%2Bcode"));
    assert!(body.contains("grant_type=authorization_code"));
    assert!(body.contains(&format!("code_verifier={}", "v".repeat(43))));
    let Some(space) = space else {
        return;
    };
    response(
        &mut stream,
        json!({
            "access_token": "synthetic-access-token",
            "refresh_token": "synthetic-refresh-token",
            "token_type": "Bearer",
            "scope": "api:read api:write offline_access",
            "expires_in": 600
        }),
    )
    .await;
    let (mut stream, _) = listener.accept().await.unwrap();
    let (headers, _) = request(&mut stream).await;
    assert!(headers.starts_with("GET /space HTTP/1.1"));
    assert!(headers.contains("authorization: Bearer synthetic-access-token"));
    assert!(headers
        .lines()
        .any(|line| line.eq_ignore_ascii_case("x-capacities-api-version: 1.0.0")));
    response(&mut stream, json!({"id": space})).await;
}

async fn fixture_provider(space: Option<Uuid>) -> (Registration, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let registration = Registration::loopback(&origin);
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), provider(listener, origin, space))
            .await
            .unwrap();
    });
    (registration, task)
}

#[tokio::test]
async fn exchange_holds_original_owner_and_publishes_verified_space_with_local_config() {
    let fixture = Fixture::new(true);
    let space = Uuid::new_v4();
    let (registration, server) = fixture_provider(Some(space)).await;
    let report = fixture.act("exchange", Some(&registration)).await;
    server.await.unwrap();
    assert_eq!(report.status, Outcome::Committed);
    assert_eq!(report.space_id, Some(space));
    assert!(matches!(
        GrantOwner::open(&fixture.paths.credentials, fixture.binding.grant_id),
        Err(Failure::Conflict)
    ));
    let mut frame = Vec::new();
    report.write_to(&mut frame).unwrap();
    let wire = String::from_utf8(frame).unwrap();
    for secret in ["synthetic", "verifier", "access_token", "refresh_token"] {
        assert!(!wire.contains(secret));
    }
    let receipt: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(receipt.as_object().unwrap().len(), 10);
    assert_eq!(receipt["space_id"], json!(space));
    let installed = fs::read(&fixture.path).unwrap();
    assert_eq!(
        receipt["installed_config_sha256"],
        json!(digest(&installed))
    );
    let config: Value = serde_json::from_slice(&installed).unwrap();
    assert_eq!(
        config["capacities_export"]["grant_id"],
        json!(fixture.binding.grant_id)
    );
    assert_eq!(
        config["product_settings"],
        json!({"writer_enabled": false, "export_destination": "mem"})
    );
    assert_eq!(fixture.act("status", None).await.status, Outcome::Committed);
    let other = Binding {
        operation_id: Uuid::new_v4(),
        ..fixture.binding.clone()
    };
    assert_eq!(
        fixture.act_as("resume", None, &other).await.status,
        Outcome::Unresolved
    );
}

#[tokio::test]
async fn lost_token_response_keeps_original_pending_and_refuses_replay_and_resume() {
    let fixture = Fixture::new(true);
    let (registration, server) = fixture_provider(None).await;
    assert_eq!(
        fixture.act("exchange", Some(&registration)).await.status,
        Outcome::Unresolved
    );
    server.await.unwrap();
    assert_eq!(
        fixture.act("exchange", Some(&registration)).await.status,
        Outcome::Unresolved
    );
    assert_eq!(
        fixture.act("status", None).await.status,
        Outcome::Unresolved
    );
    assert_eq!(
        fixture.act("resume", None).await.status,
        Outcome::Unresolved
    );
    assert_eq!(
        GrantOwner::open_existing(&fixture.paths.credentials, fixture.binding.grant_id)
            .unwrap()
            .unwrap()
            .status()
            .unwrap()
            .phase,
        Phase::PendingExchange
    );
    assert!(config_matches(
        &fixture.path,
        &fixture.binding.config_sha256
    ));
}

#[tokio::test]
async fn visible_stage_sync_failure_retains_owner_and_can_only_resume_original_config() {
    let fixture = Fixture::new(true);
    let id = fixture.binding.grant_id;
    let space = Uuid::new_v4();
    let doc = Document::load(Some(fixture.path.clone())).unwrap();
    let target = doc.target(id, space).unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let grant = serde_json::to_vec(&grant_fixture(id, space, expires)).unwrap();
    let mut permit = GrantOwner::open(&fixture.paths.credentials, id)
        .unwrap()
        .begin_exchange(
            fixture
                .binding
                .enrollment("fixture-public-client".to_owned()),
        )
        .unwrap();
    permit.authorize_dispatch().unwrap();
    files::fail_next_atomic(fixture.grant_path("json"), files::AtomicFailure::ParentSync);
    assert_eq!(
        permit.stage_in_place(space, &grant, digest(&target)),
        Err(Failure::Storage)
    );
    assert!(matches!(
        GrantOwner::open(&fixture.paths.credentials, id),
        Err(Failure::Conflict)
    ));
    assert!(config_matches(
        &fixture.path,
        &fixture.binding.config_sha256
    ));
    drop(permit);

    let registration = Registration::loopback("http://127.0.0.1:1");
    let mut other_client = Registration::loopback("http://127.0.0.1:1");
    other_client.client_id = "another-public-client".to_owned();
    assert_eq!(fixture.act("status", None).await.status, Outcome::Staged);
    assert_eq!(fixture.act("resume", None).await.status, Outcome::Staged);
    assert_eq!(
        fixture.act("resume", Some(&other_client)).await.status,
        Outcome::Staged
    );
    assert!(config_matches(
        &fixture.path,
        &fixture.binding.config_sha256
    ));
    let original = fs::read(&fixture.path).unwrap();
    let mut different = original.clone();
    different.push(b' ');
    fs::write(&fixture.path, &different).unwrap();
    assert_eq!(
        fixture.act("resume", Some(&registration)).await.status,
        Outcome::Unresolved
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), different);
    fs::write(&fixture.path, &original).unwrap();
    assert_eq!(
        fixture.act("resume", Some(&registration)).await.status,
        Outcome::Committed
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), target);
}

#[tokio::test]
async fn expired_stage_cannot_replace_the_current_configuration() {
    let fixture = Fixture::new(true);
    let id = fixture.binding.grant_id;
    let space = Uuid::new_v4();
    let doc = Document::load(Some(fixture.path.clone())).unwrap();
    let target = doc.target(id, space).unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let grant = serde_json::to_vec(&grant_fixture(id, space, expires)).unwrap();
    let mut permit = GrantOwner::open(&fixture.paths.credentials, id)
        .unwrap()
        .begin_exchange(
            fixture
                .binding
                .enrollment("fixture-public-client".to_owned()),
        )
        .unwrap();
    permit.authorize_dispatch().unwrap();
    permit
        .stage_in_place(space, &grant, digest(&target))
        .unwrap();
    drop(permit);
    let record_path = fixture.grant_path("json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    let expiry = record.pointer_mut("/state/grant/expires_at").unwrap();
    *expiry = json!(1);
    files::atomic(&record_path, &serde_json::to_vec(&record).unwrap()).unwrap();
    let approval = Registration::loopback("http://127.0.0.1:1");
    assert_eq!(
        fixture.act("resume", Some(&approval)).await.status,
        Outcome::Unresolved
    );
    assert!(config_matches(
        &fixture.path,
        &fixture.binding.config_sha256
    ));
}

#[test]
fn pending_fence_publication_error_retains_grant_owner() {
    let fixture = Fixture::new(true);
    let owner = GrantOwner::open(&fixture.paths.credentials, fixture.binding.grant_id).unwrap();
    files::fail_next_atomic(fixture.grant_path("json"), files::AtomicFailure::ParentSync);
    let Err((owner, failure)) = owner.begin_exchange_retained(
        fixture
            .binding
            .enrollment("fixture-public-client".to_owned()),
    ) else {
        panic!("injected fence failure should refuse dispatch");
    };
    assert_eq!(failure, Failure::Storage);
    assert!(matches!(
        GrantOwner::open(&fixture.paths.credentials, fixture.binding.grant_id),
        Err(Failure::Conflict)
    ));
    assert_eq!(owner.status().unwrap().phase, Phase::PendingExchange);
}
