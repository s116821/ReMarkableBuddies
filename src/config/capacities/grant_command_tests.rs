use super::*;
use crate::config::capacities::access_token;

struct Enrolled {
    fixture: Fixture,
    space: Uuid,
    config: Vec<u8>,
}

impl Enrolled {
    fn new() -> Self {
        let mut fixture = Fixture::new(true);
        let space = Uuid::new_v4();
        let grant_id = fixture.binding.grant_id;
        let doc = Document::load(Some(fixture.path.clone())).unwrap();
        let config = doc.target(grant_id, space).unwrap();
        let expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600;
        let grant = serde_json::to_vec(&grant_fixture(grant_id, space, expiry)).unwrap();
        let mut permit = GrantOwner::open(&fixture.paths.credentials, grant_id)
            .unwrap()
            .begin_exchange(
                fixture
                    .binding
                    .enrollment("fixture-public-client".to_owned()),
            )
            .unwrap();
        permit.authorize_dispatch().unwrap();
        let config_sha256 = digest(&config);
        let owner = permit.stage(space, &grant, config_sha256.clone()).unwrap();
        fs::write(&fixture.path, &config).unwrap();
        owner
            .activate_after_config_readback(fixture.binding.operation_id, &config_sha256)
            .unwrap();
        fixture.binding.config_sha256 = config_sha256;
        Self {
            fixture,
            space,
            config,
        }
    }

    async fn act(
        &self,
        selector: &str,
        operation: Uuid,
        expected: Option<&str>,
        registration: Option<&Registration>,
    ) -> grant_command::Report {
        let binding = Binding {
            operation_id: operation,
            ..self.fixture.binding.clone()
        };
        let mut input = json!({"contract_version":1,"selector":selector,"budget_ms":30000,
            "binding":binding,"space_id":self.space});
        if let Some(expected) = expected {
            input["expected_grant_sha256"] = json!(expected);
        }
        let input = grant_command::Request::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
        let (_sender, canceled) = watch::channel(false);
        grant_command::handle(
            Some(self.fixture.path.clone()),
            input,
            Instant::now(),
            canceled,
            registration,
            Some(self.fixture.binding.boot_id),
        )
        .await
    }

    async fn status(&self) -> Value {
        serde_json::to_value(self.act("grant-status", Uuid::new_v4(), None, None).await).unwrap()
    }
}

enum Provider {
    Refresh {
        verified_space: Option<Uuid>,
        lost_install_ack: Option<PathBuf>,
    },
    Revoke {
        empty: bool,
    },
}

async fn maintenance_provider(action: Provider) -> (Registration, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let registration = Registration::loopback(&origin);
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(request(&mut stream).await.0.starts_with("GET /.well-known/oauth-authorization-server HTTP/1.1"));
            response(&mut stream, json!({
                "issuer":origin,
                "authorization_endpoint":format!("{origin}/oauth/authorize"),
                "token_endpoint":format!("{origin}/oauth/token"),
                "revocation_endpoint":format!("{origin}/oauth/revoke"),
                "code_challenge_methods_supported":["S256"],
                "grant_types_supported":["authorization_code","refresh_token"],
                "response_types_supported":["code"],
                "token_endpoint_auth_methods_supported":["none"],
                "scopes_supported":["api:read","api:write","offline_access"]
            })).await;
            let (mut stream, _) = listener.accept().await.unwrap();
            let (headers, body) = request(&mut stream).await;
            assert!(!headers.to_ascii_lowercase().contains("authorization:"));
            assert!(body.contains("client_id=fixture-public-client"));
            match action {
                Provider::Refresh {verified_space,lost_install_ack} => {
                    assert!(headers.starts_with("POST /oauth/token HTTP/1.1"));
                    assert!(body.contains("grant_type=refresh_token"));
                    assert!(body.contains("refresh_token=synthetic-refresh-token"));
                    if let Some(space) = verified_space {
                        response(&mut stream,json!({
                            "token_type":"Bearer","access_token":"replacement-access",
                            "refresh_token":"replacement-refresh","expires_in":600
                        })).await;
                        let (mut stream, _) = listener.accept().await.unwrap();
                        let (headers, _) = request(&mut stream).await;
                        assert!(headers.starts_with("GET /space HTTP/1.1"));
                        assert!(headers.contains("authorization: Bearer replacement-access"));
                        if let Some(path) = lost_install_ack {
                            files::fail_next_atomic(path, files::AtomicFailure::ParentSync);
                        }
                        response(&mut stream,json!({"id":space})).await;
                    }
                }
                Provider::Revoke {empty} => {
                    assert!(headers.starts_with("POST /oauth/revoke HTTP/1.1"));
                    assert!(body.contains("token_type_hint=refresh_token"));
                    assert!(body.contains("token=synthetic-refresh-token"));
                    if empty {
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        stream.shutdown().await.unwrap();
                    } else {
                        response(&mut stream,json!({"unexpected":"data"})).await;
                    }
                }
            }
        }).await.unwrap();
    });
    (registration, task)
}

