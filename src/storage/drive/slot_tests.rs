use super::*;
use crate::storage::drive::slot::{PointerMatch, SlotBinding, SlotPointer};

fn binding() -> SlotBinding {
    SlotBinding {
        group: Uuid::from_u128(1),
        scope_sha256: digest(b"source-document"),
        slot_id: "agreed-slot".into(),
    }
}
fn pointer(name: &str, bytes: &[u8]) -> SlotPointer {
    SlotPointer {
        binding: binding(),
        descriptor_id: name.into(),
        descriptor_sha256: digest(bytes),
    }
}
fn slot_metadata(p: &SlotPointer) -> Value {
    json!({"id": p.binding.slot_id, "trashed": false, "appProperties": {
        "buddy_protocol":"1", "kind":"commit-slot", "collection":p.binding.group,
        "descriptor_id":p.descriptor_id, "sha256":p.descriptor_sha256,
        "scope_sha256":p.binding.scope_sha256
    }})
}
fn raw(bytes: &[u8]) -> Reply {
    Reply {
        status: 200,
        headers: vec![],
        body: bytes.to_vec(),
    }
}
fn staged(name: &str, bytes: &[u8]) -> Vec<Reply> {
    vec![
        reply(200, json!({"user":{"permissionId":"fixture-account"}})),
        reply(200, metadata(name, &tag(bytes))),
        raw(bytes),
    ]
}
fn attempt(
    status: u16,
    candidate: &[u8],
    observed: &SlotPointer,
    observed_bytes: &[u8],
) -> Vec<Reply> {
    let mut r = staged("candidate", candidate);
    r.extend([
        reply(status, json!({})),
        reply(200, slot_metadata(observed)),
        reply(200, metadata(&observed.descriptor_id, &tag(observed_bytes))),
        raw(observed_bytes),
    ]);
    r
}
fn prepare(f: &mut Fixture, bytes: &[u8]) -> slot::PreparedSlot {
    f.client
        .prepare_slot(
            binding(),
            "candidate".into(),
            ObjectRef {
                sha256: digest(bytes),
                bytes: bytes.len() as u64,
            },
        )
        .unwrap()
}

#[test]
fn metadata_create_ack_conflict_and_lost_response_read_the_exact_slot() {
    let bytes = br#"{"operation":"a","base":"accepted"}"#;
    for status in [200, 201, 409, 503, 0] {
        let p = pointer("candidate", bytes);
        let mut f = Fixture::new(|_| {
            let mut r = staged("candidate", bytes);
            r.extend(attempt(status, bytes, &p, bytes));
            r
        });
        let prepared = prepare(&mut f, bytes);
        let result = f.client.create_or_read_slot(&prepared).unwrap();
        assert_eq!(result.candidate_match, PointerMatch::Same);
        assert_eq!(result.descriptor, bytes);
        let requests = f.finish();
        let posts: Vec<_> = requests.iter().filter(|r| r.starts_with("POST ")).collect();
        assert_eq!(posts.len(), 1);
        assert!(posts[0].starts_with("POST /drive/v3/files?fields=id "));
        assert!(!posts[0].contains("uploadType") && !posts[0].contains("multipart"));
        let body: Value = serde_json::from_str(posts[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["id"], "agreed-slot");
        assert_eq!(body["parents"], json!(["appDataFolder"]));
        assert_eq!(body["appProperties"].as_object().unwrap().len(), 6);
        assert!(requests
            .iter()
            .any(|r| r.starts_with("GET /drive/v3/files/agreed-slot?fields=")));
        assert!(requests.iter().all(|r| !r.contains("generateIds")
            && !r.starts_with("PATCH ")
            && !r.starts_with("DELETE ")));
    }
}

#[test]
fn different_pointer_and_replayed_stale_candidate_cannot_choose_a_successor() {
    let candidate = br#"{"operation":"loser","base":"same"}"#;
    let winner = br#"{"operation":"winner","base":"same","next":"successor"}"#;
    let p = pointer("winner", winner);
    let mut f = Fixture::new(|_| {
        let mut r = staged("candidate", candidate);
        r.extend(attempt(409, candidate, &p, winner));
        r.extend(attempt(409, candidate, &p, winner));
        r
    });
    let prepared = prepare(&mut f, candidate);
    for _ in 0..2 {
        let result = f.client.create_or_read_slot(&prepared).unwrap();
        assert_eq!(result.candidate_match, PointerMatch::Different);
        assert_eq!(result.descriptor, winner);
    }
    for request in f.finish().iter().filter(|r| r.starts_with("POST ")) {
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["id"], "agreed-slot");
        assert!(!request.contains("successor"));
    }
}

