use super::*;
use crate::{
    mode_maintenance, restore_maintenance,
    storage::{
        digest,
        ordinary_restore::tests::{record, Fixture as RestoreFixture},
        sync::mode::SyncModeChoice,
        Store,
    },
};
use serde_json::{json, Value};
use std::{cell::Cell, collections::BTreeMap, os::unix::fs::symlink, sync::Barrier, thread};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Fault {
    Rename,
    BeforePublication,
    AfterPublication,
}

thread_local! {
    static FAILURE: Cell<Option<Fault>> = const { Cell::new(None) };
}

pub(super) fn fault(phase: Fault) -> io::Result<()> {
    FAILURE.with(|slot| {
        if slot.get() != Some(phase) {
            return Ok(());
        }
        slot.set(None);
        Err(io::Error::from_raw_os_error(match phase {
            Fault::Rename => libc::EOPNOTSUPP,
            Fault::BeforePublication | Fault::AfterPublication => libc::EIO,
        }))
    })
}

struct Fixture {
    home: PathBuf,
    config: PathBuf,
    defaults: Config,
}

impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("buddy-initial-config-{}", Uuid::new_v4()));
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, Permissions::from_mode(0o700)).unwrap();
        let config = home.join(".config/remarkable-buddies/config.json");
        let defaults = Config {
            paths: StorePaths {
                data: home.join(".local/share/remarkable-buddies"),
                cache: home.join(".cache/remarkable-buddies"),
                credentials: home.join(".config/remarkable-buddies/credentials"),
            },
            ..Config::default()
        };
        Self {
            home,
            config,
            defaults,
        }
    }

    fn run(&self, action: Action) -> State {
        Tree::open(&self.home, &self.home, &self.config)
            .and_then(|tree| run_in_tree(&tree, action, &self.defaults))
            .unwrap_or(State::Blocked)
    }

    fn parent(&self) -> &Path {
        self.config.parent().unwrap()
    }

    fn assert_no_store(&self) {
        for path in [
            &self.defaults.paths.data,
            &self.defaults.paths.cache,
            &self.defaults.paths.credentials,
        ] {
            assert!(!path.exists(), "initializer created {path:?}");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !thread::panicking() {
            assert_eq!(self.home.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self
                .home
                .file_name()
                .unwrap()
                .as_bytes()
                .starts_with(b"buddy-initial-config-"));
            let _ = fs::remove_dir_all(&self.home);
        }
    }
}

#[test]
fn production_defaults_are_fixed_and_secret_free() {
    let bytes = serde_json::to_vec(&Config::default()).unwrap();
    let (parsed, _) = inspection::document(&bytes).unwrap();
    assert_eq!(
        super::super::FIXED_PATH,
        "/home/root/.config/remarkable-buddies/config.json"
    );
    assert_eq!(
        parsed.paths.data,
        Path::new("/home/root/.local/share/remarkable-buddies")
    );
    assert_eq!(
        parsed.paths.cache,
        Path::new("/home/root/.cache/remarkable-buddies")
    );
    assert_eq!(
        parsed.paths.credentials,
        Path::new("/home/root/.config/remarkable-buddies/credentials")
    );
    assert!(parsed.model_credential.is_none());
    assert!(!parsed.sync.enabled);
}

#[test]
fn check_is_read_only_and_create_publishes_one_complete_private_file() {
    let f = Fixture::new();
    assert_eq!(f.run(Action::CheckInitial), State::Missing);
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
    assert_eq!(f.run(Action::CreateInitial), State::Created);
    assert_eq!(f.run(Action::CheckInitial), State::Present);
    assert_eq!(f.run(Action::CreateInitial), State::Present);
    assert_eq!(
        fs::read(&f.config).unwrap(),
        serde_json::to_vec(&f.defaults).unwrap()
    );
    let meta = f.config.metadata().unwrap();
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(meta.nlink(), 1);
    assert_eq!(fs::read_dir(f.parent()).unwrap().count(), 1);
    f.assert_no_store();
}

#[test]
fn existing_valid_file_bypasses_creation_only_roots_and_keeps_exact_bytes() {
    let f = Fixture::new();
    fs::create_dir_all(f.parent()).unwrap();
    fs::create_dir_all(&f.defaults.paths.data).unwrap();
    fs::create_dir_all(&f.defaults.paths.credentials).unwrap();
    let sentinel = f.defaults.paths.credentials.join("existing");
    fs::write(&sentinel, b"keep-secret").unwrap();
    let baseline = b"{ \"model\": \"keep-existing\" }\n";
    fs::write(&f.config, baseline).unwrap();
    let original = f.config.metadata().unwrap();
    for action in [Action::CheckInitial, Action::CreateInitial] {
        assert_eq!(f.run(action), State::Present);
    }
    assert!(same_inode(&original, &f.config.metadata().unwrap()));
    assert_eq!(fs::read(&f.config).unwrap(), baseline);
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep-secret");
}

