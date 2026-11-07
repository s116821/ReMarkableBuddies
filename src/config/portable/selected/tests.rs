use super::*;
use crate::{config::portable::project, conversation::receiving::memory::research::ResearchPolicy};

fn artifact(settings: Value) -> Artifact {
    Artifact::from_value(json!({"portable_configuration_version": 1, "settings": settings}))
        .unwrap()
}

#[test]
fn wire_validation_preserves_compact_identity_and_rejects_invalid_values() {
    let bytes = br#" { "settings": {"research_policy":null,"product_settings":{"writer_enabled":false}}, "portable_configuration_version": 1 } "#;
    let parsed = Artifact::parse(bytes).unwrap();
    let equivalent = artifact(json!({
        "product_settings": {"writer_enabled": false},
        "research_policy": null
    }));
    assert_eq!(parsed.value(), equivalent.value());
    assert_eq!(parsed.sha256(), equivalent.sha256());

    for (input, expected) in [
        (
            br#"{"portable_configuration_version":1,"settings":{"product_settings":{"writer_enabled":false,"writer_enabled":true}}}"#.as_slice(),
            Status::InvalidInput,
        ),
        (
            br#"{"portable_configuration_version":1,"settings":{"personalization":null}}"#.as_slice(),
            Status::InvalidInput,
        ),
        (
            br#"{"portable_configuration_version":1,"settings":{"base_url":"https://local.invalid"}}"#.as_slice(),
            Status::InvalidInput,
        ),
        (
            br#"{"portable_configuration_version":2,"settings":{}}"#.as_slice(),
            Status::Unsupported,
        ),
    ] {
        assert_eq!(Artifact::parse(input).err(), Some(expected));
    }
    let mut padded = serde_json::to_vec(parsed.value()).unwrap();
    padded.resize(MAX_IO, b' ');
    assert_eq!(Artifact::parse(&padded).unwrap().sha256(), parsed.sha256());
    padded.push(b' ');
    assert_eq!(Artifact::parse(&padded).err(), Some(Status::Unavailable));
}

#[test]
fn cumulative_edit_preserves_sparse_children_null_and_exact_text() {
    let base = artifact(json!({
        "product_settings": {"writer_enabled": false},
        "personalization": {"text": "  café\n"},
        "research_policy": null
    }));
    let patch = artifact(json!({
        "product_settings": {},
        "personalization": {"enabled": true}
    }));
    let next = base.overlay(&patch).unwrap();
    assert_eq!(
        next.value()["settings"],
        json!({
            "product_settings": {"writer_enabled": false},
            "personalization": {"enabled": true, "text": "  café\n"},
            "research_policy": null
        })
    );
    assert!(next.overlay(&Artifact::empty()).unwrap() == next);
    assert!(next.overlay(&patch).unwrap() == next);

    let null = next
        .overlay(&artifact(json!({"product_settings": null})))
        .unwrap();
    assert_eq!(null.value()["settings"]["product_settings"], Value::Null);
    let empty = null
        .overlay(&artifact(json!({"product_settings": {}})))
        .unwrap();
    assert_eq!(empty.value()["settings"]["product_settings"], json!({}));
    assert_ne!(empty.sha256(), null.sha256());
    assert_eq!(
        Artifact::empty().overlay(&patch).unwrap().value(),
        patch.value()
    );
}

#[test]
fn effective_config_changes_only_preferences_and_leaves_the_local_snapshot_intact() {
    let local = json!({
        "config_schema_version": 1,
        "model": "local-model",
        "base_url": "https://local.invalid",
        "model_credential": "local-reference",
        "trigger_corner": "LL",
        "paths": {"data": "/owned/data", "cache": "/owned/cache", "credentials": "/owned/credentials"},
        "memory": {"enabled": true, "profile": "7a9b6af7-d169-44a8-9438-8cc951d22980"},
        "reader": {"selection": null},
        "sync": {"enabled": false, "poll_seconds": 75},
        "product_settings": {"writer_enabled": false, "export_destination": "disabled"},
        "personalization": {"text": "Local text"},
        "research_policy": "disabled"
    });
    let bytes = serde_json::to_vec(&local).unwrap();
    let snapshot = LocalConfigSnapshot::parse(&bytes).unwrap();
    let exported = Artifact::from_value(project(&bytes).unwrap()).unwrap();
    assert_eq!(
        exported.value()["settings"]["personalization"],
        json!({"text": "Local text"})
    );

    let chosen = artifact(json!({
        "product_settings": {"writer_enabled": true},
        "personalization": {"enabled": true, "text": "Shared text\n"},
        "research_policy": null
    }));
    let effective = snapshot.effective(Some(&chosen)).unwrap();
    assert!(effective.admits_writer(false));
    assert!(!effective.admits_writer(true));
    assert_eq!(
        effective
            .personalization_snapshot()
            .unwrap()
            .effective_text(),
        Some("Shared text\n")
    );
    assert_eq!(
        effective.research_policy_snapshot().unwrap().policy(),
        ResearchPolicy::Required
    );
    let base = serde_json::to_value(snapshot.base_config()).unwrap();
    let actual = serde_json::to_value(&effective).unwrap();
    for field in [
        "model",
        "base_url",
        "model_credential",
        "trigger_corner",
        "paths",
        "reader",
        "memory",
        "sync",
    ] {
        assert_eq!(actual[field], base[field], "{field}");
    }
    assert!(!snapshot.base_config().admits_writer(false));
    assert_eq!(
        serde_json::to_value(snapshot.effective(None).unwrap()).unwrap(),
        base
    );
    assert_eq!(
        snapshot.raw.get("personalization"),
        local.get("personalization")
    );
}
