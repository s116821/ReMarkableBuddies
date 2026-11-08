use super::*;
use crate::conversation::target_source::tests::Fixture;

fn empty_store(root: &Path, actor: Uuid, targets: bool) -> Store {
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    // Synthetic same-actor recovery destinations model a retained backup identity.
    // No source live owner, mode or preparation files are transferred.
    files::directory(&paths.data).unwrap();
    files::create_identity(
        &paths.data.join("identity.json"),
        &Identity {
            format: FORMAT,
            actor,
        },
    )
    .unwrap();
    if targets {
        Store::open_source_target_startup(&crate::config::Config {
            paths,
            ..Default::default()
        })
        .unwrap()
    } else {
        Store::prepare_notebook(paths).unwrap()
    }
}

fn check_empty_restore(archive_targets: bool, base_targets: bool, conditional: bool) {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("target-empty-restore-{}", Uuid::new_v4())));
    let actor = Uuid::new_v4();
    let source = empty_store(&fixture.0.join("source"), actor, archive_targets);
    assert!(source.inner.lock().unwrap().index.records.is_empty());
    let archive = fixture.0.join("archive");
    assert_eq!(
        source.export_notebook_backup(&archive).unwrap().format,
        if archive_targets { 6 } else { 4 }
    );
    drop(source);
    let base = empty_store(&fixture.0.join("base"), actor, base_targets);
    assert!(base.inner.lock().unwrap().index.records.is_empty());
    let paths = base.paths.clone();
    if conditional {
        let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
        let result = base
            .apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
            .unwrap();
        assert!(
            matches!(result, ordinary_restore::Publication::Verified(_)),
            "{result:?}"
        );
    } else {
        base.restore_notebook_backup(&archive).unwrap();
    }
    let generation = base.inner.lock().unwrap().generation;
    let expected = if archive_targets || base_targets {
        6
    } else {
        4
    };
    assert_eq!(
        feedback_generation::version(&base.generation(generation)).unwrap(),
        expected,
        "an empty history still retains the archive/base capability and provenance"
    );
    assert!(base.inner.lock().unwrap().index.records.is_empty());
    if expected == 6 {
        target_generation::validate_ready(&paths, actor, generation).unwrap();
        let marker: serde_json::Value = files::json(
            &base.generation(generation).join(target_generation::MARKER),
            MAX_RECORD as u64,
        )
        .unwrap();
        assert_eq!(marker["actor"], actor.to_string());
        assert_eq!(marker["generation"], generation.to_string());
    }
    drop(base);
    let reopened = Store::open(paths).unwrap();
    assert_eq!(
        feedback_generation::version(&reopened.generation(generation)).unwrap(),
        expected
    );
}

#[test]
fn target_empty_restore_conditional_legacy_control() {
    check_empty_restore(false, false, true);
}
#[test]
fn target_empty_restore_conditional_target_archive_into_notebook_base() {
    check_empty_restore(true, false, true);
}
#[test]
fn target_empty_restore_conditional_notebook_archive_into_target_base() {
    check_empty_restore(false, true, true);
}
#[test]
fn target_empty_restore_conditional_target_archive_into_target_base() {
    check_empty_restore(true, true, true);
}
#[test]
fn target_empty_restore_public_target_archive_into_notebook_base() {
    check_empty_restore(true, false, false);
}
#[test]
fn target_empty_restore_public_notebook_archive_into_target_base() {
    check_empty_restore(false, true, false);
}
#[test]
fn target_empty_restore_public_target_archive_into_target_base() {
    check_empty_restore(true, true, false);
}