#[test]
fn existing_default_setup_preserves_owned_data_and_unlocks_the_revision_editor() {
    use crate::{
        config::personalization::{self, Operation, Status},
        conversation::{Ledger, Record},
    };
    use std::sync::Arc;

    let f = Fixture::new();
    assert_eq!(f.run(Action::CheckExistingDefault), State::Blocked);
    f.assert_no_store();
    let store = Arc::new(Store::open(f.defaults.paths.clone()).unwrap());
    let conversation = Uuid::new_v4();
    Ledger::new(store.clone())
        .create(conversation, Uuid::new_v4(), 1)
        .unwrap();
    let credential = f.defaults.paths.credentials.join("keep");
    fs::write(&credential, b"retained credential").unwrap();
    let current = f.defaults.paths.data.join("CURRENT");
    let before = fs::read(&current).unwrap();
    assert_eq!(f.run(Action::CreateExistingDefault), State::Blocked);
    assert!(!f.config.exists());
    drop(store);

    assert_eq!(f.run(Action::CheckInitial), State::Blocked);
    assert_eq!(f.run(Action::CreateInitial), State::Blocked);
    assert_eq!(f.run(Action::CheckExistingDefault), State::Missing);
    assert!(!f.config.exists());
    assert_eq!(f.run(Action::CreateExistingDefault), State::Created);
    assert_eq!(f.run(Action::CheckExistingDefault), State::Present);
    assert_eq!(
        fs::read(&f.config).unwrap(),
        serde_json::to_vec(&f.defaults).unwrap()
    );

    let original =
        personalization::run(Operation::Inspect, Some(f.config.clone()), &mut io::empty());
    let input = serde_json::to_vec(&json!({
        "contract_version": 1,
        "expected_config_sha256": original.config_sha256.unwrap(),
        "enabled": true,
        "text": "Use concise examples.\nPreserve π.",
    }))
    .unwrap();
    let saved = personalization::run(
        Operation::Set,
        Some(f.config.clone()),
        &mut input.as_slice(),
    );
    assert_eq!(saved.status, Status::Ok);
    assert!(saved.disk_reload_verified);
    assert_eq!(
        saved.personalization.unwrap().text,
        "Use concise examples.\nPreserve π."
    );
    let edited = fs::read(&f.config).unwrap();
    assert_eq!(f.run(Action::CreateExistingDefault), State::Present);
    assert_eq!(fs::read(&f.config).unwrap(), edited);
    assert_eq!(fs::read(credential).unwrap(), b"retained credential");
    assert_eq!(fs::read(current).unwrap(), before);
    let ledger = Ledger::new(Arc::new(Store::open(f.defaults.paths.clone()).unwrap()));
    assert!(ledger
        .inspect(conversation, false)
        .unwrap()
        .iter()
        .any(|record| matches!(record, Record::Root(root) if root.id == conversation)));
}

#[test]
fn existing_default_setup_refuses_conflicting_and_unsafe_configuration_entries() {
    let f = Fixture::new();
    drop(Store::open(f.defaults.paths.clone()).unwrap());
    let legacy = f.home.join(".config/reader-buddy/config.json");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, b"{}").unwrap();
    assert_eq!(f.run(Action::CheckExistingDefault), State::Blocked);
    assert_eq!(f.run(Action::CreateExistingDefault), State::Blocked);
    assert!(!f.config.exists());
    fs::write(&f.config, b"{}").unwrap();
    assert_eq!(f.run(Action::CreateExistingDefault), State::Blocked);
    assert_eq!(fs::read(&f.config).unwrap(), b"{}");
    assert_eq!(fs::read(&legacy).unwrap(), b"{}");
    fs::remove_file(&legacy).unwrap();
    symlink(f.home.join("missing"), &legacy).unwrap();
    assert_eq!(f.run(Action::CheckExistingDefault), State::Blocked);
    fs::remove_file(&legacy).unwrap();
    fs::write(&f.config, b"invalid").unwrap();
    assert_eq!(f.run(Action::CheckExistingDefault), State::Blocked);
    assert_eq!(f.run(Action::CreateExistingDefault), State::Blocked);
    assert_eq!(fs::read(&f.config).unwrap(), b"invalid");
}

