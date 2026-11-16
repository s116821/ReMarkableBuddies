use super::*;
use crate::{
    mode_maintenance::{self, Operation, Status as ModeStatus},
    storage::{sync::mode::SyncModeChoice, StorePaths},
};
use serde_json::{json, Value};
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture {
    root: PathBuf,
    paths: StorePaths,
    config: PathBuf,
    token: Value,
    collection: Uuid,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-drive-credential-{}", Uuid::new_v4()));
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let config = root.join("config.json");
        let store = Store::open(paths.clone()).unwrap();
        files::atomic_json(&config, &json!({"paths":paths})).unwrap();
        let token = Self::choose(&store, SyncModeChoice::LocalOnly);
        Self {
            root,
            paths,
            config,
            token,
            collection: Uuid::new_v4(),
        }
    }

    fn choose(store: &Store, mode: SyncModeChoice) -> Value {
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                mode,
            )
            .unwrap();
        serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap()
    }

    fn target(&self) -> PathBuf {
        self.paths.credentials.join("drive.json")
    }

    fn credential(&self, token: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "credential_version": 1,
            "client_id": "fixture.apps.googleusercontent.com",
            "client_secret": null,
            "access_token": token,
            "refresh_token": "never-emit-synthetic-secret",
            "expires_at": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 1800,
            "scopes": [APP_SCOPE],
            "account_permission_id": "never-emit-synthetic-account",
            "collection": self.collection
        }))
        .unwrap()
    }

    fn report(&self, report: ModeReport) -> Value {
        let mut bytes = Vec::new();
        report.write_to(&mut bytes).unwrap();
        let json = std::str::from_utf8(&bytes).unwrap();
        assert!(json.ends_with('\n'));
        assert!(!json.contains("never-emit") && !json.contains(self.root.to_str().unwrap()));
        serde_json::from_str(json).unwrap()
    }

    fn run(&self, request: &Value) -> Value {
        let bytes = serde_json::to_vec(request).unwrap();
        self.report(mode_maintenance::run(
            Operation::DriveCredential,
            Some(Path::new("-")),
            Some(self.config.clone()),
            &mut bytes.as_slice(),
        ))
    }

    fn with_hook(&self, request: &Value, hook: impl FnMut(Phase) -> Checked<()>) -> Value {
        let bytes = serde_json::to_vec(request).unwrap();
        self.report(run_with(
            ModeReport::new(Operation::DriveCredential, ModeStatus::Unavailable),
            Some(self.config.clone()),
            &mut bytes.as_slice(),
            hook,
        ))
    }

    fn inspect(&self) -> Value {
        let report =
            self.run(&json!({"control_version":1,"action":"inspect","expected_mode":self.token}));
        assert_eq!(report["outcome"], "observed", "{report}");
        report["current"].clone()
    }

    fn apply(&self, kind: &str, original: &Value, bytes: &[u8]) -> Value {
        let mut change = json!({
            "kind":kind, "collection":self.collection, "binding_sha256":decode(bytes).unwrap().binding_digest(),
        });
        if kind != "remove" {
            change["credential_b64"] = json!(STANDARD.encode(bytes));
        }
        json!({"control_version":1,"action":"apply","operation":Uuid::new_v4(),"original":original,"change":change})
    }

    fn readback(&self, apply: &Value, desired: Value) -> Value {
        self.run(
            &json!({"control_version":1,"action":"readback","operation":apply["operation"],
            "original":apply["original"],"desired":desired}),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn fixed_selector_creates_then_reconnects_same_isolated_binding_and_removes() {
    let mut fixture = Fixture::new();
    let first = fixture.credential("never-emit-first-access");
    let second = fixture.credential("never-emit-second-access");
    let initial = fixture.inspect();
    assert_eq!(initial["slot"], json!({"kind":"absent"}));
    let create = fixture.apply("create", &initial, &first);
    assert_eq!(fixture.run(&create)["outcome"], "verified");
    assert_eq!(fs::read(fixture.target()).unwrap(), first);
    assert_eq!(
        fs::metadata(fixture.target()).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(fixture.run(&create)["outcome"], "conflict");
    assert_eq!(fixture.inspect()["mode"], initial["mode"]);

    let store = Store::open_existing_for_mode(fixture.paths.clone()).unwrap();
    fixture.token = Fixture::choose(
        &store,
        SyncModeChoice::IsolatedBackup {
            binding_sha256: decode(&first).unwrap().binding_digest(),
        },
    );
    drop(store);
    let isolated = fixture.inspect();
    let replace = fixture.apply("replace", &isolated, &second);
    let result = fixture.run(&replace);
    assert_eq!(result["outcome"], "verified");
    assert_eq!(result["current"]["epoch"], isolated["epoch"]);
    assert_eq!(result["current"]["mode"], isolated["mode"]);
    assert_eq!(fs::read(fixture.target()).unwrap(), second);
    assert_eq!(fixture.run(&replace)["outcome"], "conflict");

    let remove = fixture.apply("remove", &fixture.inspect(), &second);
    assert_eq!(fixture.run(&remove)["outcome"], "verified");
    assert_eq!(
        fixture.readback(&remove, json!({"kind":"absent"}))["outcome"],
        "desired-state-observed"
    );
    assert!(!fixture.target().exists());
    assert_eq!(fixture.inspect()["mode_token"], isolated["mode_token"]);
    let reconnect = fixture.apply("create", &fixture.inspect(), &second);
    assert_eq!(fixture.run(&reconnect)["outcome"], "verified");
}

#[test]
fn exact_original_readback_handles_lost_ack_without_repeating_mutation() {
    let fixture = Fixture::new();
    let bytes = fixture.credential("never-emit-after-lost-ack");
    let create = fixture.apply("create", &fixture.inspect(), &bytes);
    let desired = json!({"kind":"present","sha256":digest(&bytes),
        "binding_sha256":decode(&bytes).unwrap().binding_digest(),"collection":fixture.collection});
    assert_eq!(
        fixture.readback(&create, desired.clone())["outcome"],
        "original-state-observed"
    );
    let failed = fixture.with_hook(&create, |phase| {
        if phase == Phase::BeforeReadback {
            Err(Status::Unavailable)
        } else {
            Ok(())
        }
    });
    assert_eq!(failed["outcome"], "outcome-unknown");
    assert_eq!(failed["disk_readback_verified"], false);
    let readback = fixture.readback(&create, desired);
    assert_eq!(readback["outcome"], "desired-state-observed");
    assert_eq!(readback["operation"], create["operation"]);
    assert_eq!(fs::read(fixture.target()).unwrap(), bytes);

    let mut config = fs::read(&fixture.config).unwrap();
    config.push(b'\n');
    fs::write(&fixture.config, config).unwrap();
    let original_slot = readback["current"]["slot"].clone();
    let desired = json!({"kind":"present","sha256":original_slot["sha256"],
        "binding_sha256":original_slot["binding_sha256"],"collection":fixture.collection});
    assert_eq!(fixture.readback(&create, desired)["outcome"], "unresolved");
}

#[test]
fn refuses_busy_unsafe_leaf_and_drift_before_publication() {
    let fixture = Fixture::new();
    let bytes = fixture.credential("never-emit-safe-only");
    let original = fixture.inspect();
    let create = fixture.apply("create", &original, &bytes);
    let live_store = Store::open_existing_for_mode(fixture.paths.clone()).unwrap();
    assert_eq!(fixture.run(&create)["outcome"], "busy");
    drop(live_store);
    let foreign = fixture.root.join("foreign");
    fs::write(&foreign, b"unrelated").unwrap();
    symlink(&foreign, fixture.target()).unwrap();
    assert_eq!(fixture.run(&create)["outcome"], "unsupported");
    assert_eq!(fs::read(&foreign).unwrap(), b"unrelated");
    fs::remove_file(fixture.target()).unwrap();
    fs::hard_link(&foreign, fixture.target()).unwrap();
    assert_eq!(fixture.run(&create)["outcome"], "unsupported");
    fs::remove_file(fixture.target()).unwrap();
    fs::write(fixture.target(), &bytes).unwrap();
    fs::set_permissions(fixture.target(), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(fixture.run(&create)["outcome"], "unsupported");
    assert_eq!(fs::read(fixture.target()).unwrap(), bytes);
    fs::remove_file(fixture.target()).unwrap();

    let result = fixture.with_hook(&create, |phase| {
        if phase == Phase::Staged {
            let mut config = fs::read(&fixture.config).unwrap();
            config.push(b'\n');
            fs::write(&fixture.config, config).unwrap();
        }
        Ok(())
    });
    assert_eq!(result["outcome"], "conflict");
    assert!(!fixture.target().exists());
    assert_eq!(fs::read_dir(&fixture.paths.credentials).unwrap().count(), 1); // only owner.json
}

#[test]
fn rejects_noncanonical_grant_and_input_file() {
    let fixture = Fixture::new();
    let bytes = fixture.credential("never-emit-bad");
    let original = fixture.inspect();
    let mut create = fixture.apply("create", &original, &bytes);
    let mut credential: Value = serde_json::from_slice(&bytes).unwrap();
    credential["scopes"] = json!([APP_SCOPE, "extra"]);
    create["change"]["credential_b64"] =
        json!(STANDARD.encode(serde_json::to_vec(&credential).unwrap()));
    assert_eq!(fixture.run(&create)["outcome"], "invalid-input");
    create["change"]["collection"] = json!(fixture.collection.simple().to_string());
    assert_eq!(fixture.run(&create)["outcome"], "invalid-input");
    let report = mode_maintenance::run(
        Operation::DriveCredential,
        Some(&fixture.config),
        Some(fixture.config.clone()),
        &mut b"".as_slice(),
    );
    assert_eq!(fixture.report(report)["outcome"], "invalid-input");
    assert!(!fixture.target().exists());
}