#[test]
fn target_restore_plan_binds_empty_base_family_and_preserves_capability() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("target-restore-plan-{}", Uuid::new_v4())));
    let actor = Uuid::new_v4();
    let source = empty_store(&fixture.0.join("source"), actor, false);
    let archive = fixture.0.join("archive");
    source.export_notebook_backup(&archive).unwrap();
    drop(source);
    let base = empty_store(&fixture.0.join("base"), actor, false);
    let paths = base.paths.clone();
    let old = base
        .prepare_notebook_restore(&archive, || Ok(()))
        .unwrap()
        .conditions;
    let generation = base.inner.lock().unwrap().generation;
    drop(base);
    let base = Store::open_source_target_startup(&crate::config::Config {
        paths: paths.clone(),
        ..Default::default()
    })
    .unwrap();
    let new = base
        .prepare_notebook_restore(&archive, || Ok(()))
        .unwrap()
        .conditions;
    assert_eq!(base.inner.lock().unwrap().generation, generation);
    assert_ne!(old.base_history_sha256, new.base_history_sha256);
    assert!(base
        .apply_notebook_restore(&archive, &old, || Ok(()))
        .is_err());
    assert!(!base.generation(old.target_generation).exists());
    base.restore_notebook_backup(&archive).unwrap();
    let restored = files::json::<Uuid>(&paths.data.join("CURRENT"), 128).unwrap();
    assert_ne!(restored, generation);
    target_generation::validate_ready(&paths, actor, restored).unwrap();
}

#[test]
fn target_restore_plan_previews_real_domain_four_archive_and_rejects_changed_origin() {
    let (fixture, ledger, _, _) = crate::conversation::target_source::tests::fixture();
    let source = ledger.store().clone();
    drop(ledger);
    let archive = fixture.0.join("archive");
    source.export_notebook_backup(&archive).unwrap();
    let base = empty_store(&fixture.0.join("base"), source.actor_id, false);
    let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
    assert!(plan.preview.target.manifests > 0 && plan.preview.target.complete_media);
    let before = files::read(&base.paths.data.join("CURRENT"), 128).unwrap();
    let marker = archive.join("backup.json");
    let mut value: serde_json::Value = files::json(&marker, MAX_METADATA as u64).unwrap();
    value["target_feature"]["feature"]["operation"] = serde_json::json!(Uuid::new_v4());
    files::atomic_json(&marker, &value).unwrap();
    // The changed closed header is valid in isolation; the old plan must still
    // reject its different origin instead of silently refreshing comparison.
    Store::inspect_notebook_backup(&archive).unwrap();
    assert!(base
        .apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
        .is_err());
    assert_eq!(
        files::read(&base.paths.data.join("CURRENT"), 128).unwrap(),
        before
    );
    assert!(!base.generation(plan.conditions.target_generation).exists());
}

#[test]
fn target_restore_plan_binds_empty_base_feature_provenance() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("target-restore-origin-{}", Uuid::new_v4())));
    let actor = Uuid::new_v4();
    let source = empty_store(&fixture.0.join("source"), actor, false);
    let archive = fixture.0.join("archive");
    source.export_notebook_backup(&archive).unwrap();
    drop(source);
    let base = empty_store(&fixture.0.join("base"), actor, true);
    let old = base
        .prepare_notebook_restore(&archive, || Ok(()))
        .unwrap()
        .conditions;
    let generation = base.inner.lock().unwrap().generation;
    let root = base.generation(generation);
    let operation = Uuid::new_v4();
    for (name, nested) in [
        (target_generation::MARKER, false),
        ("source-target-preparation.json", true),
    ] {
        let path = root.join(name);
        let mut value: serde_json::Value = files::json(&path, MAX_RECORD as u64).unwrap();
        if nested {
            value["feature"]["operation"] = serde_json::json!(operation);
        } else {
            value["operation"] = serde_json::json!(operation);
        }
        files::atomic_json(&path, &value).unwrap();
    }
    target_generation::validate_ready(&base.paths, actor, generation).unwrap();
    let new = base
        .prepare_notebook_restore(&archive, || Ok(()))
        .unwrap()
        .conditions;
    assert_ne!(old.base_history_sha256, new.base_history_sha256);
    assert!(base
        .apply_notebook_restore(&archive, &old, || Ok(()))
        .is_err());
    assert!(!base.generation(old.target_generation).exists());
    assert_eq!(
        files::json::<Uuid>(&base.paths.data.join("CURRENT"), 128).unwrap(),
        generation
    );
}

