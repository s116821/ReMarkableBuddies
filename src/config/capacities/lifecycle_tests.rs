use super::*;
use crate::config::capacities::{
    access_token,
    tests::{grant_fixture, Temp},
};
use serde_json::json;
use std::os::unix::fs::{symlink, PermissionsExt};

fn grant_path(root: &Path, id: Uuid) -> PathBuf {
    root.join(format!("capacities-grant-{id}.json"))
}

fn legacy(root: &Path, space: Uuid, id: Uuid) -> serde_json::Value {
    let grant = grant_fixture(id, space, now().unwrap() + 900);
    let path = grant_path(root, id);
    fs::write(&path, serde_json::to_vec(&grant).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    grant
}

#[test]
fn access_lease_excludes_mutation_and_other_readers_for_the_same_grant() {
    let temp = Temp::new();
    let id = Uuid::new_v4();
    let space = Uuid::new_v4();
    legacy(temp.path(), space, id);
    let access = access_token(temp.path(), space, id, true).unwrap();
    assert_eq!(access.expose(), "synthetic-access-token");
    assert!(!format!("{access:?}").contains("synthetic-access-token"));
    assert_eq!(
        access_token(temp.path(), space, id, true).unwrap_err(),
        Failure::Conflict
    );
    assert!(matches!(
        GrantOwner::open(temp.path(), id),
        Err(Failure::Conflict)
    ));
    assert!(GrantOwner::open(temp.path(), Uuid::new_v4()).is_ok());

    drop(access);
    assert_eq!(
        GrantOwner::open(temp.path(), id)
            .unwrap()
            .status()
            .unwrap()
            .phase,
        Phase::Active
    );
    let link = Uuid::new_v4();
    let target = temp.path().join(format!("capacities-grant-{id}.lock"));
    symlink(
        &target,
        temp.path().join(format!("capacities-grant-{link}.lock")),
    )
    .unwrap();
    assert!(matches!(
        GrantOwner::open(temp.path(), link),
        Err(Failure::Storage)
    ));
}

#[test]
fn interrupted_rotation_hides_old_tokens_and_never_reopens_for_retry() {
    let temp = Temp::new();
    let id = Uuid::new_v4();
    let space = Uuid::new_v4();
    let operation = Uuid::new_v4();
    let old = legacy(temp.path(), space, id);
    let mut rotation = GrantOwner::open(temp.path(), id)
        .unwrap()
        .begin_rotation(space, operation)
        .unwrap();
    let token = rotation.release_refresh_token().unwrap();
    assert!(matches!(
        GrantOwner::open(temp.path(), id),
        Err(Failure::Conflict)
    ));
    assert_eq!(token, "synthetic-refresh-token");
    assert_eq!(
        rotation.release_refresh_token().unwrap_err(),
        Failure::Conflict
    );
    let state = fs::read_to_string(grant_path(temp.path(), id)).unwrap();
    for field in ["access_token", "refresh_token"] {
        assert!(!state.contains(old[field].as_str().unwrap()));
    }
    assert_eq!(
        rotation
            .install(&serde_json::to_vec(&old).unwrap())
            .unwrap_err(),
        Failure::Conflict
    );

    let owner = GrantOwner::open(temp.path(), id).unwrap();
    assert_eq!(owner.status().unwrap().phase, Phase::PendingRotation);
    assert_eq!(owner.status().unwrap().operation_id, Some(operation));
    assert_eq!(
        owner.access_token(space, true).unwrap_err(),
        Failure::Authentication
    );
    let owner = GrantOwner::open(temp.path(), id).unwrap();
    assert!(matches!(
        owner.begin_rotation(space, Uuid::new_v4()),
        Err(Failure::Conflict)
    ));
    let owner = GrantOwner::open(temp.path(), id).unwrap();
    owner.disable_local(space).unwrap();
    assert_eq!(owner.status().unwrap().phase, Phase::LocalDisabled);
    assert!(matches!(
        owner.begin_revocation(space, Uuid::new_v4()),
        Err(Failure::Conflict)
    ));
}

#[test]
fn stale_fence_refuses_dispatch_and_visible_replacement_requires_directory_sync() {
    let temp = Temp::new();
    let id = Uuid::new_v4();
    let space = Uuid::new_v4();
    legacy(temp.path(), space, id);
    let mut rotation = GrantOwner::open(temp.path(), id)
        .unwrap()
        .begin_rotation(space, Uuid::new_v4())
        .unwrap();
    let path = grant_path(temp.path(), id);
    let mut changed: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    changed["state"]["operation_id"] = json!(Uuid::new_v4());
    fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(
        rotation.release_refresh_token().unwrap_err(),
        Failure::Conflict
    );
    drop(rotation);
    assert_eq!(
        access_token(temp.path(), space, id, false).unwrap_err(),
        Failure::Authentication
    );

    let next_id = Uuid::new_v4();
    let mut grant = legacy(temp.path(), space, next_id);
    let mut next = GrantOwner::open(temp.path(), next_id)
        .unwrap()
        .begin_rotation(space, Uuid::new_v4())
        .unwrap();
    next.release_refresh_token().unwrap();
    grant["access_token"] = json!("synthetic-replacement-access");
    grant["refresh_token"] = json!("synthetic-replacement-refresh");
    let path = grant_path(temp.path(), next_id);
    files::fail_next_atomic(path.clone(), files::AtomicFailure::ParentSync);
    assert_eq!(
        next.install(&serde_json::to_vec(&grant).unwrap())
            .unwrap_err(),
        Failure::Storage
    );
    let access = access_token(temp.path(), space, next_id, true).unwrap();
    assert_eq!(access.expose(), "synthetic-replacement-access");
    assert_eq!(fs::symlink_metadata(path).unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn staged_exchange_resumes_after_sync_and_exact_original_config_readback() {
    let temp = Temp::new();
    let id = Uuid::new_v4();
    let space = Uuid::new_v4();
    let binding = Enrollment {
        operation_id: Uuid::new_v4(),
        tablet_id: "fixture-tablet".to_owned(),
        connection_id: "a".repeat(64),
        boot_id: Uuid::new_v4(),
        config_sha256: "b".repeat(64),
        client_id: "fixture-public-client".to_owned(),
    };
    let mut permit = GrantOwner::open(temp.path(), id)
        .unwrap()
        .begin_exchange(binding.clone())
        .unwrap();
    permit.authorize_dispatch().unwrap();
    assert_eq!(permit.authorize_dispatch().unwrap_err(), Failure::Conflict);
    let grant = grant_fixture(id, space, now().unwrap() + 900);
    let target = "c".repeat(64);
    let path = grant_path(temp.path(), id);
    files::fail_next_atomic(path.clone(), files::AtomicFailure::ParentSync);
    assert!(matches!(
        permit.stage(space, &serde_json::to_vec(&grant).unwrap(), target.clone()),
        Err(Failure::Storage)
    ));

    let owner = GrantOwner::open(temp.path(), id).unwrap();
    assert_eq!(
        owner
            .status_after_sync(|_| Err(anyhow::anyhow!("fixture sync refusal")))
            .unwrap_err(),
        Failure::Storage
    );
    let original = fs::read(&path).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["target_config_sha256"] = json!("d".repeat(64));
    assert_eq!(
        owner
            .status_after_sync(|root| {
                files::sync_dir(root)?;
                files::atomic(&path, &serde_json::to_vec(&changed)?)
            })
            .unwrap_err(),
        Failure::Conflict
    );
    files::atomic(&path, &original).unwrap();
    let found = owner.status().unwrap();
    assert_eq!(found.phase, Phase::Staged);
    assert_eq!(found.enrollment, Some(binding.clone()));
    assert_eq!(found.target_config_sha256, Some(target.clone()));
    assert_eq!(
        owner.access_token(space, true).unwrap_err(),
        Failure::Authentication
    );

    let owner = GrantOwner::open(temp.path(), id).unwrap();
    assert_eq!(
        owner
            .activate_after_config_readback(binding.operation_id, &"d".repeat(64))
            .unwrap_err(),
        Failure::Conflict
    );
    owner
        .activate_after_config_readback(binding.operation_id, &target)
        .unwrap();
    drop(owner);
    assert!(access_token(temp.path(), space, id, true).is_ok());
    let owner = GrantOwner::open(temp.path(), id).unwrap();
    assert!(matches!(
        owner.begin_exchange(binding),
        Err(Failure::Conflict)
    ));
}

#[test]
fn local_disable_and_unconfirmed_retirement_are_distinct_from_revoked() {
    let temp = Temp::new();
    let space = Uuid::new_v4();
    for confirmed in [false, true] {
        let id = Uuid::new_v4();
        let operation = Uuid::new_v4();
        legacy(temp.path(), space, id);
        let owner = GrantOwner::open(temp.path(), id).unwrap();
        owner.disable_local(space).unwrap();
        assert_eq!(owner.status().unwrap().phase, Phase::LocalDisabled);
        let mut revoke = owner.begin_revocation(space, operation).unwrap();
        let token = revoke.release_refresh_token().unwrap();
        assert!(matches!(
            GrantOwner::open(temp.path(), id),
            Err(Failure::Conflict)
        ));
        assert_eq!(token, "synthetic-refresh-token");
        assert_eq!(
            revoke.release_refresh_token().unwrap_err(),
            Failure::Conflict
        );
        if confirmed {
            revoke.confirm().unwrap();
        } else {
            drop(revoke);
            let owner = GrantOwner::open(temp.path(), id).unwrap();
            assert_eq!(owner.status().unwrap().phase, Phase::RevocationPending);
            assert!(matches!(
                owner.begin_revocation(space, Uuid::new_v4()),
                Err(Failure::Conflict)
            ));
            let owner = GrantOwner::open(temp.path(), id).unwrap();
            owner.retire_unconfirmed(space).unwrap();
        }
        let owner = GrantOwner::open(temp.path(), id).unwrap();
        let status = owner.status().unwrap();
        assert_eq!(
            status.phase,
            if confirmed {
                Phase::Revoked
            } else {
                Phase::RetiredUnconfirmed
            }
        );
        assert_eq!(status.operation_id, Some(operation));
        assert!(!fs::read_to_string(grant_path(temp.path(), id))
            .unwrap()
            .contains("synthetic-refresh-token"));
        assert_eq!(
            owner.access_token(space, false).unwrap_err(),
            Failure::Authentication
        );
    }
}
