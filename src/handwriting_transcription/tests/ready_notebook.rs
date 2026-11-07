//! Real prepared-store mechanics; synthetic engine replies are not recognition.
use super::*;
use crate::storage::sync::mode::SyncModeChoice;
use std::fs;

fn generation(local: &Local) -> PathBuf {
    let id: Uuid =
        serde_json::from_slice(&fs::read(local.config.paths.data.join("CURRENT")).unwrap())
            .unwrap();
    local
        .config
        .paths
        .data
        .join("generations")
        .join(id.to_string())
}

fn correction_command(local: &Local, value: Value) -> handwriting::Report {
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    LocalCorrections::new(store, &local.config)
        .unwrap()
        .execute(handwriting::parse(&mut Cursor::new(serde_json::to_vec(&value).unwrap())).unwrap())
}

fn key(command: &str, correction: Uuid) -> Value {
    json!({"handwriting_version":1,"command":command,"profile":profile(),"correction":correction})
}

#[test]
fn ready4_literal_identity_vectors_preserve_all_existing_policies() {
    for explicit in [false, true] {
        exercise_literal_vectors(&Local::ready_notebook(explicit));
    }
}

#[test]
fn prepared_confirm_reopen_query_inspect_and_selected_capture_preserve_bytes() {
    for explicit in [false, true] {
        let local = Local::ready_notebook(explicit);
        let correction = Uuid::new_v4();
        let (record, revision) =
            local.confirm(correction, &png(), "PRIVATE_ORIGINAL", "PRIVATE_CORRECTED");
        assert_eq!(
            fs::read(generation(&local).join("format.json")).unwrap(),
            b"4"
        );
        let query = correction_command(
            &local,
            json!({"handwriting_version":1,"command":"query","profile":profile(),"query_text":null,"max_matches":16,"max_encoded_bytes":65536}),
        );
        assert_eq!(query.status, handwriting::Status::Complete);
        assert_eq!(query.facts[0].revision, revision);
        drop(query);
        let inspected = correction_command(&local, key("inspect", correction));
        let fact = &inspected.facts[0];
        assert_eq!(fact.record, record);
        assert_eq!(fact.revision, revision);
        assert_eq!(
            fact.value.as_ref().unwrap().original_text,
            "PRIVATE_ORIGINAL"
        );
        assert_eq!(
            fact.value.as_ref().unwrap().corrected_text,
            "PRIVATE_CORRECTED"
        );
        assert_eq!(fact.value.as_ref().unwrap().sample.sha256, digest(&png()));
        drop(inspected);
        let before = tree_bytes(&local.root);
        for version in [1, 2, 3] {
            let input = match version {
                1 => request(context(correction, revision)),
                2 => partial_request(context(correction, revision)),
                _ => structured_request(context(correction, revision)),
            };
            let mut engine = Engine::new();
            if version == 3 {
                engine.reply = structured_reply();
            }
            let report = run(&local, &input, &mut engine);
            assert!(report.succeeded());
            assert_eq!(engine.calls, 1);
            assert_eq!(report.provider_attempt, Attempt::Called);
            let captured = &report._owner.as_ref().unwrap();
            assert_eq!(captured.examples[0].png, png());
            assert_eq!(captured.examples[0].corrected_text, "PRIVATE_CORRECTED");
            assert_eq!(captured.identity.examples[0].revision, revision);
            assert_eq!(
                output(&report)["context"]["examples"][0]["record"],
                json!(record)
            );
            assert!(!report.confirmed);
            assert_eq!(
                engine.captured.last().unwrap()["image_url"],
                input["target_png"]
            );
            assert!(engine.captured.iter().any(|part| part["text"]
                .as_str()
                .is_some_and(|s| s.contains("PRIVATE_CORRECTED"))));
            drop(report);
            assert_eq!(tree_bytes(&local.root), before);
        }
        assert!(Store::open_existing(local.config.paths.clone()).is_err());
        let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
        assert!(store.legacy_publisher_lease().is_err());
        assert!(store.legacy_sync_lease().is_err());
    }
}

