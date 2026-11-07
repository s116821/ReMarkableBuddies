//! Opt-in evidence hooks for the disposable Drive qualification example.
use super::{slot::*, *};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};

pub const MEDIA_MAX: usize = 4 * 1024;
pub const MULTIPART_MAX: usize = 16 * 1024;

pub struct Resource {
    id: String,
    group: Uuid,
    index: usize,
    shape: Shape,
}

enum Shape {
    Slot {
        scope: String,
        descriptor_id: String,
        digest: String,
        missing_property: bool,
    },
    Descriptor {
        digest: String,
        bytes: Vec<u8>,
    },
}

pub fn scope_digest(run: Uuid, group: Uuid, label: &str) -> String {
    digest(format!("qualification:{run}:{group}:{label}").as_bytes())
}

fn descriptor_bytes(run: Uuid, group: Uuid, label: &str) -> Vec<u8> {
    let base = scope_digest(run, group, "synthetic-base");
    format!("buddy-drive-qualification-v1\n{run}\n{group}\n{base}\n{label}\n").into_bytes()
}

impl Resource {
    /// Construct only a case in the fixed qualification inventory.
    pub fn frozen(
        run: Uuid,
        group: Uuid,
        ids: &[String],
        index: usize,
        contender: bool,
    ) -> Result<Self> {
        const LABELS: [&str; 7] = [
            "race-a",
            "race-b",
            "matrix-browser",
            "matrix-electron",
            "matrix-tablet",
            "negative-expected",
            "unrelated",
        ];
        ensure!(
            !run.is_nil()
                && !group.is_nil()
                && run != group
                && ids.len() == 27
                && (!contender || index < 10),
            "invalid qualification case"
        );
        let shape = match index {
            18..=24 => {
                let mut bytes = descriptor_bytes(run, group, LABELS[index - 18]);
                let declared = digest(&bytes);
                if index == 23 {
                    bytes.extend_from_slice(b"deliberate-bad-body");
                }
                Shape::Descriptor {
                    digest: declared,
                    bytes,
                }
            }
            _ => {
                let (scope, descriptor, missing_property) = match index {
                    0..=9 => ("race", if contender { 19 } else { 18 }, false),
                    10 => ("race", 18, false),
                    11..=13 => ("matrix", index + 9, false),
                    14 => ("negative", 18, true),
                    15 => ("negative", 26, false),
                    16 => ("negative", 23, false),
                    17 => ("unrelated", 24, false),
                    25 => ("negative", 18, false),
                    _ => anyhow::bail!("invalid qualification case"),
                };
                let label = if descriptor == 26 {
                    "never-uploaded"
                } else {
                    LABELS[descriptor - 18]
                };
                Shape::Slot {
                    scope: scope_digest(run, group, scope),
                    descriptor_id: ids[descriptor].clone(),
                    digest: digest(&descriptor_bytes(run, group, label)),
                    missing_property,
                }
            }
        };
        Ok(Self {
            id: ids[index].clone(),
            group,
            index,
            shape,
        })
    }

    pub fn descriptor_reference(&self) -> Result<ObjectRef> {
        let Shape::Descriptor { digest, bytes } = &self.shape else {
            anyhow::bail!("not a qualification descriptor");
        };
        Ok(ObjectRef {
            sha256: digest.clone(),
            bytes: bytes.len() as u64,
        })
    }

    pub fn matches_descriptor_bytes(&self, observed: &[u8]) -> bool {
        matches!(&self.shape, Shape::Descriptor { bytes, .. } if bytes == observed)
    }

    pub fn pointer(&self) -> Result<SlotPointer> {
        let Shape::Slot {
            scope,
            descriptor_id,
            digest,
            ..
        } = &self.shape
        else {
            anyhow::bail!("not a qualification slot");
        };
        Ok(SlotPointer {
            binding: SlotBinding {
                group: self.group,
                scope_sha256: scope.clone(),
                slot_id: self.id.clone(),
            },
            descriptor_id: descriptor_id.clone(),
            descriptor_sha256: digest.clone(),
        })
    }

