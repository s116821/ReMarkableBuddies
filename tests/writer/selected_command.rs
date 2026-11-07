use super::*;
use std::io::Write;
fn call(config: &std::path::Path, op: &str, input: &[u8], key: bool) -> Value {
    use std::process::{Command, Stdio};
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command
        .args(["--conversation-operation", op, "--conversation-input", "-"])
        .env("REMARKABLE_BUDDIES_CONFIG", config)
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("MEM_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if key {
        command.env("OPENAI_API_KEY", "fixture-not-a-key");
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
struct Ready {
    local: Local,
    config: PathBuf,
    inspect: Value,
    request: Request,
}
fn ready(api: &Api, enrich: bool) -> Ready {
    let f = Fixture::new();
    if enrich {
        drop(f.memory(false));
    }
    let mode = f.store.sync_mode_snapshot().unwrap();
    f.store
        .set_sync_mode(
            &mode.token,
            Uuid::new_v4(),
            remarkable_reader_buddy::storage::sync::mode::SyncModeChoice::SharedGroup {
                group: f.identity.group,
                binding_sha256: f.identity.binding_sha256.clone(),
                root_id: "fixture-appdata".into(),
            },
        )
        .unwrap();
    let scope = f.selection.accepted.scope().clone();
    let conversation = f.req.conversation();
    let config = f.local.0.join("selected-config.json");
    std::fs::write(&config,serde_json::to_vec(&json!({"paths":{"data":f.local.0.join("data"),"cache":f.local.0.join("cache"),"credentials":f.local.0.join("credentials")},"model":"fixture-model","base_url":api.origin,"product_settings":{"schema_version":1,"reader_enabled":false,"writer_enabled":true,"export_destination":"disabled"},"reader":{"selection":{"group":Uuid::new_v4(),"key_sha256":"f".repeat(64),"binding_sha256":"b".repeat(64)}},"memory":{"enabled":enrich,"profile":f.identity.profile,"selection":enrich.then_some(f.identity.clone())}})).unwrap()).unwrap();
    let Fixture {
        local,
        store,
        writer,
        req,
        ..
    } = f;
    drop(writer);
    drop(store);
    Ready {
        local,
        config,
        inspect: json!({"wire_version":1,"scope":scope,"conversation":conversation}),
        request: req,
    }
}
#[test]
fn selected_writer_cli_inspect_generate_process_exit_refine_and_recover_without_credentials() {
    let api = Api::new(vec![
        reply("# First\n\nx = 7; z = 9."),
        reply("# Refined\n\nx = 7; z = 9, exact."),
    ]);
    let r = ready(&api, true);
    let inspected = call(&r.config, "writer-inspect", &wire(&r.inspect), false);
    assert_eq!(inspected["status"], "complete");
    assert_eq!(api.count(), 0);
    let handles = &inspected["data"]["handles"];
    let generate = json!({"wire_version":1,"expected":handles["expected"],"root":handles["root"],"refine":null,"request":r.request});
    let generated = call(&r.config, "writer-generate", &wire(&generate), true);
    assert_eq!(generated["status"], "generated");
    assert_eq!(api.count(), 1);
    assert_eq!(generated["native_authority"], false);
    // A later winning root does not make the original exact operation fresh.
    {
        let store = r.local.open();
        let scope: SelectionScope = serde_json::from_value(r.inspect["scope"].clone()).unwrap();
        let snapshot = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
        let p = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
        let mut root = p
            .head(Namespace::Conversation, r.request.conversation())
            .unwrap()
            .0
            .clone();
        root.parents = [root.revision_id].into();
        root.revision_id = Uuid::new_v4();
        root.operation_id = Uuid::new_v4();
        store
            .commit_selected(&snapshot.token, Uuid::new_v4(), vec![root], BTreeMap::new())
            .unwrap();
    }
    let mut stale_new = generate.clone();
    stale_new["request"]["operation"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&r.config, "writer-generate", &wire(&stale_new), false)["status"],
        "unavailable"
    );
    assert_eq!(api.count(), 1);
    let recovered = call(&r.config, "writer-recover", &wire(&generate), false);
    assert_eq!(recovered["status"], "historical");
    assert!(recovered["data"]["handles"].is_null());
    assert_eq!(api.count(), 1);
    let replayed = call(&r.config, "writer-generate", &wire(&generate), false);
    assert_eq!(replayed["status"], "historical");
    assert_eq!(api.count(), 1);
    let reinspection = call(&r.config, "writer-inspect", &wire(&r.inspect), false);
    let handles = &reinspection["data"]["handles"];
    let draft = generated["data"]["draft"]["id"].as_str().unwrap();
    let request = Request::Refine {
        conversation: r.request.conversation(),
        operation: Uuid::new_v4(),
        request: "Keep study notes exact.".into(),
        draft: Uuid::parse_str(draft).unwrap(),
    };
    let refine = json!({"wire_version":1,"expected":handles["expected"],"root":handles["root"],"refine":{"source":handles["sources"][0],"draft":handles["drafts"][0]},"request":request});
    let refined = call(&r.config, "writer-refine", &wire(&refine), true);
    assert_eq!(refined["status"], "generated");
    assert_eq!(api.count(), 2);
    assert!(refined["data"]["handles"].is_object());
    let parts = api.requests.lock().unwrap();
    assert!(parts
        .iter()
        .all(|p| p.to_string().contains("ASSOCIATIVE_MEMORY")));
    assert!(parts[1].to_string().contains("First"));
    drop(parts);
    let mut unknown = generate.clone();
    unknown["request"]["operation"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&r.config, "writer-recover", &wire(&unknown), false)["status"],
        "not-found"
    );
    assert_eq!(api.count(), 2);
    drop(r.local);
}
#[test]
fn selected_writer_cli_rejects_bad_evidence_and_duplicate_fields_before_model() {
    let api = Api::new(vec![]);
    let r = ready(&api, false);
    let inspected = call(&r.config, "writer-inspect", &wire(&r.inspect), false);
    assert_eq!(inspected["status"], "complete");
    let h = &inspected["data"]["handles"];
    let generate = json!({"wire_version":1,"expected":h["expected"],"root":h["root"],"refine":null,"request":r.request});
    for field in ["raw_head_sha256", "binding_sha256", "selection_sha256"] {
        let mut input = generate.clone();
        if field == "raw_head_sha256" {
            input["expected"][field] = json!("e".repeat(64))
        } else {
            input["expected"]["selection"][field] = json!("e".repeat(64))
        };
        assert_eq!(
            call(&r.config, "writer-generate", &wire(&input), false)["status"],
            "unavailable"
        );
    }
    let mut unknown = generate.clone();
    unknown["unknown"] = json!(true);
    assert_eq!(
        call(&r.config, "writer-generate", &wire(&unknown), false)["status"],
        "invalid-input"
    );
    let encoded = String::from_utf8(wire(&generate)).unwrap();
    let duplicate = encoded.replacen("{", "{\"wire_version\":1,", 1);
    assert_eq!(
        call(&r.config, "writer-generate", duplicate.as_bytes(), false)["status"],
        "invalid-input"
    );
    assert_eq!(
        call(&r.config, "writer-refine", &wire(&generate), false)["status"],
        "invalid-input"
    );
    assert_eq!(api.count(), 0);
    assert_eq!(
        call(
            &r.config,
            "writer-generate",
            &vec![b' '; 256 * 1024 + 1],
            false
        )["status"],
        "invalid-input"
    );
    let conflicting = std::process::Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .args([
            "--conversation-operation",
            "writer-inspect",
            "--conversation-input",
            "-",
            "--writer-note",
        ])
        .env("REMARKABLE_BUDDIES_CONFIG", &r.config)
        .output()
        .unwrap();
    assert!(!conflicting.status.success());
    assert!(conflicting.stdout.is_empty());
}