#[test]
fn invalid_private_pointer_or_missing_slot_is_not_a_free_slot() {
    let bytes = b"opaque-descriptor";
    let p = pointer("candidate", bytes);
    let mut invalid = slot_metadata(&p);
    invalid["appProperties"]["scope_sha256"] = json!(digest(b"another-document"));
    let mut incomplete = slot_metadata(&p);
    incomplete["appProperties"]
        .as_object_mut()
        .unwrap()
        .remove("descriptor_id");
    let mut wrong_id = slot_metadata(&p);
    wrong_id["id"] = json!("another-slot");
    let mut removed = slot_metadata(&p);
    removed["trashed"] = json!(true);
    let mut unsupported = slot_metadata(&p);
    unsupported["appProperties"]["buddy_protocol"] = json!("99");
    for response in [
        reply(200, invalid),
        reply(200, incomplete),
        reply(200, wrong_id),
        reply(200, removed),
        reply(200, unsupported),
        reply(404, json!({})),
    ] {
        let mut f = Fixture::new(|_| {
            let mut r = staged("candidate", bytes);
            r.extend(staged("candidate", bytes));
            r.extend([reply(409, json!({})), response]);
            r
        });
        let prepared = prepare(&mut f, bytes);
        let error = f.client.create_or_read_slot(&prepared).unwrap_err();
        assert!(!format!("{error:?}").contains("fixture-private-token"));
        assert_eq!(
            f.finish().iter().filter(|r| r.starts_with("POST ")).count(),
            1
        );
    }
}

#[test]
fn descriptor_integrity_is_rechecked_before_create_and_after_observation() {
    let candidate = b"candidate-bytes";
    let observed_bytes = b"observed-bytes";
    let p = pointer("observed", observed_bytes);
    let mut f = Fixture::new(|_| {
        let mut r = staged("candidate", candidate);
        r.extend([
            reply(200, json!({"user":{"permissionId":"fixture-account"}})),
            reply(200, metadata("candidate", &tag(candidate))),
            raw(b"changed-in-storage"),
        ]);
        r
    });
    let prepared = prepare(&mut f, candidate);
    assert!(f.client.create_or_read_slot(&prepared).is_err());
    assert!(f.finish().iter().all(|r| !r.starts_with("POST ")));

    let mut f = Fixture::new(|_| {
        let mut r = staged("candidate", candidate);
        r.extend(staged("candidate", candidate));
        r.extend([
            reply(409, json!({})),
            reply(200, slot_metadata(&p)),
            reply(200, metadata("observed", &tag(observed_bytes))),
            raw(b"corrupt-wire-bytes"),
        ]);
        r
    });
    let prepared = prepare(&mut f, candidate);
    assert!(f.client.create_or_read_slot(&prepared).is_err());
    f.finish();
}

#[test]
fn bounds_and_changed_application_are_refused_before_publication() {
    let mut f = Fixture::new(|_| vec![]);
    let reference = ObjectRef {
        sha256: digest(b"x"),
        bytes: 1,
    };
    assert!(f
        .client
        .prepare_slot(binding(), "x".repeat(112), reference.clone())
        .is_err());
    assert!(f
        .client
        .prepare_slot(binding(), "agreed-slot".into(), reference.clone())
        .is_err());
    assert!(f
        .client
        .prepare_slot(
            binding(),
            "candidate".into(),
            ObjectRef {
                bytes: 65537,
                ..reference
            }
        )
        .is_err());
    assert!(f.finish().is_empty());

    let bytes = b"opaque";
    let mut f = Fixture::new(|_| staged("candidate", bytes));
    let prepared = prepare(&mut f, bytes);
    f.client.credentials.client_id = "different-oauth-registration".into();
    assert!(f.client.create_or_read_slot(&prepared).is_err());
    assert!(f.finish().iter().all(|r| !r.starts_with("POST ")));
}