#[tokio::test]
async fn rotation_publishes_replacement_under_owner_then_existing_export_uses_it() {
    let f = Enrolled::new();
    let before = f.status().await;
    let revision = before["grant_sha256"].as_str().unwrap();
    let original = Uuid::new_v4();
    let (registration, server) = maintenance_provider(Provider::Refresh {
        verified_space: Some(f.space),
        lost_install_ack: None,
    })
    .await;
    let result = f
        .act("refresh", original, Some(revision), Some(&registration))
        .await;
    server.await.unwrap();
    let root = &f.fixture.paths.credentials;
    let id = f.fixture.binding.grant_id;
    assert!(matches!(
        access_token(root, f.space, id, true),
        Err(Failure::Conflict)
    ));
    let receipt = serde_json::to_value(result).unwrap();
    assert_eq!(receipt["status"], "refreshed");
    assert_eq!(receipt["phase"], "active");
    assert_eq!(receipt["observed_operation_id"], json!(original));
    assert_ne!(receipt["grant_sha256"], before["grant_sha256"]);
    assert!(!receipt.to_string().contains("replacement-"));
    assert_eq!(
        access_token(root, f.space, id, true).unwrap().expose(),
        "replacement-access"
    );
    assert_eq!(f.status().await["observed_operation_id"], json!(original));
    assert_eq!(
        serde_json::to_value(
            f.act("refresh", original, Some(revision), Some(&registration))
                .await
        )
        .unwrap()["status"],
        "conflict"
    );
    assert_eq!(fs::read(&f.fixture.path).unwrap(), f.config);
}

#[tokio::test]
async fn lost_rotation_cannot_reopen_for_export_retry_or_revoke_the_old_token() {
    let f = Enrolled::new();
    let before = f.status().await;
    let revision = before["grant_sha256"].as_str().unwrap();
    let operation = Uuid::new_v4();
    let (registration, server) = maintenance_provider(Provider::Refresh {
        verified_space: None,
        lost_install_ack: None,
    })
    .await;
    let first = f
        .act("refresh", operation, Some(revision), Some(&registration))
        .await;
    server.await.unwrap();
    assert_eq!(serde_json::to_value(first).unwrap()["status"], "unresolved");
    let pending = f.status().await;
    assert_eq!(pending["phase"], "pending-rotation");
    assert_eq!(pending["observed_operation_id"], json!(operation));
    let current = pending["grant_sha256"].as_str().unwrap();
    assert_eq!(
        serde_json::to_value(
            f.act(
                "refresh",
                Uuid::new_v4(),
                Some(current),
                Some(&registration)
            )
            .await
        )
        .unwrap()["status"],
        "refused"
    );
    let root = &f.fixture.paths.credentials;
    let id = f.fixture.binding.grant_id;
    assert!(matches!(
        access_token(root, f.space, id, true),
        Err(Failure::Authentication)
    ));
    let disabled = serde_json::to_value(
        f.act(
            "disconnect",
            Uuid::new_v4(),
            Some(current),
            Some(&registration),
        )
        .await,
    )
    .unwrap();
    assert_eq!(disabled["status"], "local-disabled");
    assert_eq!(disabled["phase"], "local-disabled");
    assert!(!fs::read_to_string(f.fixture.grant_path("json"))
        .unwrap()
        .contains("synthetic-refresh"));
    assert_eq!(fs::read(&f.fixture.path).unwrap(), f.config);
}

#[tokio::test]
async fn lost_replacement_publish_ack_reconciles_exact_rotation_without_retry() {
    let f = Enrolled::new();
    let before = f.status().await;
    let revision = before["grant_sha256"].as_str().unwrap();
    let operation = Uuid::new_v4();
    let (registration, server) = maintenance_provider(Provider::Refresh {
        verified_space: Some(f.space),
        lost_install_ack: Some(f.fixture.grant_path("json")),
    })
    .await;
    let result = f
        .act("refresh", operation, Some(revision), Some(&registration))
        .await;
    server.await.unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["status"],
        "unresolved"
    );

    let observed = f.status().await;
    assert_eq!(observed["phase"], "active");
    assert_eq!(observed["observed_operation_id"], json!(operation));
    assert_ne!(observed["grant_sha256"], before["grant_sha256"]);
    let stale = f
        .act("refresh", operation, Some(revision), Some(&registration))
        .await;
    assert_eq!(serde_json::to_value(stale).unwrap()["status"], "conflict");
}