#[test]
fn selected_writer_cli_prepared_without_finish_recovers_without_model_or_credentials() {
    let api = Api::new(vec![]);
    let r = ready(&api, false);
    let inspected = call(&r.config, "writer-inspect", &wire(&r.inspect), false);
    let h = &inspected["data"]["handles"];
    let input = json!({"wire_version":1,"expected":h["expected"],"root":h["root"],"refine":null,"request":r.request});
    {
        let store = r.local.open();
        let scope: SelectionScope = serde_json::from_value(r.inspect["scope"].clone()).unwrap();
        let snapshot = store.selected_snapshot(&scope, 4096).unwrap().unwrap();
        let writer = SelectedAdmission::new(store, scope).unwrap();
        let Request::Generate {
            document,
            selection,
            guidance,
            request,
            ..
        } = &r.request
        else {
            unreachable!()
        };
        let req = SelectedWriterPrepare {
            operation: r.request.operation(),
            root: serde_json::from_value(h["root"].clone()).unwrap(),
            request: request.clone(),
            source: SelectedWriterSource::Generate {
                normalized: NormalizedTextSource::new(
                    r.request.conversation(),
                    writer::stage(r.request.operation(), "user"),
                    document.clone(),
                    selection.clone(),
                    guidance.clone(),
                )
                .unwrap(),
            },
        };
        assert!(matches!(
            writer
                .prepare_selected_writer(&snapshot.token, req, 2)
                .unwrap(),
            SelectedWriterPreparation::Fresh { .. }
        ));
    }
    let recovered = call(&r.config, "writer-recover", &wire(&input), false);
    assert_eq!(recovered["status"], "historical-pending");
    assert!(recovered["data"]["handles"].is_null());
    assert_eq!(
        call(&r.config, "writer-generate", &wire(&input), false)["status"],
        "historical-pending"
    );
    let mut changed = input;
    changed["request"]["request"] = json!("conflicting text");
    assert_eq!(
        call(&r.config, "writer-recover", &wire(&changed), false)["status"],
        "unavailable"
    );
    assert_eq!(api.count(), 0);
}

