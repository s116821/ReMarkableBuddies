use super::*;
use crate::storage::{
    self,
    selection::{SelectionChange, SelectionScope},
    Scope, Store, StorePaths,
};
use serde_json::json;
use std::{fs, path::PathBuf};

fn patch(value: serde_json::Value) -> Artifact {
    Artifact::from_value(json!({"portable_configuration_version":1,"settings":value})).unwrap()
}
fn condition(group: Uuid) -> Condition {
    Condition {
        group,
        mode_epoch: Uuid::new_v4(),
        binding_sha256: digest(b"portable binding"),
        store_generation: Uuid::new_v4(),
        local_config_sha256: digest(b"local configuration"),
        selection: SelectionCondition::Absent {},
    }
}
fn scope(original: &Condition) -> SelectionScope {
    SelectionScope {
        group: original.group,
        key_sha256: key_sha256(),
        binding_sha256: original.binding_sha256.clone(),
    }
}
fn bundle(records: &[Envelope]) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let mut objects = BTreeMap::new();
    let mut references = vec![];
    let mut namespaces = BTreeMap::new();
    for record in records {
        // Noncanonical original bytes are intentional; projection must preserve
        // their actual ObjectRef rather than hash a parsed replacement.
        let bytes = serde_json::to_vec_pretty(record).unwrap();
        let sha = digest(&bytes);
        references.push(ObjectRef {
            sha256: sha.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(sha.clone(), record.namespace);
        objects.insert(sha, bytes);
    }
    (
        Manifest {
            format: 1,
            transaction_id: Uuid::new_v4(),
            scope: Scope::SelectedRecords,
            records: references,
            record_namespaces: namespaces,
            media: vec![],
            media_coverage: vec![],
        },
        objects,
    )
}
fn projection(original: &Condition, records: &[Envelope]) -> Projection {
    let (manifest, objects) = bundle(records);
    project(&scope(original), &manifest, |reference| {
        Ok(objects[&reference.sha256].clone())
    })
    .unwrap()
}

#[test]
fn declaration_noop_sparse_chain_and_original_revision_are_distinct_from_fallback() {
    let original = condition(Uuid::new_v4());
    let actor = Uuid::new_v4();
    let operation = Uuid::new_v4();
    assert!(
        prepare(actor, operation, &original, None, &Artifact::empty())
            .unwrap()
            .is_none()
    );
    let declared = patch(json!({"product_settings":null}));
    let first = prepare(actor, operation, &original, None, &declared)
        .unwrap()
        .unwrap();
    assert!(
        first
            == prepare(actor, operation, &original, None, &declared)
                .unwrap()
                .unwrap()
    );
    assert_eq!(
        first.record_id,
        Uuid::new_v5(&original.group, NAME.as_bytes())
    );
    let prior = projection(&original, std::slice::from_ref(&first));
    let mut next = original.clone();
    next.selection = SelectionCondition::Selected {
        aggregate_generation: Uuid::new_v4(),
        accepted_base_sha256: digest(b"accepted base"),
        selection_sha256: digest(b"selected head"),
        revision: prior.revision(),
    };
    assert!(
        prepare(actor, Uuid::new_v4(), &next, Some(&prior), &declared)
            .unwrap()
            .is_none()
    );
    // Empty declaration object and explicit null are different even when both
    // decode to the same effective local/default product behavior.
    let empty_object = patch(json!({"product_settings":{}}));
    let second = prepare(actor, Uuid::new_v4(), &next, Some(&prior), &empty_object)
        .unwrap()
        .unwrap();
    assert_eq!(second.parents, BTreeSet::from([first.revision_id]));
    let current = projection(&original, &[second, first]);
    assert_eq!(
        current.artifact().value()["settings"]["product_settings"],
        json!({})
    );
    let mut changed = original.clone();
    changed.local_config_sha256 = digest(b"changed local configuration");
    let other = prepare(actor, operation, &changed, None, &declared)
        .unwrap()
        .unwrap();
    assert_ne!(other.revision_id, prior.revision());
    changed.group = Uuid::new_v4();
    changed.selection = next.selection.clone();
    assert!(prepare(actor, Uuid::new_v4(), &changed, Some(&prior), &declared).is_err());
}

#[test]
fn original_raw_duplicates_foreign_scope_and_conflicted_chain_refuse() {
    let original = condition(Uuid::new_v4());
    let actor = Uuid::new_v4();
    let first = prepare(
        actor,
        Uuid::new_v4(),
        &original,
        None,
        &patch(json!({"product_settings":{"writer_enabled":false}})),
    )
    .unwrap()
    .unwrap();
    let (mut manifest, mut objects) = bundle(std::slice::from_ref(&first));
    let old = manifest.records[0].sha256.clone();
    let raw = String::from_utf8(objects.remove(&old).unwrap())
        .unwrap()
        .replace(
            "\"writer_enabled\": false",
            "\"writer_enabled\": false, \"writer_enabled\": true",
        );
    let sha = digest(raw.as_bytes());
    manifest.records[0] = ObjectRef {
        sha256: sha.clone(),
        bytes: raw.len() as u64,
    };
    manifest.record_namespaces = BTreeMap::from([(sha.clone(), Namespace::PortableConfiguration)]);
    objects.insert(sha, raw.into_bytes());
    assert!(project(&scope(&original), &manifest, |reference| Ok(objects
        [&reference.sha256]
        .clone()))
    .is_err());
    let prior = projection(&original, std::slice::from_ref(&first));
    let mut selected = original.clone();
    selected.selection = SelectionCondition::Selected {
        aggregate_generation: Uuid::new_v4(),
        accepted_base_sha256: digest(b"base"),
        selection_sha256: digest(b"selection"),
        revision: prior.revision(),
    };
    let a = prepare(
        actor,
        Uuid::new_v4(),
        &selected,
        Some(&prior),
        &patch(json!({"product_settings":{"writer_enabled":true}})),
    )
    .unwrap()
    .unwrap();
    let b = prepare(
        actor,
        Uuid::new_v4(),
        &selected,
        Some(&prior),
        &patch(json!({"research_policy":null})),
    )
    .unwrap()
    .unwrap();
    let (manifest, objects) = bundle(&[first.clone(), a, b]);
    assert!(project(&scope(&original), &manifest, |reference| Ok(objects
        [&reference.sha256]
        .clone()))
    .is_err());
    let (manifest, objects) = bundle(std::slice::from_ref(&first));
    let mut foreign = scope(&original);
    foreign.group = Uuid::new_v4();
    assert!(project(&foreign, &manifest, |reference| Ok(objects
        [&reference.sha256]
        .clone()))
    .is_err());
    foreign = scope(&original);
    foreign.key_sha256 = digest(b"wrong key");
    assert!(project(&foreign, &manifest, |reference| Ok(objects
        [&reference.sha256]
        .clone()))
    .is_err());
    let mut tombstone = first;
    tombstone.kind = Kind::Tombstone;
    tombstone.payload = serde_json::Value::Null;
    assert!(tombstone.validate().is_err());
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("portable-record-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn generation(&self) -> Uuid {
        serde_json::from_slice(&fs::read(self.paths().data.join("CURRENT")).unwrap()).unwrap()
    }
    fn prepare(&self, original: &Condition) -> Store {
        let store = Store::open(self.paths()).unwrap();
        let request = storage::compound_generation::Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: self.generation(),
            group: original.group,
            binding_sha256: original.binding_sha256.clone(),
            expected_mode: serde_json::from_value(
                serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap(),
            )
            .unwrap(),
        };
        drop(store);
        Store::prepare_shared_notebook(self.paths(), request).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn actual_selected_store_roundtrip_keeps_raw_references_and_domain_head() {
    let fixture = Fixture::new();
    let mut original = condition(Uuid::new_v4());
    let store = fixture.prepare(&original);
    original.store_generation = fixture.generation();
    let operation = Uuid::new_v4();
    let record = prepare(
        store.actor_id,
        operation,
        &original,
        None,
        &patch(json!({"personalization":{"enabled":true,"text":"  synthetic private text\n"}})),
    )
    .unwrap()
    .unwrap();
    let (manifest, objects) = bundle(std::slice::from_ref(&record));
    let expected = manifest.records.clone();
    let published = store
        .initialize_selected(
            &scope(&original),
            SelectionChange {
                operation,
                accepted_base_sha256: digest(b"qualified genesis fixture"),
                selected: manifest,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    assert_eq!(published.transaction.selected.records, expected);
    drop(store);
    let store = Store::open(fixture.paths()).unwrap();
    let selected = store
        .selected_snapshot(&scope(&original), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let decoded = project(
        &scope(&original),
        &selected.transaction.selected,
        |reference| store.read_object(reference),
    )
    .unwrap();
    assert_eq!(decoded.revision(), record.revision_id);
    assert_eq!(decoded.operation(), operation);
    assert_eq!(
        decoded.artifact().value()["settings"]["personalization"]["text"],
        "  synthetic private text\n"
    );
    assert_eq!(selected.transaction.selected.records, expected);
}

#[test]
fn legacy_families_refuse_portable_namespace_without_generation_or_object_changes() {
    for family in [1, 2, 3, 4] {
        let fixture = Fixture::new();
        let original = condition(Uuid::new_v4());
        let mut store = if family == 4 {
            Store::prepare_notebook(fixture.paths()).unwrap()
        } else {
            Store::open(fixture.paths()).unwrap()
        };
        if family == 3 {
            storage::feedback_generation::prepare(&mut store, |_| Ok(())).unwrap();
        }
        if family == 2 {
            storage::selection::prepare_generation(
                &fixture
                    .paths()
                    .data
                    .join("generations")
                    .join(fixture.generation().to_string()),
                store.actor_id,
                fixture.generation(),
            )
            .unwrap();
        }
        let root = fixture
            .paths()
            .data
            .join("generations")
            .join(fixture.generation().to_string());
        let current = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
        let format = fs::read(root.join("format.json")).unwrap();
        let objects_before = fs::read_dir(root.join("objects")).unwrap().count();
        let operation = Uuid::new_v4();
        let record = prepare(
            store.actor_id,
            operation,
            &original,
            None,
            &patch(json!({"research_policy":null})),
        )
        .unwrap()
        .unwrap();
        assert!(store.commit(vec![record.clone()], BTreeMap::new()).is_err());
        let (manifest, objects) = bundle(&[record]);
        assert!(store
            .initialize_selected(
                &scope(&original),
                SelectionChange {
                    operation,
                    accepted_base_sha256: digest(b"not qualified"),
                    selected: manifest,
                    retained: vec![]
                },
                objects
            )
            .is_err());
        assert_eq!(
            fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
            current
        );
        assert_eq!(fs::read(root.join("format.json")).unwrap(), format);
        assert_eq!(
            fs::read_dir(root.join("objects")).unwrap().count(),
            objects_before
        );
        assert!(store
            .selected_snapshot(&scope(&original), MAX_ITEMS)
            .unwrap()
            .is_none());
    }
}