#[tokio::test]
async fn disconnect_reports_provider_confirmation_only_for_an_exact_empty_receipt() {
    for empty in [false, true] {
        let f = Enrolled::new();
        let before = f.status().await;
        let revision = before["grant_sha256"].as_str().unwrap();
        let operation = Uuid::new_v4();
        let (registration, server) = maintenance_provider(Provider::Revoke { empty }).await;
        let result = f
            .act("disconnect", operation, Some(revision), Some(&registration))
            .await;
        server.await.unwrap();
        let receipt = serde_json::to_value(result).unwrap();
        assert_eq!(
            receipt["status"],
            if empty { "revoked" } else { "unresolved" }
        );
        assert_eq!(
            receipt["phase"],
            if empty {
                "revoked"
            } else {
                "revocation-pending"
            }
        );
        assert_eq!(receipt["observed_operation_id"], json!(operation));
        assert!(matches!(
            access_token(
                &f.fixture.paths.credentials,
                f.space,
                f.fixture.binding.grant_id,
                true
            ),
            Err(Failure::Authentication)
        ));
        let current = receipt["grant_sha256"].as_str().unwrap();
        let repeated = f
            .act(
                "disconnect",
                Uuid::new_v4(),
                Some(current),
                Some(&registration),
            )
            .await;
        assert_eq!(serde_json::to_value(repeated).unwrap()["status"], "refused");
        assert_eq!(fs::read(&f.fixture.path).unwrap(), f.config);
    }
}

#[tokio::test]
async fn registration_absence_refuses_refresh_and_allows_local_disable_only() {
    let mut f = Enrolled::new();
    let before = f.status().await;
    let revision = before["grant_sha256"].as_str().unwrap();
    let refused = f.act("refresh", Uuid::new_v4(), Some(revision), None).await;
    assert_eq!(serde_json::to_value(refused).unwrap()["status"], "refused");
    assert_eq!(f.status().await["grant_sha256"], before["grant_sha256"]);

    let disabled = f
        .act("disconnect", Uuid::new_v4(), Some(revision), None)
        .await;
    let disabled = serde_json::to_value(disabled).unwrap();
    assert_eq!(disabled["status"], "local-disabled");
    assert_eq!(disabled["phase"], "local-disabled");
    assert_eq!(fs::read(&f.fixture.path).unwrap(), f.config);

    // Later destination selection cannot redirect an original status read.
    let doc = Document::load(Some(f.fixture.path.clone())).unwrap();
    let replacement = doc.target(Uuid::new_v4(), Uuid::new_v4()).unwrap();
    fs::write(&f.fixture.path, &replacement).unwrap();
    f.fixture.binding.config_sha256 = digest(&replacement);
    assert_eq!(f.status().await["phase"], "local-disabled");
    let retained = disabled["grant_sha256"].as_str().unwrap();
    let refused = f
        .act("disconnect", Uuid::new_v4(), Some(retained), None)
        .await;
    assert_eq!(serde_json::to_value(refused).unwrap()["status"], "conflict");
    assert_eq!(fs::read(&f.fixture.path).unwrap(), replacement);
}

#[test]
fn committed_disable_requires_current_config_and_deadline_after_durable_readback() {
    let f = Enrolled::new();
    let doc = Document::load(Some(f.fixture.path.clone())).unwrap();
    let owner = GrantOwner::open(&f.fixture.paths.credentials, f.fixture.binding.grant_id).unwrap();
    owner.disable_local(f.space).unwrap();
    let (_signal, canceled) = watch::channel(false);
    let fresh = Budget::new(Instant::now() + Duration::from_secs(5), canceled.clone());
    assert_eq!(
        grant_command::disabled_current(&owner, &doc, &fresh)
            .unwrap()
            .phase,
        Phase::LocalDisabled
    );
    let expired = Budget::new(Instant::now(), canceled);
    assert!(grant_command::disabled_current(&owner, &doc, &expired).is_none());

    let mut moved = f.config.clone();
    moved.push(b' ');
    fs::write(&f.fixture.path, moved).unwrap();
    assert!(grant_command::disabled_current(&owner, &doc, &fresh).is_none());
    assert_eq!(owner.status().unwrap().phase, Phase::LocalDisabled);
    assert!(matches!(
        GrantOwner::open(&f.fixture.paths.credentials, f.fixture.binding.grant_id),
        Err(Failure::Conflict)
    ));
}