#[test]
fn compound_writer_cli_keeps_qualification_sealed_and_original_recovery_readable() {
    use remarkable_reader_buddy::storage::{compound_generation, sync::mode, Kind};
    let api = Api::new(vec![]);
    let local = Local::new();
    let store = local.open();
    let group = Uuid::new_v4();
    let binding = digest(b"unqualified Writer command fixture");
    let preparation = compound_generation::Request {
        operation: Uuid::new_v4(),
        actor: store.actor_id,
        source_generation: serde_json::from_slice(
            &std::fs::read(local.0.join("data/CURRENT")).unwrap(),
        )
        .unwrap(),
        group,
        binding_sha256: binding.clone(),
        expected_mode: serde_json::from_value(
            serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap(),
        )
        .unwrap(),
    };
    let paths = store.paths.clone();
    drop(store);
    let store = Arc::new(Store::prepare_shared_notebook(paths.clone(), preparation).unwrap());
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::SharedGroup {
                group,
                binding_sha256: binding.clone(),
                root_id: "unqualified-writer-root".into(),
            },
        )
        .unwrap();
    let conversation = Uuid::new_v4();
    let req = request(conversation);
    let Request::Generate { document, .. } = &req else {
        unreachable!()
    };
    let scope = SelectionScope {
        group,
        key_sha256: memory::catalog::document_key(document.document).unwrap(),
        binding_sha256: binding,
    };
    // Explicit synthetic selected custody, not a qualified provider or native source.
    let root = Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: conversation,
        revision_id: Uuid::new_v4(),
        parents: Default::default(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(Record::Root(Root {
            id: conversation,
            next_sequence: 0,
            binding: None,
            created_ms: 1,
            updated_ms: 1,
        }))
        .unwrap(),
        media_descriptors: vec![],
    };
    let raw = serde_json::to_vec(&root).unwrap();
    let hash = digest(&raw);
    let initialized = store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"explicit command genesis"),
                selected: Manifest {
                    format: FORMAT,
                    transaction_id: Uuid::new_v4(),
                    scope: Scope::SelectedRecords,
                    records: vec![ObjectRef {
                        sha256: hash.clone(),
                        bytes: raw.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(hash, raw)]),
        )
        .unwrap();
    let config = local.0.join("config.json");
    let mut config_value = json!({"paths":paths,"base_url":api.origin,"product_settings":{"writer_enabled":true},"personalization":{"text":"PRIVATE LOCAL"}});
    std::fs::write(&config, wire(&config_value)).unwrap();
    drop(store);
    let inspected = call(
        &config,
        "writer-inspect",
        &wire(&json!({"wire_version":1,"scope":scope,"conversation":conversation})),
        false,
    );
    assert_eq!(inspected["status"], "complete");
    let handles = &inspected["data"]["handles"];
    let input = json!({"wire_version":1,"expected":handles["expected"],"root":handles["root"],"refine":null,"request":req});
    let refused = call(&config, "writer-generate", &wire(&input), true);
    assert_eq!(refused["status"], "unavailable");
    assert_eq!(refused["native_authority"], false);
    assert!(!refused.to_string().contains("PRIVATE"));
    assert_eq!(api.count(), 0);
    let store = local.open();
    assert_eq!(
        store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token,
        initialized.token
    );
    let writer = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
    let Request::Generate {
        document,
        selection,
        guidance,
        request,
        ..
    } = req
    else {
        unreachable!()
    };
    let operation: Uuid = serde_json::from_value(input["request"]["operation"].clone()).unwrap();
    let prepared = writer
        .prepare_selected_writer(
            &initialized.token,
            SelectedWriterPrepare {
                operation,
                root: serde_json::from_value(handles["root"].clone()).unwrap(),
                request,
                source: SelectedWriterSource::Generate {
                    normalized: NormalizedTextSource::new(
                        conversation,
                        writer::stage(operation, "user"),
                        document,
                        selection,
                        guidance,
                    )
                    .unwrap(),
                },
            },
            2,
        )
        .unwrap();
    assert!(matches!(prepared, SelectedWriterPreparation::Fresh { .. }));
    drop(prepared);
    drop(writer);
    drop(store);
    config_value["product_settings"]["writer_enabled"] = json!(false);
    std::fs::write(&config, wire(&config_value)).unwrap();
    for op in ["writer-recover", "writer-generate"] {
        let report = call(&config, op, &wire(&input), false);
        assert_eq!(report["status"], "historical-pending");
        assert!(report["data"]["handles"].is_null());
    }
    let mut changed = input;
    changed["request"]["request"] = json!("changed original request");
    assert_eq!(
        call(&config, "writer-recover", &wire(&changed), false)["status"],
        "unavailable"
    );
    assert_eq!(api.count(), 0);
}