#[test]
fn target_restore_original_recovers_frozen_closure_and_budget_across_activation_boundaries() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
        Fault::BeforeActivation,
        Fault::AfterCommit,
        Fault::AfterActivation,
    ] {
        let (fixture, ledger, conversation, _) =
            crate::conversation::target_source::tests::fixture();
        let source = ledger.store().clone();
        let original_records =
            serde_json::to_value(ledger.inspect(conversation, false).unwrap()).unwrap();
        drop(ledger);
        let archive = fixture.0.join("archive");
        source.export_notebook_backup(&archive).unwrap();
        let base = empty_store(&fixture.0.join("base"), source.actor_id, false);
        let paths = base.paths.clone();
        let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
        base.set_fault(fault).unwrap();
        let result = base
            .apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
            .unwrap();
        assert!(!matches!(
            result,
            ordinary_restore::Publication::Verified(_)
        ));
        let target = base.generation(plan.conditions.target_generation);
        let before: serde_json::Value =
            files::json(&target.join(target_restore::ORIGINAL), MAX_RECORD as u64).unwrap();
        drop(base);
        if fault == Fault::AfterCommit {
            fs::rename(&archive, fixture.0.join("archive-retired")).unwrap();
        }
        let config = crate::config::Config {
            paths: paths.clone(),
            ..Default::default()
        };
        let restored = Store::open_source_target_startup(&config).unwrap();
        assert_eq!(
            restored.inner.lock().unwrap().generation,
            plan.conditions.target_generation
        );
        target_generation::validate_ready(
            &paths,
            source.actor_id,
            plan.conditions.target_generation,
        )
        .unwrap();
        let after: serde_json::Value =
            files::json(&target.join(target_restore::ORIGINAL), MAX_RECORD as u64).unwrap();
        assert_eq!(
            after["original"], before["original"],
            "recovery must preserve every original condition"
        );
        if fault != Fault::AfterActivation {
            assert!(
                after["remaining"]["usage"]["work_items"].as_u64().unwrap()
                    > before["remaining"]["usage"]["work_items"].as_u64().unwrap()
            );
        } else {
            assert_eq!(
                after, before,
                "completed lost acknowledgment must remain validate-only"
            );
        }
        let restored = std::sync::Arc::new(restored);
        let ledger = crate::conversation::Ledger::new(restored.clone());
        assert_eq!(
            serde_json::to_value(ledger.inspect(conversation, false).unwrap()).unwrap(),
            original_records
        );
        assert!(restored
            .observe_notebook_restore(&plan.conditions, || Ok(()))
            .unwrap()
            .is_some());
        assert!(restored
            .apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
            .is_err());
    }
}

#[test]
fn target_restore_original_refuses_changed_staged_history_or_configuration() {
    for changed in ["history", "configuration"] {
        let (fixture, ledger, _, _) = crate::conversation::target_source::tests::fixture();
        let source = ledger.store().clone();
        drop(ledger);
        let archive = fixture.0.join("archive");
        source.export_notebook_backup(&archive).unwrap();
        let base = empty_store(&fixture.0.join("base"), source.actor_id, false);
        let paths = base.paths.clone();
        let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
        base.set_fault(Fault::BeforeActivation).unwrap();
        assert!(matches!(
            base.apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
                .unwrap(),
            ordinary_restore::Publication::NotActivated
        ));
        let current = files::read(&paths.data.join("CURRENT"), 128).unwrap();
        let target = base.generation(plan.conditions.target_generation);
        let original: serde_json::Value =
            files::json(&target.join(target_restore::ORIGINAL), MAX_RECORD as u64).unwrap();
        let path = target.join(if changed == "history" {
            "target-restore-history.json"
        } else {
            "restored-config.json"
        });
        files::atomic(&path, b"{}").unwrap();
        drop(base);
        let config = crate::config::Config {
            paths: paths.clone(),
            ..Default::default()
        };
        assert!(
            Store::open_source_target_startup(&config).is_err(),
            "{changed}"
        );
        assert_eq!(
            files::read(&paths.data.join("CURRENT"), 128).unwrap(),
            current
        );
        let after: serde_json::Value =
            files::json(&target.join(target_restore::ORIGINAL), MAX_RECORD as u64).unwrap();
        assert_eq!(after["original"], original["original"]);
        assert!(
            after["remaining"]["usage"]["work_items"].as_u64().unwrap()
                > original["remaining"]["usage"]["work_items"]
                    .as_u64()
                    .unwrap()
        );
    }
}
