use super::*;
use crate::conversation::target_source::tests::Fixture;

fn identity_paths(root: &Path, actor: Uuid) -> StorePaths {
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
    paths
}
fn empty_store(root: &Path, actor: Uuid, targets: bool) -> Store {
    let paths = identity_paths(root, actor);
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

#[test]
fn target_restore_combines_populated_target_history_with_newer_subject_revision() {
    use crate::conversation::{ContextBudget, Ledger, Record, TargetChange};
    let (fixture, ledger, conversation, _) = crate::conversation::target_source::tests::fixture();
    let store = ledger.store().clone();
    let budget = ContextBudget {
        max_text_bytes: 8192,
        max_turns: 16,
        provider_token_limit: Some(8192),
    };
    let target = ledger
        .context(conversation, &budget, |_| Ok(1))
        .unwrap()
        .targets[0]
        .clone();
    let archive = fixture.0.join("archive");
    store.export_notebook_backup(&archive).unwrap();
    ledger
        .change_target(
            Uuid::new_v4(),
            ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            crate::conversation::target_source::tests::user(conversation),
            &TargetChange::CorrectSubject {
                target: target.reference.clone(),
                subject: "Newer base subject".into(),
            },
        )
        .unwrap();
    let before = serde_json::to_value(ledger.inspect(conversation, false).unwrap()).unwrap();
    let mut objects = BTreeMap::new();
    let root = store.generation(store.inner.lock().unwrap().generation);
    for entry in fs::read_dir(root.join("objects")).unwrap() {
        let path = entry.unwrap().path();
        objects.insert(
            path.file_name().unwrap().to_owned(),
            files::read(&path, MAX_MEDIA).unwrap(),
        );
    }
    drop(ledger);
    let plan = store.prepare_notebook_restore(&archive, || Ok(())).unwrap();
    assert!(matches!(
        store
            .apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
            .unwrap(),
        ordinary_restore::Publication::Verified(_)
    ));
    let restored_root = store.generation(plan.conditions.target_generation);
    for (name, bytes) in objects {
        assert_eq!(
            files::read(&restored_root.join("objects").join(name), MAX_MEDIA).unwrap(),
            bytes
        );
    }
    let ledger = Ledger::new(store);
    let records = ledger.inspect(conversation, false).unwrap();
    assert_eq!(serde_json::to_value(&records).unwrap(), before);
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, Record::SourceTarget(_)))
            .count(),
        1
    );
    let context = ledger.context(conversation, &budget, |_| Ok(1)).unwrap();
    assert_eq!(context.targets.len(), 2);
    assert!(context
        .targets
        .iter()
        .any(|view| view.reference.revision == target.reference.revision));
    assert!(context
        .targets
        .iter()
        .any(|view| view.target.candidate.subject == "Newer base subject"));
}

#[test]
fn target_restore_nonempty_additive_matrix_preserves_both_histories_and_exact_objects() {
    use crate::conversation::{target_source::tests::populate, Ledger};
    use std::sync::Arc;
    for archive_targets in [false, true] {
        for base_targets in [false, true] {
            for conditional in [false, true] {
                let fixture =
                    Fixture(std::env::temp_dir().join(format!("target-matrix-{}", Uuid::new_v4())));
                let actor = Uuid::new_v4();
                let source = Arc::new(empty_store(
                    &fixture.0.join("source"),
                    actor,
                    archive_targets,
                ));
                let source_conversation = if archive_targets {
                    populate(&Ledger::new(source.clone())).0
                } else {
                    compound_generation::tests::produce_notebook_with_conversation(source.clone()).0
                };
                let archive = fixture.0.join("archive");
                source.export_notebook_backup(&archive).unwrap();
                let source_view = serde_json::to_value(
                    Ledger::new(source.clone())
                        .inspect(source_conversation, false)
                        .unwrap(),
                )
                .unwrap();
                let base = Arc::new(empty_store(&fixture.0.join("base"), actor, base_targets));
                let base_conversation = if base_targets {
                    populate(&Ledger::new(base.clone())).0
                } else {
                    compound_generation::tests::produce_notebook_with_conversation(base.clone()).0
                };
                let base_view = serde_json::to_value(
                    Ledger::new(base.clone())
                        .inspect(base_conversation, false)
                        .unwrap(),
                )
                .unwrap();
                let mut objects = BTreeMap::new();
                for store in [&source, &base] {
                    let root = store.generation(store.inner.lock().unwrap().generation);
                    for entry in fs::read_dir(root.join("objects")).unwrap() {
                        let path = entry.unwrap().path();
                        objects.insert(
                            path.file_name().unwrap().to_owned(),
                            files::read(&path, MAX_MEDIA).unwrap(),
                        );
                    }
                }
                if conditional {
                    let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
                    assert!(matches!(
                        base.apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
                            .unwrap(),
                        ordinary_restore::Publication::Verified(_)
                    ));
                } else {
                    base.restore_notebook_backup(&archive).unwrap();
                }
                let generation = base.inner.lock().unwrap().generation;
                let family = if archive_targets || base_targets {
                    6
                } else {
                    4
                };
                assert_eq!(
                    feedback_generation::version(&base.generation(generation)).unwrap(),
                    family
                );
                for (name, bytes) in objects {
                    assert_eq!(
                        files::read(
                            &base.generation(generation).join("objects").join(name),
                            MAX_MEDIA
                        )
                        .unwrap(),
                        bytes
                    );
                }
                let paths = base.paths.clone();
                drop(base);
                let reopened = Arc::new(Store::open(paths).unwrap());
                let ledger = Ledger::new(reopened);
                assert_eq!(
                    serde_json::to_value(ledger.inspect(source_conversation, false).unwrap())
                        .unwrap(),
                    source_view
                );
                assert_eq!(
                    serde_json::to_value(ledger.inspect(base_conversation, false).unwrap())
                        .unwrap(),
                    base_view
                );
            }
        }
    }
}

