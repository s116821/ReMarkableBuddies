use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
fn reply(status: u16, value: Value) -> Reply {
    Reply {
        status,
        headers: vec![],
        body: serde_json::to_vec(&value).unwrap(),
    }
}
struct Fixture {
    root: PathBuf,
    client: GoogleDrive,
    requests: Arc<Mutex<Vec<String>>>,
    server: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(build: impl FnOnce(&str) -> Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let replies = build(&base);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let server = thread::spawn(move || {
            for reply in replies {
                let until = std::time::Instant::now() + Duration::from_secs(15);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(s) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < until,
                                "fixture request deadline"
                            );
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(e) => panic!("fixture accept: {e}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let header_end = loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    assert!(bytes.len() < 64 * 1024);
                };
                let header = String::from_utf8_lossy(&bytes[..header_end]);
                let length = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&bytes).into_owned());
                write!(
                    stream,
                    "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                )
                .unwrap();
                for (key, value) in reply.headers {
                    write!(stream, "{key}: {value}\r\n").unwrap();
                }
                stream.write_all(b"\r\n").unwrap();
                stream.write_all(&reply.body).unwrap();
            }
        });
        let root = std::env::temp_dir().join(format!("buddy-drive-wire-{}", Uuid::new_v4()));
        files::directory(&root).unwrap();
        let credentials = Credentials {
            credential_version: 1,
            client_id: "fixture-client".into(),
            client_secret: None,
            access_token: "fixture-private-token".into(),
            refresh_token: "fixture-refresh".into(),
            expires_at: now() + 3600,
            scopes: vec![APP_SCOPE.into()],
            account_permission_id: "fixture-account".into(),
            collection: Uuid::from_u128(1),
        };
        let credential_path = root.join("drive.json");
        files::atomic_json(&credential_path, &credentials).unwrap();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .new_agent();
        let client = GoogleDrive {
            agent,
            credential_path,
            credentials,
            api: format!("{base}/drive/v3"),
            upload_api: format!("{base}/upload/drive/v3"),
            token_url: format!("{base}/token"),
        };
        Self {
            root,
            client,
            requests,
            server: Some(server),
        }
    }
    fn finish(&mut self) -> Vec<String> {
        self.server.take().unwrap().join().unwrap();
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn tag(bytes: &[u8]) -> Tag {
    Tag {
        collection: Uuid::from_u128(1),
        kind: "object".into(),
        sha256: digest(bytes),
    }
}
fn metadata(id: &str, tag: &Tag) -> Value {
    json!({"id":id,"appProperties":tag.properties(),"trashed":false})
}

#[test]
fn real_wire_pagination_binding_generated_id_and_verified_409() {
    let bytes = b"synthetic ordinary record".to_vec();
    let tag = tag(&bytes);
    let mut f = Fixture::new(|_| {
        vec![
            reply(200, json!({"user":{"permissionId":"fixture-account"}})),
            reply(200, json!({"startPageToken":"start"})),
            reply(200, json!({"files":[],"nextPageToken":"page2"})),
            reply(200, json!({"files":[]})),
            reply(200, json!({"changes":[],"newStartPageToken":"next"})),
            reply(200, json!({"ids":["allocated"]})),
            reply(409, json!({"error":"already exists"})),
            reply(200, metadata("allocated", &tag)),
            Reply {
                status: 200,
                headers: vec![],
                body: bytes.clone(),
            },
        ]
    });
    f.client.authorize(Uuid::from_u128(1)).unwrap();
    assert_eq!(f.client.start_token().unwrap(), "start");
    assert_eq!(f.client.list(None).unwrap().next.as_deref(), Some("page2"));
    assert!(f.client.list(Some("page2")).unwrap().next.is_none());
    assert_eq!(
        f.client.changes("start").unwrap().checkpoint.as_deref(),
        Some("next")
    );
    let id = f.client.allocate().unwrap();
    let mut source = files::Source::Memory(bytes);
    assert!(matches!(
        f.client
            .upload(&id, &tag, &mut source, &mut UploadSession::default())
            .unwrap(),
        UploadProgress::Complete
    ));
    let requests = f.finish();
    assert!(requests[2].contains("spaces=appDataFolder"));
    assert!(requests[3].contains("pageToken=page2"));
    assert!(requests[4].contains("includeRemoved=true"));
    assert!(requests[5].contains("space=appDataFolder"));
    assert!(requests[6].contains("multipart/related"));
    assert!(requests[6].contains("\"id\":\"allocated\""));
    assert!(requests[8].contains("alt=media"));
}

#[test]
fn real_resumable_offsets_expiration_and_verified_completion_stream() {
    let bytes = vec![42_u8; CHUNK + 12345];
    let tag = tag(&bytes);
    let mut f = Fixture::new(|base| {
        vec![
            Reply {
                status: 200,
                headers: vec![(
                    "Location".into(),
                    format!("{base}/upload/drive/v3/session1"),
                )],
                body: vec![],
            },
            reply(404, json!({"error":"expired"})),
            Reply {
                status: 200,
                headers: vec![(
                    "Location".into(),
                    format!("{base}/upload/drive/v3/session2"),
                )],
                body: vec![],
            },
            Reply {
                status: 308,
                headers: vec![],
                body: vec![],
            },
            Reply {
                status: 308,
                headers: vec![("Range".into(), format!("bytes=0-{}", CHUNK - 1))],
                body: vec![],
            },
            Reply {
                status: 308,
                headers: vec![("Range".into(), format!("bytes=0-{}", CHUNK - 1))],
                body: vec![],
            },
            reply(201, json!({"id":"allocated"})),
            reply(200, metadata("allocated", &tag)),
            Reply {
                status: 200,
                headers: vec![],
                body: bytes.clone(),
            },
        ]
    });
    let path = f.root.join("media");
    let reference = ObjectRef {
        sha256: tag.sha256.clone(),
        bytes: bytes.len() as u64,
    };
    files::copy_verified(&path, bytes.as_slice(), &reference).unwrap();
    let mut source = files::Source::open(&path, &reference).unwrap();
    let mut session = UploadSession::default();
    assert!(matches!(
        f.client
            .upload("allocated", &tag, &mut source, &mut session)
            .unwrap(),
        UploadProgress::Pending
    ));
    assert!(matches!(
        f.client
            .upload("allocated", &tag, &mut source, &mut session)
            .unwrap(),
        UploadProgress::Expired
    ));
    assert!(session.uri.is_none());
    assert!(matches!(
        f.client
            .upload("allocated", &tag, &mut source, &mut session)
            .unwrap(),
        UploadProgress::Pending
    ));
    assert!(matches!(
        f.client
            .upload("allocated", &tag, &mut source, &mut session)
            .unwrap(),
        UploadProgress::Pending
    ));
    assert!(matches!(
        f.client
            .upload("allocated", &tag, &mut source, &mut session)
            .unwrap(),
        UploadProgress::Complete
    ));
    let requests = f.finish();
    assert!(requests[0].contains("uploadType=resumable"));
    assert!(requests[1].contains(&format!("bytes */{}", bytes.len())));
    assert!(requests[4].contains(&format!("bytes 0-{}/{}", CHUNK - 1, bytes.len())));
    assert!(requests[6].contains(&format!(
        "bytes {}-{}/{}",
        CHUNK,
        bytes.len() - 1,
        bytes.len()
    )));
}

#[test]
fn token_refresh_scope_binding_and_quota_error_are_sanitized() {
    let mut f = Fixture::new(|_| {
        vec![
            reply(
                200,
                json!({"access_token":"new-fixture-secret","expires_in":3600,"scope":APP_SCOPE}),
            ),
            reply(200, json!({"user":{"permissionId":"fixture-account"}})),
            Reply {
                status: 503,
                headers: vec![("Retry-After".into(), "17".into())],
                body: b"secret-response-body".to_vec(),
            },
        ]
    });
    f.client.credentials.expires_at = 0;
    f.client.authorize(Uuid::from_u128(1)).unwrap();
    let error = f.client.start_token().unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<TransportFailure>()
            .unwrap()
            .retry_after_seconds,
        Some(17)
    );
    assert!(!format!("{error:?}").contains("secret"));
    let saved: Credentials = parse(&read_secret_file(&f.client.credential_path).unwrap()).unwrap();
    assert_eq!(saved.access_token, "new-fixture-secret");
    let requests = f.finish();
    assert!(requests[0].contains("grant_type=refresh_token"));
    assert!(requests[1].contains("Bearer new-fixture-secret"));
}

#[test]
fn wrong_409_content_identity_and_untrusted_session_locations_fail_closed() {
    let bytes = b"fixture".to_vec();
    let tag = tag(&bytes);
    let mut wrong = tag.clone();
    wrong.collection = Uuid::new_v4();
    let mut f = Fixture::new(|_| {
        vec![
            reply(409, json!({})),
            reply(200, metadata("allocated", &wrong)),
        ]
    });
    let error = f
        .client
        .upload(
            "allocated",
            &tag,
            &mut files::Source::Memory(bytes),
            &mut UploadSession::default(),
        )
        .err()
        .unwrap();
    assert!(!format!("{error:?}").contains("fixture-private-token"));
    f.finish();
    assert!(f
        .client
        .safe_session("https://example.invalid/upload/drive/v3/private")
        .is_err());
}