#[test]
fn existing_default_setup_keeps_possible_publication_and_serializes_competing_requests() {
    let f = Fixture::new();
    drop(Store::open(f.defaults.paths.clone()).unwrap());
    FAILURE.with(|slot| slot.set(Some(Fault::BeforePublication)));
    assert_eq!(f.run(Action::CreateExistingDefault), State::Blocked);
    assert!(!f.config.exists());
    FAILURE.with(|slot| slot.set(Some(Fault::AfterPublication)));
    assert_eq!(f.run(Action::CreateExistingDefault), State::Unknown);
    assert_eq!(f.run(Action::CheckExistingDefault), State::Present);
    assert_eq!(
        fs::read(&f.config).unwrap(),
        serde_json::to_vec(&f.defaults).unwrap()
    );

    let f = Fixture::new();
    drop(Store::open(f.defaults.paths.clone()).unwrap());
    let barrier = Barrier::new(2);
    let results = thread::scope(|s| {
        let left = s.spawn(|| {
            barrier.wait();
            f.run(Action::CreateExistingDefault)
        });
        barrier.wait();
        let right = f.run(Action::CreateExistingDefault);
        [left.join().unwrap(), right]
    });
    assert_eq!(
        results
            .iter()
            .filter(|state| **state == State::Created)
            .count(),
        1
    );
    assert!(results
        .iter()
        .all(|state| matches!(state, State::Created | State::Present | State::Blocked)));
    assert_eq!(f.run(Action::CheckExistingDefault), State::Present);
}

#[test]
fn refuses_ambiguous_existing_files_without_repair() {
    let f = Fixture::new();
    fs::create_dir_all(f.parent()).unwrap();
    let sentinel = f.home.join("unrelated");
    fs::write(&sentinel, b"{}").unwrap();
    for content in [b"bad json".as_slice(), b"{\"foreign\":true}".as_slice()] {
        fs::write(&f.config, content).unwrap();
        assert_eq!(f.run(Action::CheckInitial), State::Blocked);
        assert_eq!(f.run(Action::CreateInitial), State::Blocked);
        assert_eq!(fs::read(&f.config).unwrap(), content);
    }
    fs::remove_file(&f.config).unwrap();
    fs::hard_link(&sentinel, &f.config).unwrap();
    assert_eq!(f.run(Action::CreateInitial), State::Blocked);
    fs::remove_file(&f.config).unwrap();
    symlink(f.home.join("absent"), &f.config).unwrap();
    assert_eq!(f.run(Action::CreateInitial), State::Blocked);
    assert!(fs::symlink_metadata(&f.config).unwrap().is_symlink());
    assert_eq!(fs::read(&sentinel).unwrap(), b"{}");
}

#[test]
fn refuses_missing_config_with_nonempty_store_or_unsafe_ancestry() {
    let f = Fixture::new();
    fs::create_dir_all(&f.defaults.paths.data).unwrap();
    let sentinel = f.defaults.paths.data.join("data");
    fs::write(&sentinel, b"keep").unwrap();
    assert_eq!(f.run(Action::CheckInitial), State::Blocked);
    assert_eq!(f.run(Action::CreateInitial), State::Blocked);
    assert!(!f.parent().exists());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");

    let g = Fixture::new();
    let other = g.home.join("other");
    fs::create_dir(&other).unwrap();
    symlink(&other, g.home.join(".config")).unwrap();
    assert_eq!(g.run(Action::CheckInitial), State::Blocked);
    assert_eq!(g.run(Action::CreateInitial), State::Blocked);
    assert_eq!(fs::read_dir(&other).unwrap().count(), 0);
}

#[test]
fn concurrent_creators_have_one_winner_and_no_staging_residue() {
    let f = Fixture::new();
    let barrier = Barrier::new(2);
    let create = || {
        barrier.wait();
        f.run(Action::CreateInitial)
    };
    let (first, second) = thread::scope(|scope| {
        let first = scope.spawn(create);
        let second = scope.spawn(create);
        (first.join().unwrap(), second.join().unwrap())
    });
    assert!(matches!(
        (first, second),
        (State::Created, State::Present) | (State::Present, State::Created)
    ));
    assert_eq!(fs::read_dir(f.parent()).unwrap().count(), 1);
    assert_eq!(
        fs::read(&f.config).unwrap(),
        serde_json::to_vec(&f.defaults).unwrap()
    );
    f.assert_no_store();
}