/// Two real adapter clients overlap their POSTs against a modeled exclusive
/// provider. The barrier establishes overlap; time is not the winner rule.
/// This is host wire evidence, not evidence of Google's live atomicity.
#[test]
fn overlapping_clients_observe_one_modeled_provider_pointer() {
    use std::sync::Condvar;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let api = format!("http://{}/drive/v3", listener.local_addr().unwrap());
    let posts = Arc::new((Mutex::new(0_u32), Condvar::new()));
    let selected: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let state = selected.clone();
    let server = thread::spawn(move || {
        let mut workers = Vec::new();
        // Each client: three preparation requests + seven create/read requests.
        for _ in 0..20 {
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
            let state = state.clone();
            let posts = posts.clone();
            workers.push(thread::spawn(move || {
                // Accepted sockets can inherit the listener's nonblocking mode on Windows.
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let header_end = loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") { break i + 4; }
                    assert!(bytes.len() < 8192);
                };
                let header = String::from_utf8_lossy(&bytes[..header_end]);
                let line = header.lines().next().unwrap().to_owned();
                let length = header.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                assert!(length <= 8192);
                while bytes.len() < header_end + length {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let response = if line.starts_with("POST ") {
                    let request: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    assert_eq!(request["id"], "agreed-slot");
                    let (arrivals, ready) = &*posts;
                    let mut arrived = arrivals.lock().unwrap();
                    *arrived += 1;
                    ready.notify_all();
                    let (arrived, _) = ready
                        .wait_timeout_while(arrived, Duration::from_secs(5), |n| *n < 2)
                        .unwrap();
                    assert_eq!(*arrived, 2, "both contenders must arrive before arbitration");
                    drop(arrived);
                    let mut selected = state.lock().unwrap();
                    if selected.is_some() { reply(409, json!({})) } else {
                        *selected = Some(json!({"id":"agreed-slot","trashed":false,"appProperties":request["appProperties"]}));
                        reply(200, json!({"id":"agreed-slot"}))
                    }
                } else if line.contains("/about?") {
                    reply(200, json!({"user":{"permissionId":"fixture-account"}}))
                } else if line.contains("/files/agreed-slot?") {
                    reply(200, state.lock().unwrap().clone().unwrap())
                } else {
                    let (name, content): (&str, &[u8]) = if line.contains("/files/candidate-a?") {
                        ("candidate-a", b"opaque-a")
                    } else {
                        assert!(line.contains("/files/candidate-b?"));
                        ("candidate-b", b"opaque-b")
                    };
                    if line.contains("alt=media") { raw(content) } else { reply(200, metadata(name, &tag(content))) }
                };
                write!(stream, "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.status, response.body.len()).unwrap();
                stream.write_all(&response.body).unwrap();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let mut clients = Vec::new();
    for (name, content) in [("candidate-a", b"opaque-a"), ("candidate-b", b"opaque-b")] {
        let mut f = Fixture::new(|_| vec![]);
        f.finish();
        f.client.api = api.clone();
        let prepared = f
            .client
            .prepare_slot(
                binding(),
                name.into(),
                ObjectRef {
                    sha256: digest(content),
                    bytes: content.len() as u64,
                },
            )
            .unwrap();
        clients.push(thread::spawn(move || {
            f.client.create_or_read_slot(&prepared).unwrap()
        }));
    }
    let a = clients.remove(0).join().unwrap();
    let b = clients.remove(0).join().unwrap();
    server.join().unwrap();
    assert_eq!(a.pointer, b.pointer);
    assert_eq!(a.descriptor, b.descriptor);
    assert_ne!(a.candidate_match, b.candidate_match);
    assert!(selected.lock().unwrap().is_some());
}
