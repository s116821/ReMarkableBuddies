use super::*;
use crate::{
    config::capacities::{
        tests::{grant_fixture, Temp},
        ExportConnection,
    },
    conversation::CapacitiesAttempt,
    storage::{digest, Namespace, StorePaths},
};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn original_command_uses_retained_grant_in_ready_notebook_and_refuses_ambiguity() {
    let root = Temp::new();
    let active_space = Uuid::new_v4();
    let active_grant = Uuid::new_v4();
    let original_space = Uuid::new_v4();
    let original_grant = Uuid::new_v4();
    let mut config = Config {
        paths: StorePaths {
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            credentials: root.path().join("credentials"),
        },
        capacities_export: Some(ExportConnection {
            schema_version: 1,
            space_id: active_space,
            grant_id: active_grant,
            tags: vec![],
        }),
        ..Config::default()
    };
    config.product_settings = Some(Some(
        serde_json::from_value(json!({
            "writer_enabled": false,
            "export_destination": "disabled"
        }))
        .unwrap(),
    ));
    let config_path = root.path().join("config.json");
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let conversation = Uuid::new_v4();
    let correlation = Uuid::new_v4();
    let run = || {
        let request = json!({"command": "reconcile", "conversation": conversation, "correlation": correlation});
        let bytes = serde_json::to_vec(&request).unwrap();
        command::run(Some(config_path.clone()), &mut bytes.as_slice())
    };
    assert_eq!(run().failure, Some(Failure::Storage));
    assert!(!config.paths.data.exists());

    let store = Store::open(config.paths.clone()).unwrap();
    let ledger = Ledger::new(Arc::new(store));
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    drop(ledger);
    let store = Store::prepare_notebook(config.paths.clone()).unwrap();
    let ledger = Ledger::new(Arc::new(store));
    let effect = Uuid::new_v4();
    let key = CapacitiesAttempt::association_id(conversation, original_space, correlation);
    let attempt = CapacitiesAttempt {
        evidence_version: 2,
        id: effect,
        space: original_space,
        structure: "RootPage".to_owned(),
        correlation,
        payload_sha256: digest(b"typed page"),
        body_sha256: digest(b"fixture note"),
        tags: BTreeSet::new(),
        grant_id: Some(original_grant),
        candidate: None,
        verified_body_block: None,
        managed: None,
    };
    let publish = |ledger: &Ledger, association: ExportAssociation| {
        let expected_root = ledger
            .expected(Namespace::Conversation, conversation)
            .unwrap();
        let heads = ledger
            .expected(Namespace::ExportAssociation, association.id)
            .unwrap();
        ledger
            .associate_export(association.operation, expected_root, heads, association)
            .unwrap();
    };
    publish(
        &ledger,
        ExportAssociation {
            id: key,
            conversation,
            backend: "capacities".into(),
            native_note: None,
            source_scope: "fixture notebook".into(),
            source_revision: "1".into(),
            operation: CapacitiesAttempt::stage_id(effect, "prepared"),
            outcome: Outcome::Prepared,
            attempt: None,
            capacities: Some(attempt),
        },
    );
    drop(ledger);

    fs::set_permissions(&config.paths.credentials, fs::Permissions::from_mode(0o700)).unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 900;
    let write_grant = |id, space| {
        let path = config
            .paths
            .credentials
            .join(format!("capacities-grant-{id}.json"));
        fs::write(
            &path,
            serde_json::to_vec(&grant_fixture(id, space, expires)).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    };
    write_grant(active_grant, active_space);
    assert_eq!(run().failure, Some(Failure::MissingCapacitiesCredentials));
    write_grant(original_grant, original_space);
    let selected = run();
    assert_eq!(selected.status, ExportStatus::ReconcileRequired);
    assert_eq!(selected.effect_operation, effect);
    assert!(selected.failure.is_none());
    let writer_input = serde_json::to_vec(
        &json!({"command": "reconcile-export", "conversation": conversation, "correlation": correlation}),
    ).unwrap();
    let crate::writer::command::CommandReport::Flow(writer) =
        crate::writer::command::dispatch(Some(config_path.clone()), &mut writer_input.as_slice())
    else {
        panic!("expected Writer flow report");
    };
    assert_eq!(writer.status, crate::writer_flow::Status::ReconcileRequired);
    assert!(!writer.observation.destination_polled && !writer.observation.write_polled);

    let store = Store::open_existing_for_export_reconciliation(config.paths.clone()).unwrap();
    let ledger = Ledger::new(Arc::new(store));
    publish(
        &ledger,
        ExportAssociation {
            id: correlation,
            conversation,
            backend: "mem".into(),
            native_note: None,
            source_scope: "fixture notebook".into(),
            source_revision: "1".into(),
            operation: Uuid::new_v4(),
            outcome: Outcome::Prepared,
            attempt: None,
            capacities: None,
        },
    );
    drop(ledger);
    assert_eq!(run().status, ExportStatus::Conflict);
}

#[test]
fn selected_capacities_requires_private_configuration_before_initializing_store() {
    let root = Temp::new();
    let config = json!({
        "paths": {"data": root.path().join("data"), "cache": root.path().join("cache"), "credentials": root.path().join("credentials")},
        "product_settings": {"export_destination": "capacities"}
    });
    let config_path = root.path().join("config.json");
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let note = PreparedNote {
        conversation: Uuid::new_v4(),
        correlation: Uuid::new_v4(),
        effect_operation: Uuid::new_v4(),
        source_scope: "fixture".into(),
        source_revision: "1".into(),
        title: "Prepared note".into(),
        markdown: "Private fixture content".into(),
    };
    let frame = serde_json::to_vec(&json!({"command": "export", "note": note})).unwrap();
    let result = command::run(Some(config_path), &mut frame.as_slice());
    assert_eq!(result.status, ExportStatus::Refused);
    assert_eq!(result.failure, Some(Failure::MissingCapacitiesCredentials));
    for path in ["data", "cache", "credentials"] {
        assert!(!root.path().join(path).exists());
    }
}