#[test]
fn stage_cleanup_leaves_a_replaced_name_untouched() {
    let f = Fixture::new();
    fs::create_dir_all(f.parent()).unwrap();
    let parent = File::open(f.parent()).unwrap();
    let stage = Stage::new(&parent).unwrap();
    let replaced = f
        .parent()
        .join(std::ffi::OsStr::from_bytes(stage.name.to_bytes()));
    let detached = f.parent().join("detached-stage");
    fs::rename(&replaced, &detached).unwrap();
    fs::write(&replaced, b"unrelated").unwrap();
    drop(stage);
    assert_eq!(fs::read(&replaced).unwrap(), b"unrelated");
    assert!(detached.exists());
    assert!(!f.config.exists());
}

#[test]
fn unsupported_and_prepublication_failure_block_but_postpublication_is_unknown() {
    for fault in [Fault::Rename, Fault::BeforePublication] {
        let f = Fixture::new();
        FAILURE.with(|slot| slot.set(Some(fault)));
        assert_eq!(f.run(Action::CreateInitial), State::Blocked);
        assert!(!f.config.exists());
        assert_eq!(fs::read_dir(f.parent()).unwrap().count(), 0);
        f.assert_no_store();
    }
    let f = Fixture::new();
    FAILURE.with(|slot| slot.set(Some(Fault::AfterPublication)));
    assert_eq!(f.run(Action::CreateInitial), State::Unknown);
    assert_eq!(f.run(Action::CheckInitial), State::Present);
    assert_eq!(
        fs::read(&f.config).unwrap(),
        serde_json::to_vec(&f.defaults).unwrap()
    );
    assert_eq!(fs::read_dir(f.parent()).unwrap().count(), 1);
    f.assert_no_store();
}

#[test]
fn anchored_creation_composes_with_initialize_and_verified_foreign_actor_restore() {
    let target = Fixture::new();
    assert_eq!(target.run(Action::CreateInitial), State::Created);
    target.assert_no_store();
    let original = fs::read(&target.config).unwrap();

    let mut output = Vec::new();
    mode_maintenance::run(
        mode_maintenance::Operation::Initialize,
        None,
        Some(target.config.clone()),
        &mut io::empty(),
    )
    .write_to(&mut output)
    .unwrap();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["status"], "ok");
    let target_actor = Store::open_existing(target.defaults.paths.clone())
        .unwrap()
        .actor_id;
    let secret = target.defaults.paths.credentials.join("target-secret");
    fs::write(&secret, b"target-only").unwrap();

    let origin = RestoreFixture::new();
    let source = origin.open();
    let entry = record(&source);
    assert_ne!(target_actor, entry.actor_id);
    source.commit(vec![entry.clone()], BTreeMap::new()).unwrap();
    source
        .set_sync_mode(
            &source.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::IsolatedBackup {
                binding_sha256: digest(b"fixture binding"),
            },
        )
        .unwrap();
    let lease = source
        .sync_mode_lease(&source.sync_mode_snapshot().unwrap().token)
        .unwrap();
    let package = origin.root.join("package");
    let mut attempted = None;
    let exported = source
        .export_isolated_package(
            &lease,
            &package,
            &origin.root.join("config.json"),
            &mut attempted,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        Store::inspect_isolated_package(&package, attempted.as_deref()).unwrap(),
        exported
    );
    assert!(!exported.data.configuration_included);

    let data = package.join("data");
    let restore = |operation, value: Value| {
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut output = Vec::new();
        restore_maintenance::run(
            operation,
            Path::new("-"),
            Some(target.config.clone()),
            &mut bytes.as_slice(),
        )
        .write_to(&mut output)
        .unwrap();
        serde_json::from_slice::<Value>(&output).unwrap()
    };
    let prepared = restore(
        restore_maintenance::Operation::Prepare,
        json!({
            "restore_version": 1, "operation": Uuid::new_v4(), "source": data
        }),
    );
    assert_eq!(prepared["outcome"], "prepared");
    let intent = json!({ "source": data, "intent": prepared["data"]["intent"] });
    let applied = restore(restore_maintenance::Operation::Apply, intent.clone());
    assert_eq!(applied["outcome"], "restored");
    assert_eq!(applied["disk_readback_verified"], true);
    let observed = restore(restore_maintenance::Operation::Status, intent);
    assert_eq!(observed["outcome"], "desired-generation-observed");

    let restored = Store::open_existing(target.defaults.paths.clone()).unwrap();
    assert_eq!(restored.actor_id, target_actor);
    assert!(restored.sync_mode_snapshot().unwrap().mode.is_none());
    let heads = restored.heads(entry.namespace, entry.record_id).unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].actor_id, entry.actor_id);
    assert_eq!(heads[0].revision_id, entry.revision_id);
    assert_eq!(fs::read(&target.config).unwrap(), original);
    assert_eq!(fs::read(&secret).unwrap(), b"target-only");
}