#[test]
fn configured_cli_refuses_foreign_grant_before_publication() {
    use crate::{
        conversation::{CapacitiesAttempt, Ledger},
        note_export::{command, ExportStatus},
    };

    let f = Enrolled::new();
    let mut config: Value = serde_json::from_slice(&f.config).unwrap();
    let paths = StorePaths {
        data: f.fixture.paths.data.with_file_name("foreign-data"),
        cache: f.fixture.paths.cache.with_file_name("foreign-cache"),
        credentials: f
            .fixture
            .paths
            .credentials
            .with_file_name("foreign-credentials"),
    };
    let conversation = Uuid::new_v4();
    let correlation = Uuid::new_v4();
    let ledger = Ledger::new(Arc::new(Store::open(paths.clone()).unwrap()));
    assert_ne!(
        ledger.store().actor_id.to_string(),
        f.fixture.binding.tablet_id
    );
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    drop(ledger);
    let source = f.fixture.grant_path("json");
    let copied = paths.credentials.join(source.file_name().unwrap());
    fs::copy(&source, &copied).unwrap();
    fs::set_permissions(&copied, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    config["paths"] = json!(paths);
    config["product_settings"] = json!({"writer_enabled":true,"export_destination":"capacities"});
    fs::write(&f.fixture.path, serde_json::to_vec(&config).unwrap()).unwrap();

    let frame = serde_json::to_vec(&json!({
        "command":"export",
        "note":{
            "conversation":conversation,
            "correlation":correlation,
            "effect_operation":Uuid::new_v4(),
            "source_scope":"fixture",
            "source_revision":"1",
            "title":"Fixture",
            "markdown":"Fixture body."
        }
    }))
    .unwrap();
    let report = command::run(Some(f.fixture.path.clone()), &mut frame.as_slice());
    assert_eq!(report.status, ExportStatus::Refused);
    assert_eq!(report.failure, Some(Failure::Authentication));
    let ledger = Ledger::new(Arc::new(Store::open(paths).unwrap()));
    let id = CapacitiesAttempt::association_id(conversation, f.space, correlation);
    assert!(ledger
        .export_association(conversation, id)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn configured_send_time_rotates_expired_held_grant_under_original_config_only() {
    use crate::config::capacities::configured::Access;
    let f = Enrolled::new();
    let (_, saved) = Config::load_with_saved_revision(Some(f.fixture.path.clone())).unwrap();
    let (registration, server) = maintenance_provider(Provider::Refresh {
        verified_space: Some(f.space),
        lost_install_ack: None,
    })
    .await;
    let mut access = Access::open_registered(
        &f.fixture.paths,
        saved,
        f.space,
        f.fixture.binding.grant_id,
        true,
        Some(registration),
    )
    .unwrap();
    // Admission occurs while the credential is fresh. Move its expiry in
    // this fixture so send-time sees the post-generation state without sleep.
    let actor = Uuid::parse_str(&f.fixture.binding.tablet_id).unwrap();
    access.admit(Some(actor), true).unwrap();
    let (_sender, canceled) = watch::channel(false);
    assert!(matches!(
        access
            .current(
                Uuid::new_v4(),
                Instant::now() + Duration::from_secs(10),
                canceled.clone(),
                || Ok(())
            )
            .await,
        Err(Failure::Authentication)
    ));
    let path = f.fixture.grant_path("json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    *record.pointer_mut("/state/grant/expires_at").unwrap() = json!(1);
    files::atomic(&path, &serde_json::to_vec(&record).unwrap()).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut changed = f.config.clone();
    changed.push(b' ');
    fs::write(&f.fixture.path, &changed).unwrap();
    assert!(matches!(
        access
            .current(actor, deadline, canceled.clone(), || Ok(()))
            .await,
        Err(Failure::Conflict)
    ));
    fs::write(&f.fixture.path, &f.config).unwrap();
    assert!(matches!(
        access
            .current(actor, Instant::now(), canceled.clone(), || Ok(()))
            .await,
        Err(Failure::Deadline)
    ));
    let replacement = access
        .current(actor, deadline, canceled.clone(), || Ok(()))
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(replacement.expose(), "replacement-access");
    assert!(matches!(
        GrantOwner::open(&f.fixture.paths.credentials, f.fixture.binding.grant_id),
        Err(Failure::Conflict)
    ));

    // No second token POST for this export. Fresh access now reads the
    // replacement under that same original grant ID and owner.
    assert_eq!(
        access
            .current(actor, deadline, canceled, || Ok(()))
            .await
            .unwrap()
            .expose(),
        "replacement-access"
    );
}