#[test]
fn prepared_correction_update_cas_forget_status_and_reopen_remain_coherent() {
    for explicit in [false, true] {
        let local = Local::ready_notebook(explicit);
        let correction = Uuid::new_v4();
        let (_, first) = local.confirm(correction, &png(), "before", "after");
        let op = Uuid::new_v4();
        let update = json!({"handwriting_version":1,"command":"update","profile":profile(),"correction":correction,"operation":op,"expected_heads":[first],"confirmation":"user-confirmed","original_text":"before","corrected_text":"after update","confirmed_ms":2,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic test","source":null},"sample_png_base64":STANDARD.encode(png())});
        let changed = correction_command(&local, update.clone());
        assert_eq!(changed.status, handwriting::Status::Complete);
        let revision = changed.facts[0].revision;
        drop(changed);
        let mut stale = update.clone();
        stale["operation"] = json!(Uuid::new_v4());
        let stale = correction_command(&local, stale);
        assert_ne!(stale.status, handwriting::Status::Complete);
        drop(stale);
        let report = run(
            &local,
            &request(context(correction, first)),
            &mut Engine::new(),
        );
        assert_eq!(report.failure, Some(Failure::ContextStale));
        drop(report);
        let mut status = key("status", correction);
        status["operation"] = json!(op);
        let historical = correction_command(&local, status);
        assert!(historical.historical);
        assert_eq!(historical.facts[0].revision, revision);
        drop(historical);
        let mut forget = key("forget", correction);
        forget["operation"] = json!(Uuid::new_v4());
        forget["expected_heads"] = json!([revision]);
        let removed = correction_command(&local, forget);
        assert_eq!(removed.status, handwriting::Status::Complete);
        assert_eq!(removed.facts[0].state, handwriting::FactState::Tombstone);
        drop(removed);
        let mut engine = Engine::new();
        let report = run(&local, &request(context(correction, revision)), &mut engine);
        assert!(matches!(
            report.failure,
            Some(Failure::ContextStale | Failure::ContextUnavailable)
        ));
        assert_eq!(engine.calls, 0);
        assert!(report.context.is_none());
        drop(report);
    }
}

#[test]
fn prepared_readonly_admission_refusals_leave_capabilities_staging_and_tree_unchanged() {
    for case in 0..17 {
        let mut local = Local::new();
        let correction = Uuid::new_v4();
        let (_, revision) = local.confirm(correction, &png(), "a", "b");
        let nonlocal = if case == 12 || case == 13 {
            let store = Store::open(local.config.paths.clone()).unwrap();
            let choice = if case == 12 {
                SyncModeChoice::IsolatedBackup {
                    binding_sha256: digest(b"fixture"),
                }
            } else {
                SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: digest(b"fixture"),
                    root_id: "fixture".into(),
                }
            };
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    choice,
                )
                .unwrap();
            let bytes = fs::read(local.config.paths.data.join("sync/mode.json")).unwrap();
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    SyncModeChoice::LocalOnly,
                )
                .unwrap();
            Some(bytes)
        } else {
            None
        };
        drop(Store::prepare_notebook(local.config.paths.clone()).unwrap());
        if let Some(bytes) = nonlocal {
            fs::write(local.config.paths.data.join("sync/mode.json"), bytes).unwrap();
        }
        let g = generation(&local);
        fs::write(
            local.config.paths.data.join(".stage-preserve"),
            b"retained staging sentinel",
        )
        .unwrap();
        match case {
            0 => fs::remove_file(local.config.paths.data.join("capabilities.json")).unwrap(),
            1 => fs::write(local.config.paths.data.join("contract-v3.json"), b"wrong").unwrap(),
            2..=4 => {
                let p = g.join("notebook-feature.json");
                let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                v[match case {
                    2 => "actor",
                    3 => "generation",
                    _ => "format",
                }] = if case == 4 {
                    json!(2)
                } else {
                    json!(Uuid::new_v4())
                };
                fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
            }
            5 => fs::remove_file(g.join("notebook-feature.json")).unwrap(),
            6 => fs::write(g.join("format.json"), b"1").unwrap(),
            7 | 8 => {
                let name = if case == 7 {
                    "selections"
                } else {
                    "selection-history"
                };
                fs::create_dir_all(g.join(name)).unwrap();
                fs::write(g.join(name).join("residue"), b"private").unwrap();
            }
            9 => fs::write(g.join("commits/bad.json"), b"invalid commit").unwrap(),
            10 => {
                let p = fs::read_dir(g.join("objects"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::write(p, b"corrupt retained object").unwrap();
            }
            11 => {
                let p = fs::read_dir(g.join("commits"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                v["scope"] = json!("selected-records");
                fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                let store = Store::open(local.config.paths.clone()).unwrap();
                assert_eq!(store.unavailable_commits().unwrap(), 0);
                assert!(store
                    .manifests()
                    .unwrap()
                    .iter()
                    .any(|manifest| manifest.scope == crate::storage::Scope::SelectedRecords));
            }
            12 | 13 => {}
            14 => local.config.sync.enabled = true,
            15 => fs::write(
                local.config.paths.data.join("CURRENT"),
                serde_json::to_vec(&Uuid::new_v4()).unwrap(),
            )
            .unwrap(),
            _ => fs::write(g.join("selection-history"), b"not a directory").unwrap(),
        }
        fs::write(
            local.config.paths.data.join(".stage-preserve"),
            b"retained staging sentinel",
        )
        .unwrap();
        let before = tree_bytes(&local.root);
        let mut engine = Engine::new();
        let report = run(&local, &request(context(correction, revision)), &mut engine);
        assert_eq!(
            report.failure,
            Some(if case == 14 {
                Failure::UnsupportedMode
            } else {
                Failure::StorageUnavailable
            }),
            "case {case}"
        );
        assert_eq!(report.provider_attempt, Attempt::NotCalled);
        assert_eq!(engine.calls, 0);
        assert!(report.context.is_none());
        assert!(!report.confirmed);
        drop(report);
        assert_eq!(before, tree_bytes(&local.root), "case {case}");
    }
}

#[test]
fn prepared_success_capture_refusal_cancellation_and_output_keep_real_ownership() {
    for explicit in [false, true] {
        for case in 0..5 {
            let local = Local::ready_notebook(explicit);
            let correction = Uuid::new_v4();
            let mut sample = png();
            if case == 3 {
                sample.extend_from_slice(b"trailing");
            }
            let (_, revision) = local.confirm(correction, &sample, "a", "b");
            let input = request(context(
                correction,
                if case == 1 { Uuid::new_v4() } else { revision },
            ));
            let mut engine = Engine::new();
            let report = if case == 2 || case == 4 {
                let mut prepared =
                    Prepared::admit(&serde_json::to_vec(&input).unwrap(), &mut || Ok(()));
                prepared.capture(&local.config, &mut || Ok(()));
                assert!(prepared.report.context.is_some());
                let mut checkpoints = 0;
                let report = prepared.execute("m", &mut engine, &mut || {
                    checkpoints += 1;
                    if checkpoints == if case == 2 { 1 } else { 3 } {
                        anyhow::bail!("cancel");
                    }
                    Ok(())
                });
                assert_eq!(report.failure, Some(Failure::Canceled));
                assert_eq!(engine.calls, if case == 2 { 0 } else { 1 });
                report
            } else {
                run(&local, &input, &mut engine)
            };
            assert!(
                report._owner.is_some() || report._refused_owner.is_some(),
                "case {case}"
            );
            assert!(Store::open(local.config.paths.clone()).is_err());
            struct Observed<'a> {
                local: &'a Local,
                bytes: Vec<u8>,
                flushed: bool,
            }
            impl Write for Observed<'_> {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    assert!(Store::open(self.local.config.paths.clone()).is_err());
                    let n = bytes.len().min(11);
                    self.bytes.extend_from_slice(&bytes[..n]);
                    Ok(n)
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    assert!(Store::open(self.local.config.paths.clone()).is_err());
                    self.flushed = true;
                    Ok(())
                }
            }
            let mut observed = Observed {
                local: &local,
                bytes: vec![],
                flushed: false,
            };
            report.write_to(&mut observed).unwrap();
            assert!(observed.flushed);
            let emitted: Value = serde_json::from_slice(&observed.bytes).unwrap();
            assert_eq!(emitted["confirmed"], false);
            assert_eq!(engine.calls, if case == 0 || case == 4 { 1 } else { 0 });
            struct Broken;
            impl Write for Broken {
                fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                    Err(std::io::Error::other("blocked"))
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    panic!("no flush on failed write")
                }
            }
            assert!(report.write_to(&mut Broken).is_err());
            assert!(Store::open(local.config.paths.clone()).is_err());
            drop(report);
            let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
            let mode = store.sync_mode_snapshot().unwrap();
            assert!(store.ordinary_mode_lease(&mode.token).is_ok());
            assert!(store.legacy_publisher_lease().is_err());
            assert!(store.legacy_sync_lease().is_err());
        }
    }
}