#[test]
fn target_restore_original_recovers_header_and_capability_publication_boundaries() {
    use target_generation::Stage;
    for stage in [
        Stage::Original,
        Stage::Marker,
        Stage::Contract,
        Stage::Format,
        Stage::Capabilities,
    ] {
        let (fixture, ledger, conversation, _) =
            crate::conversation::target_source::tests::fixture();
        let source = ledger.store().clone();
        let expected = serde_json::to_value(ledger.inspect(conversation, false).unwrap()).unwrap();
        drop(ledger);
        let archive = fixture.0.join("archive");
        source.export_notebook_backup(&archive).unwrap();
        let base = empty_store(&fixture.0.join("base"), source.actor_id, false);
        let paths = base.paths.clone();
        let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
        let interruption = target_restore::interrupt(stage);
        let result = base
            .apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
            .unwrap();
        drop(interruption);
        assert!(matches!(
            result,
            ordinary_restore::Publication::Unknown | ordinary_restore::Publication::NotActivated
        ));
        let root = base.generation(plan.conditions.target_generation);
        let before: serde_json::Value =
            files::json(&root.join(target_restore::ORIGINAL), MAX_RECORD as u64).unwrap();
        drop(base);
        if stage == Stage::Capabilities {
            fs::rename(&archive, fixture.0.join("archive-retired")).unwrap();
        }
        let reopened = Store::open_source_target_startup(&crate::config::Config {
            paths: paths.clone(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            reopened.inner.lock().unwrap().generation,
            plan.conditions.target_generation
        );
        let after: serde_json::Value =
            files::json(&root.join(target_restore::ORIGINAL), MAX_RECORD as u64).unwrap();
        assert_eq!(after["original"], before["original"]);
        assert!(
            after["remaining"]["usage"]["work_items"].as_u64().unwrap()
                > before["remaining"]["usage"]["work_items"].as_u64().unwrap()
        );
        let ledger = crate::conversation::Ledger::new(std::sync::Arc::new(reopened));
        assert_eq!(
            serde_json::to_value(ledger.inspect(conversation, false).unwrap()).unwrap(),
            expected
        );
        target_generation::validate_ready(
            &paths,
            source.actor_id,
            plan.conditions.target_generation,
        )
        .unwrap();
    }
}

#[test]
fn target_restore_exhaustion_stays_with_original_and_cannot_mint_a_new_plan() {
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
    let root = base.generation(plan.conditions.target_generation);
    let receipt_path = root.join(target_restore::ORIGINAL);
    let original: serde_json::Value = files::json(&receipt_path, MAX_RECORD as u64).unwrap();
    let current = files::read(&paths.data.join("CURRENT"), 128).unwrap();
    let history_path = root.join("target-restore-history.json");
    let valid = files::read(&history_path, (MAX_METADATA + MAX_ITEMS + 2) as u64).unwrap();
    files::atomic(&history_path, b"{}").unwrap();
    drop(base);
    let config = crate::config::Config {
        paths: paths.clone(),
        ..Default::default()
    };
    for _ in 0..2 {
        assert!(Store::open_source_target_startup(&config).is_err());
    }
    // Even repairing the test fault cannot replenish the original allowance.
    files::atomic(&history_path, &valid).unwrap();
    assert!(Store::open_source_target_startup(&config).is_err());
    let exhausted: serde_json::Value = files::json(&receipt_path, MAX_RECORD as u64).unwrap();
    assert_eq!(exhausted["original"], original["original"]);
    assert_eq!(exhausted["remaining"]["exhausted"], true);
    assert_eq!(
        exhausted["remaining"]["usage"]["work_items"]
            .as_u64()
            .unwrap(),
        original["original"]["work"].as_u64().unwrap() * 3
    );
    assert!(Store::open_source_target_startup(&config).is_err());
    assert_eq!(
        files::json::<serde_json::Value>(&receipt_path, MAX_RECORD as u64).unwrap(),
        exhausted
    );
    assert_eq!(
        files::read(&paths.data.join("CURRENT"), 128).unwrap(),
        current
    );
    let base = Store::open_existing_for_notebook_restore(paths).unwrap();
    let count = fs::read_dir(base.paths.data.join("generations"))
        .unwrap()
        .count();
    assert!(base.prepare_notebook_restore(&archive, || Ok(())).is_err());
    assert!(base.restore_notebook_backup(&archive).is_err());
    assert_eq!(
        fs::read_dir(base.paths.data.join("generations"))
            .unwrap()
            .count(),
        count
    );
}

#[test]
fn target_restore_refuses_foreign_actor_before_staging() {
    let (fixture, ledger, _, _) = crate::conversation::target_source::tests::fixture();
    let source = ledger.store().clone();
    drop(ledger);
    let archive = fixture.0.join("archive");
    source.export_notebook_backup(&archive).unwrap();
    for targets in [false, true] {
        let base = empty_store(
            &fixture
                .0
                .join(if targets { "foreign6" } else { "foreign4" }),
            Uuid::new_v4(),
            targets,
        );
        let current = files::read(&base.paths.data.join("CURRENT"), 128).unwrap();
        let count = fs::read_dir(base.paths.data.join("generations"))
            .unwrap()
            .count();
        assert!(base.prepare_notebook_restore(&archive, || Ok(())).is_err());
        assert!(base.restore_notebook_backup(&archive).is_err());
        assert_eq!(
            files::read(&base.paths.data.join("CURRENT"), 128).unwrap(),
            current
        );
        assert_eq!(
            fs::read_dir(base.paths.data.join("generations"))
                .unwrap()
                .count(),
            count
        );
    }
}

#[test]
fn target_restore_original_refuses_changed_actor_paths_or_valid_local_mode() {
    for changed in ["actor", "paths", "mode"] {
        let (fixture, ledger, _, _) = crate::conversation::target_source::tests::fixture();
        let source = ledger.store().clone();
        drop(ledger);
        let archive = fixture.0.join("archive");
        source.export_notebook_backup(&archive).unwrap();
        let base = empty_store(&fixture.0.join("base"), source.actor_id, false);
        let mut paths = base.paths.clone();
        let plan = base.prepare_notebook_restore(&archive, || Ok(())).unwrap();
        base.set_fault(Fault::BeforeActivation).unwrap();
        assert!(matches!(
            base.apply_notebook_restore(&archive, &plan.conditions, || Ok(()))
                .unwrap(),
            ordinary_restore::Publication::NotActivated
        ));
        let before: serde_json::Value = files::json(
            &base
                .generation(plan.conditions.target_generation)
                .join(target_restore::ORIGINAL),
            MAX_RECORD as u64,
        )
        .unwrap();
        let current = files::read(&paths.data.join("CURRENT"), 128).unwrap();
        drop(base);
        match changed {
            "actor" => {
                let identity = Identity {
                    format: FORMAT,
                    actor: Uuid::new_v4(),
                };
                for path in [
                    paths.data.join("identity.json"),
                    paths.cache.join("owner.json"),
                    paths.credentials.join("owner.json"),
                ] {
                    files::atomic_json(&path, &identity).unwrap();
                }
            }
            "paths" => {
                let relocated = fixture.0.join("relocated-base");
                fs::rename(fixture.0.join("base"), &relocated).unwrap();
                paths = StorePaths {
                    data: relocated.join("data"),
                    cache: relocated.join("cache"),
                    credentials: relocated.join("credentials"),
                };
            }
            "mode" => {
                let mode_source = Store::open(identity_paths(
                    &fixture.0.join("mode-source"),
                    source.actor_id,
                ))
                .unwrap();
                mode_source
                    .set_sync_mode(
                        &mode_source.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        sync::mode::SyncModeChoice::LocalOnly,
                    )
                    .unwrap();
                let bytes = files::read(
                    &mode_source.paths.data.join("sync/mode.json"),
                    MAX_METADATA as u64,
                )
                .unwrap();
                files::atomic(&paths.data.join("sync/mode.json"), &bytes).unwrap();
            }
            _ => unreachable!(),
        }
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
        let after: serde_json::Value = files::json(
            &paths
                .data
                .join("generations")
                .join(plan.conditions.target_generation.to_string())
                .join(target_restore::ORIGINAL),
            MAX_RECORD as u64,
        )
        .unwrap();
        assert_eq!(after["original"], before["original"]);
        if changed != "mode" {
            assert_eq!(after, before);
        } else {
            assert!(
                after["remaining"]["usage"]["work_items"].as_u64().unwrap()
                    > before["remaining"]["usage"]["work_items"].as_u64().unwrap()
            );
        }
    }
}

#[test]
fn target_restore_selected_families_cannot_enter_ordinary_restore_or_startup() {
    for targets in [false, true] {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("target-selected-restore-{}", Uuid::new_v4())),
        );
        let actor = Uuid::new_v4();
        let archive_source = empty_store(&fixture.0.join("source"), actor, true);
        let archive = fixture.0.join("archive");
        archive_source.export_notebook_backup(&archive).unwrap();
        let base = empty_store(&fixture.0.join("base"), actor, false);
        let paths = base.paths.clone();
        let token = serde_json::to_value(base.sync_mode_snapshot().unwrap().token).unwrap();
        let request = compound_generation::Request {
            operation: Uuid::new_v4(),
            actor,
            source_generation: base.inner.lock().unwrap().generation,
            group: Uuid::new_v4(),
            binding_sha256: digest(b"synthetic selected binding"),
            expected_mode: compound_generation::ModeEvidence {
                actor,
                sha256: token["sha256"].as_str().unwrap().into(),
            },
        };
        drop(base);
        let mut selected = Store::prepare_shared_notebook(paths.clone(), request.clone()).unwrap();
        selected
            .set_sync_mode(
                &selected.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                sync::mode::SyncModeChoice::SharedGroup {
                    group: request.group,
                    binding_sha256: request.binding_sha256,
                    root_id: "synthetic-root".into(),
                },
            )
            .unwrap();
        if targets {
            target_generation::prepare(
                &mut selected,
                &digest(b"selected fixture config"),
                target_generation::BaseKind::CompoundSelected,
                |_| Ok(()),
            )
            .unwrap();
        }
        let current = files::read(&paths.data.join("CURRENT"), 128).unwrap();
        let count = fs::read_dir(paths.data.join("generations"))
            .unwrap()
            .count();
        let destination = fixture.0.join("must-not-export");
        assert!(selected.export_notebook_backup(&destination).is_err());
        assert!(!destination.exists());
        assert!(selected
            .prepare_notebook_restore(&archive, || Ok(()))
            .is_err());
        assert!(selected.restore_notebook_backup(&archive).is_err());
        drop(selected);
        assert!(
            Store::open_source_target_startup(&crate::config::Config {
                paths: paths.clone(),
                ..Default::default()
            })
            .is_err(),
            "ordinary startup must refuse selected family{}",
            if targets { 6 } else { 5 }
        );
        assert_eq!(
            files::read(&paths.data.join("CURRENT"), 128).unwrap(),
            current
        );
        assert_eq!(
            fs::read_dir(paths.data.join("generations"))
                .unwrap()
                .count(),
            count
        );
        if targets {
            let mode =
                files::read(&paths.data.join("sync/mode.json"), MAX_METADATA as u64).unwrap();
            let ordinary = empty_store(&fixture.0.join("ordinary-mode-fault"), actor, true);
            let ordinary_paths = ordinary.paths.clone();
            let before = files::read(&ordinary_paths.data.join("CURRENT"), 128).unwrap();
            drop(ordinary);
            // Inject a valid same-actor SharedGroup mode as external interference;
            // ordinary provenance alone must not qualify this changed mode.
            files::atomic(&ordinary_paths.data.join("sync/mode.json"), &mode).unwrap();
            assert!(Store::open_source_target_startup(&crate::config::Config {
                paths: ordinary_paths.clone(),
                ..Default::default()
            })
            .is_err());
            assert_eq!(
                files::read(&ordinary_paths.data.join("CURRENT"), 128).unwrap(),
                before
            );
        }
    }
}