    fn properties(&self) -> Result<BTreeMap<String, String>> {
        id(&self.id)?;
        match &self.shape {
            Shape::Slot {
                missing_property, ..
            } => {
                let mut properties = self.pointer()?.properties()?;
                if *missing_property {
                    properties.remove("descriptor_id");
                }
                Ok(properties)
            }
            Shape::Descriptor { digest, bytes } => {
                ensure!(
                    !bytes.is_empty() && bytes.len() <= MEDIA_MAX,
                    "descriptor bound"
                );
                let tag = Tag {
                    collection: self.group,
                    kind: "object".into(),
                    sha256: digest.clone(),
                };
                tag.validate()?;
                Ok(tag.properties())
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Exact,
    Missing,
    Mismatch,
}

pub fn set_window(client: &mut GoogleDrive, deadline: Instant, cancel: Arc<AtomicBool>) {
    client.set_request_window(Some(deadline), Some(cancel));
}

pub fn verify_binding(client: &GoogleDrive, expected: &str, group: Uuid) -> Result<()> {
    ensure!(
        client.binding_digest() == expected && client.credentials.collection == group,
        "qualification binding mismatch"
    );
    ensure!(
        client.credentials.scopes.as_slice() == [APP_SCOPE],
        "qualification requires the app-data-only grant"
    );
    Ok(())
}

pub fn same_account(a: &GoogleDrive, b: &GoogleDrive) -> bool {
    a.credentials.account_permission_id == b.credentials.account_permission_id
}

pub fn same_registration(a: &GoogleDrive, b: &GoogleDrive) -> bool {
    a.credentials.client_id == b.credentials.client_id
}

pub fn independent_grants(a: &GoogleDrive, b: &GoogleDrive) -> bool {
    !a.credentials.refresh_token.is_empty()
        && !b.credentials.refresh_token.is_empty()
        && a.credentials.refresh_token != b.credentials.refresh_token
        && a.credentials.access_token != b.credentials.access_token
}

pub fn authorize_refresh(client: &mut GoogleDrive, group: Uuid, stimulate: bool) -> Result<bool> {
    if stimulate {
        // The example admits only dedicated run-owned grant files.
        client.credentials.expires_at = 0;
    }
    let before = client.credentials.expires_at;
    let eligible = before <= now() + 60;
    client.authorize(group)?;
    Ok(eligible && client.credentials.expires_at > before)
}

pub fn create(
    client: &mut GoogleDrive,
    prepared: &PreparedSlot,
    before: impl FnOnce(usize) -> Result<()>,
    after: impl FnOnce(Option<u16>) -> Result<()>,
) -> Result<SlotObservation> {
    client.create_or_read_slot_with(prepared, before, after)
}

/// Only the closed example ledger selects these fixture shapes and IDs.
pub fn stage(
    client: &mut GoogleDrive,
    resource: &Resource,
    reserve: impl FnOnce(usize) -> Result<()>,
) -> Result<Option<u16>> {
    ensure!(
        (14..=16).contains(&resource.index) || (18..=24).contains(&resource.index),
        "not a staging case"
    );
    let properties = resource.properties()?;
    client.authorize(resource.group)?;
    let token = client.token()?;
    match &resource.shape {
        Shape::Slot { .. } => {
            let request = slot_request(&resource.id, properties)?;
            reserve(request.len())?;
            Ok(client
                .send_slot_request(&token, request)?
                .map(|response| response.status().as_u16()))
        }
        Shape::Descriptor { digest, bytes } => {
            let metadata = json!({
                "id": resource.id, "name": digest, "parents": ["appDataFolder"],
                "appProperties": properties,
            });
            let (content_type, body) = GoogleDrive::small_upload_request(&metadata, bytes);
            ensure!(body.len() <= MULTIPART_MAX, "multipart bound");
            reserve(body.len())?;
            Ok(Some(
                client
                    .send_small_upload(&token, &content_type, body)?
                    .status()
                    .as_u16(),
            ))
        }
    }
}

/// Read by exact ID. A 404 is not evidence of ownership.
pub fn presence(client: &mut GoogleDrive, resource: &Resource) -> Result<Presence> {
    let expected = resource.properties()?;
    client.authorize(resource.group)?;
    let metadata = match client.metadata(&resource.id) {
        Ok(metadata) => metadata,
        Err(error)
            if error
                .downcast_ref::<TransportFailure>()
                .is_some_and(|e| e.status == 404) =>
        {
            return Ok(Presence::Missing);
        }
        Err(error) => return Err(error),
    };
    if metadata.trashed || metadata.id != resource.id || metadata.app_properties != expected {
        return Ok(Presence::Mismatch);
    }
    if let Shape::Descriptor { bytes, .. } = &resource.shape {
        if client.download(&resource.id, MEDIA_MAX as u64)? != *bytes {
            return Ok(Presence::Mismatch);
        }
    }
    Ok(Presence::Exact)
}

/// A separately admitted caller verifies the frozen ledger and supplies an exact witness.
pub fn delete_verified(client: &mut GoogleDrive, resource: &Resource) -> Result<bool> {
    ensure!(resource.index < 25, "never-created case cannot be deleted");
    if presence(client, resource)? != Presence::Exact {
        return Ok(false);
    }
    let token = client.token()?;
    let mut response = client
        .bounded(
            client
                .agent
                .delete(format!("{}/files/{}", client.api, resource.id)),
        )?
        .header("Authorization", token)
        .call()
        .map_err(|_| anyhow::anyhow!("qualification delete outcome unknown"))?;
    check(&mut response)?;
    Ok(presence(client, resource)? == Presence::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::drive::shared_fixture::Wire;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    const GROUP: Uuid = Uuid::from_u128(1);
    const RUN: Uuid = Uuid::from_u128(2);

    fn resource(index: usize) -> Resource {
        let ids = (0..27)
            .map(|i| format!("qualification-{i}"))
            .collect::<Vec<_>>();
        Resource::frozen(RUN, GROUP, &ids, index, false).unwrap()
    }

    struct Fixture {
        root: PathBuf,
        wire: Wire,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                root: std::env::temp_dir()
                    .join(format!("buddy-qualification-wire-{}", Uuid::new_v4())),
                wire: Wire::new(),
            }
        }

        fn client(&self, name: &str) -> GoogleDrive {
            self.wire.client(self.root.join(name), GROUP)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for name in ["original.json", "observer.json"] {
                let _ = std::fs::remove_file(self.root.join(name));
            }
            let _ = std::fs::remove_dir(&self.root);
        }
    }

    #[test]
    fn acknowledgement_cut_stops_after_post_and_original_recovers_on_fresh_client() {
        for lose_response in [false, true] {
            let fixture = Fixture::new();
            let mut client = fixture.client("original.json");
            let pointer = resource(10).pointer().unwrap();
            let bytes = descriptor_bytes(RUN, GROUP, "race-a");
            fixture
                .wire
                .object(&pointer.descriptor_id, GROUP, bytes.clone());
            if lose_response {
                fixture.wire.lose_create(&pointer.binding.slot_id);
            }
            let prepared = client
                .prepare_slot(
                    pointer.binding.clone(),
                    pointer.descriptor_id.clone(),
                    resource(18).descriptor_reference().unwrap(),
                )
                .unwrap();
            let mut receipt = None;
            let error = create(
                &mut client,
                &prepared,
                |len| {
                    assert!(len > 0);
                    Ok(())
                },
                |status| {
                    receipt = Some(status);
                    anyhow::bail!("intentional cut")
                },
            )
            .unwrap_err();
            assert_eq!(error.to_string(), "intentional cut");
            assert_eq!(receipt, Some(if lose_response { None } else { Some(201) }));
            let before_recovery = fixture.wire.requests();
            assert!(before_recovery
                .last()
                .unwrap()
                .starts_with("POST /drive/v3/files?"));
            assert_eq!(
                before_recovery
                    .iter()
                    .filter(|r| r.starts_with("POST "))
                    .count(),
                1
            );
            let mut observer = fixture.client("observer.json");
            let observed = observer.observe_prepared_slot(&prepared).unwrap();
            assert_eq!(observed.pointer, pointer);
            assert_eq!(observed.descriptor, bytes);
            assert!(fixture.wire.requests()[before_recovery.len()..]
                .iter()
                .all(|r| r.starts_with("GET ")));
        }
    }

    #[test]
    fn negative_fixtures_use_literal_private_properties_and_media_bytes() {
        let fixture = Fixture::new();
        let mut client = fixture.client("original.json");
        let corrupt = resource(23);
        assert!(stage(&mut client, &corrupt, |_| Ok(()))
            .unwrap()
            .is_some_and(|s| (200..300).contains(&s)));
        let expected = descriptor_bytes(RUN, GROUP, "negative-expected");
        let mut bad_bytes = expected.clone();
        bad_bytes.extend_from_slice(b"deliberate-bad-body");
        let meta = client.metadata(&corrupt.id).unwrap();
        assert_eq!(
            meta.app_properties,
            Tag {
                collection: GROUP,
                kind: "object".into(),
                sha256: digest(&expected)
            }
            .properties()
        );
        assert_eq!(
            client.download(&corrupt.id, MEDIA_MAX as u64).unwrap(),
            bad_bytes
        );
        for (index, invalid) in [
            (14, SlotInvalid::Metadata),
            (15, SlotInvalid::Descriptor),
            (16, SlotInvalid::Descriptor),
        ] {
            let fixture = resource(index);
            assert_eq!(stage(&mut client, &fixture, |_| Ok(())).unwrap(), Some(201));
            let pointer = fixture.pointer().unwrap();
            let mut expected = pointer.properties().unwrap();
            if index == 14 {
                assert!(expected.remove("descriptor_id").is_some());
            }
            assert_eq!(
                client.metadata(&fixture.id).unwrap().app_properties,
                expected
            );
            assert!(
                matches!(client.observe_agreed_slot(&pointer.binding).unwrap(), AgreedSlotObservation::Invalid(actual) if actual == invalid)
            );
        }
        assert_eq!(
            fixture
                .wire
                .requests()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            4
        );
    }

    #[test]
    fn cleanup_never_deletes_metadata_or_body_mismatches() {
        let fixture = Fixture::new();
        let mut client = fixture.client("original.json");
        let normal = resource(18);
        fixture
            .wire
            .object(&normal.id, GROUP, b"other metadata".to_vec());
        assert!(!delete_verified(&mut client, &normal).unwrap());
        let mut corrupt = resource(23);
        stage(&mut client, &corrupt, |_| Ok(())).unwrap();
        let Shape::Descriptor { bytes, .. } = &mut corrupt.shape else {
            panic!("descriptor case");
        };
        bytes.push(0);
        assert!(!delete_verified(&mut client, &corrupt).unwrap());
        assert!(fixture
            .wire
            .requests()
            .iter()
            .all(|r| !r.starts_with("DELETE ")));
    }

    #[test]
    fn cleanup_deletes_only_after_exact_metadata_and_body_then_checks_absence() {
        // The existing shared fixture has no DELETE route. Script only this
        // exact request sequence and exercise the real adapter against it.
        let fixture = Fixture::new();
        let mut client = fixture.client("original.json");
        let descriptor = resource(18);
        let metadata = serde_json::to_vec(&json!({
            "id": descriptor.id, "appProperties": descriptor.properties().unwrap(), "trashed": false,
        })).unwrap();
        let account =
            serde_json::to_vec(&json!({"user":{"permissionId":"shared-wire-account"}})).unwrap();
        let bytes = descriptor_bytes(RUN, GROUP, "race-a");
        let replies = [
            ("GET /drive/v3/about?", 200, account.clone()),
            (
                "GET /drive/v3/files/qualification-18?fields=",
                200,
                metadata,
            ),
            (
                "GET /drive/v3/files/qualification-18?alt=media ",
                200,
                bytes,
            ),
            ("DELETE /drive/v3/files/qualification-18 ", 204, Vec::new()),
            ("GET /drive/v3/about?", 200, account),
            (
                "GET /drive/v3/files/qualification-18?fields=",
                404,
                b"{}".to_vec(),
            ),
        ];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        client.api = format!("http://{}/drive/v3", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for (prefix, status, body) in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "cleanup fixture request deadline"
                            );
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("cleanup fixture accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(prefix));
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let result = delete_verified(&mut client, &descriptor);
        server.join().unwrap();
        assert!(result.unwrap());
    }
}