#[test]
fn null_context_bypasses_even_an_owned_ready_notebook() {
    let local = Local::ready_notebook(false);
    let before = tree_bytes(&local.root);
    let owner = Store::open(local.config.paths.clone()).unwrap();
    let mut engine = Engine::new();
    let report = run(&local, &request(Value::Null), &mut engine);
    assert_eq!(report.status, Status::Normalized);
    assert_eq!(engine.calls, 1);
    assert!(report._owner.is_none() && report._refused_owner.is_none() && report.context.is_none());
    drop(report);
    drop(owner);
    assert_eq!(tree_bytes(&local.root), before);
}

#[test]
fn legacy_selected_and_feedback_families_keep_ordinary_context_behavior() {
    for feedback in [false, true] {
        let local = Local::new();
        let mut store = Store::open(local.config.paths.clone()).unwrap();
        let g = generation(&local);
        let id: Uuid =
            serde_json::from_slice(&fs::read(local.config.paths.data.join("CURRENT")).unwrap())
                .unwrap();
        crate::storage::selection::prepare_generation(&g, store.actor_id, id).unwrap();
        if feedback {
            crate::storage::feedback_generation::prepare(&mut store, |_| Ok(())).unwrap();
        }
        drop(store);
        let correction = Uuid::new_v4();
        let (_, revision) = local.confirm(correction, &png(), "legacy", "confirmed");
        let before = tree_bytes(&local.root);
        let mut engine = Engine::new();
        let report = run(&local, &request(context(correction, revision)), &mut engine);
        assert_eq!(report.status, Status::Normalized);
        assert_eq!(engine.calls, 1);
        drop(report);
        assert_eq!(before, tree_bytes(&local.root));
        assert_eq!(
            fs::read(g.join("format.json")).unwrap(),
            if feedback { b"3" } else { b"2" }
        );
    }
}

#[test]
fn prepared_os_contention_is_busy_without_call_or_changed_bytes() {
    let local = Local::ready_notebook(false);
    let correction = Uuid::new_v4();
    let (_, revision) = local.confirm(correction, &png(), "a", "b");
    let before = tree_bytes(&local.root);
    let owner = Store::open(local.config.paths.clone()).unwrap();
    let mut engine = Engine::new();
    let report = run(&local, &request(context(correction, revision)), &mut engine);
    assert_eq!(report.failure, Some(Failure::OwnershipBusy));
    assert_eq!(engine.calls, 0);
    assert!(report.context.is_none());
    drop(report);
    drop(owner);
    assert_eq!(before, tree_bytes(&local.root));
}
