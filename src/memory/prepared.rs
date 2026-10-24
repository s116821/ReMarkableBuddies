//! Conditional evidence for one deliberately prepared memory fact; never live tokens.
use super::*;
use crate::{
    config::Config,
    storage::{self, selection::SelectionScope},
};
use serde::de::Deserializer;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expected {
    pub revision: Uuid,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mode {
    pub actor: Uuid,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pin {
    pub scope: SelectionScope,
    pub store_generation: Uuid,
    pub aggregate_generation: Uuid,
    pub accepted_base_sha256: String,
    pub selection_sha256: String,
    pub head_sha256: String,
}
impl Pin {
    pub(crate) fn from_read(read: &storage::selection::SelectedReadSnapshot) -> Self {
        let t = &read.snapshot.token;
        Self {
            scope: t.scope().clone(),
            store_generation: t.store_generation(),
            aggregate_generation: t.aggregate_generation(),
            accepted_base_sha256: t.accepted_base_sha256().into(),
            selection_sha256: t.selection_sha256().into(),
            head_sha256: read.accepted_history[0].sha256.clone(),
        }
    }
    pub(crate) fn matches_history(
        &self,
        h: &storage::selection::AcceptedSelectionTransaction,
    ) -> Result<bool> {
        let t = &h.transaction;
        Ok(self.scope == t.scope
            && self.store_generation == t.store_generation
            && self.aggregate_generation == t.aggregate_generation
            && self.accepted_base_sha256 == t.accepted_base_sha256
            && self.selection_sha256 == storage::digest(&serde_json::to_vec(t)?)
            && self.head_sha256 == h.sha256)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Domain {
    Local {},
    Selected {
        identity: catalog::Identity,
        pins: Vec<Pin>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub config_sha256: String,
    pub mode: Mode,
    pub actor: Uuid,
    pub store_generation: Uuid,
    pub domain: Domain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub prepared_version: u32,
    pub operation: Uuid,
    #[serde(deserialize_with = "strict_key")]
    pub key: Key,
    pub expected: Vec<Expected>,
    pub binding: Binding,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub prepared_version: u32,
    pub operation: Uuid,
    #[serde(deserialize_with = "strict_key")]
    pub key: Key,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Confirmation {
    pub intent: Intent,
}
fn strict_key<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Key, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        profile: Uuid,
        scope: StrictScope,
        fact: Uuid,
    }
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
    enum StrictScope {
        Global {},
        Document { document: Uuid },
    }
    let w = Wire::deserialize(d)?;
    Ok(Key {
        profile: w.profile,
        scope: match w.scope {
            StrictScope::Global {} => Scope::Global,
            StrictScope::Document { document } => Scope::Document { document },
        },
        fact: w.fact,
    })
}
impl Request {
    pub(crate) fn validate(&self, profile: Uuid) -> Result<()> {
        ensure!(
            self.prepared_version == 1 && !self.operation.is_nil(),
            "invalid preparation"
        );
        self.key.validate()?;
        ensure!(self.key.profile == profile, "foreign preparation profile");
        Ok(())
    }
}
impl Intent {
    pub(crate) fn validate(&self, profile: Uuid) -> Result<()> {
        Request {
            prepared_version: self.prepared_version,
            operation: self.operation,
            key: self.key.clone(),
        }
        .validate(profile)?;
        ensure!(
            self.expected.len() == 1
                && self
                    .expected
                    .iter()
                    .all(|e| !e.revision.is_nil() && storage::valid_digest(&e.sha256)),
            "invalid expected value"
        );
        let b = &self.binding;
        ensure!(
            storage::valid_digest(&b.config_sha256)
                && !b.actor.is_nil()
                && !b.store_generation.is_nil()
                && b.mode.actor == b.actor
                && storage::valid_digest(&b.mode.sha256),
            "invalid preparation binding"
        );
        if let Domain::Selected { identity, pins } = &b.domain {
            identity.validate()?;
            ensure!(
                identity.profile == profile && (2..=65).contains(&pins.len()),
                "invalid prepared selection"
            );
            let mut prior = None;
            for p in pins {
                p.scope.validate()?;
                ensure!(
                    p.scope.group == identity.group
                        && p.scope.binding_sha256 == identity.binding_sha256
                        && p.store_generation == b.store_generation
                        && !p.aggregate_generation.is_nil()
                        && storage::valid_digest(&p.accepted_base_sha256)
                        && storage::valid_digest(&p.selection_sha256)
                        && storage::valid_digest(&p.head_sha256),
                    "invalid prepared pin"
                );
                ensure!(
                    prior.is_none_or(|v: &str| v < p.scope.key_sha256.as_str()),
                    "unsorted/duplicate prepared pins"
                );
                prior = Some(&p.scope.key_sha256);
            }
        }
        Ok(())
    }
    pub(crate) fn expected_map(&self) -> BTreeMap<Uuid, String> {
        self.expected
            .iter()
            .map(|e| (e.revision, e.sha256.clone()))
            .collect()
    }
    pub(crate) fn envelope(&self, store: &Store) -> Result<Envelope> {
        super::fact_envelope(
            store,
            self.key.profile,
            &self.key,
            self.operation,
            self.expected_map().keys().copied().collect(),
            None,
        )
    }
}
#[derive(Serialize)]
struct ConfigBinding<'a> {
    config_schema_version: u32,
    config_path: &'a Path,
    paths: &'a storage::StorePaths,
    memory: &'a Configuration,
    legacy_sync_enabled: bool,
}
pub(crate) fn config_digest(config: &Config, path: &Path) -> Result<String> {
    let binding = ConfigBinding {
        config_schema_version: config.config_schema_version,
        config_path: path,
        paths: &config.paths,
        memory: &config.memory,
        legacy_sync_enabled: config.sync.enabled,
    };
    let mut bytes = b"urn:remarkable-buddies:prepared-forget-config:v1:".to_vec();
    bytes.extend(serde_json::to_vec(&binding)?);
    Ok(storage::digest(&bytes))
}
pub(crate) fn supported(key: &Key, envelope: &Envelope) -> Result<()> {
    let fact = super::decode_fact(key.profile, envelope)?;
    ensure!(
        fact.key == *key && serde_json::to_value(&fact)? == envelope.payload,
        "noncanonical preview value"
    );
    ensure!(
        fact.observed_ms <= 9_007_199_254_740_991
            && fact.expires_ms.is_none_or(|n| n <= 9_007_199_254_740_991),
        "preview numeric precision"
    );
    Ok(())
}
pub(crate) fn expected(envelopes: &[Envelope]) -> Result<Vec<Expected>> {
    envelopes
        .iter()
        .map(|e| {
            Ok(Expected {
                revision: e.revision_id,
                sha256: storage::digest(&serde_json::to_vec(e)?),
            })
        })
        .collect()
}
pub(crate) fn verify_parents(intent: &Intent, parents: &[Envelope]) -> Result<()> {
    for e in &intent.expected {
        let parent = parents
            .iter()
            .find(|p| p.revision_id == e.revision)
            .context("original parent absent")?;
        supported(&intent.key, parent)?;
        ensure!(
            storage::digest(&serde_json::to_vec(parent)?) == e.sha256,
            "original parent content changed"
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepared_wire_refuses_nested_unknown_duplicates_and_foreign_authority() {
        let profile = Uuid::new_v4();
        let key = Key {
            profile,
            scope: Scope::Global,
            fact: Uuid::new_v4(),
        };
        let actor = Uuid::new_v4();
        let intent = Intent {
            prepared_version: 1,
            operation: Uuid::new_v4(),
            key,
            expected: vec![Expected {
                revision: Uuid::new_v4(),
                sha256: storage::digest(b"head"),
            }],
            binding: Binding {
                config_sha256: storage::digest(b"config"),
                mode: Mode {
                    actor,
                    sha256: storage::digest(b"mode"),
                },
                actor,
                store_generation: Uuid::new_v4(),
                domain: Domain::Local {},
            },
        };
        let value = serde_json::to_value(&intent).unwrap();
        intent.validate(profile).unwrap();
        assert!(intent.validate(Uuid::new_v4()).is_err());
        let mut cases = vec![];
        let mut v = value.clone();
        v["extra"] = serde_json::json!(true);
        cases.push(v);
        let mut v = value.clone();
        v["binding"]["extra"] = serde_json::json!(true);
        cases.push(v);
        let mut v = value.clone();
        v["binding"]["mode"]["extra"] = serde_json::json!(true);
        cases.push(v);
        let mut v = value.clone();
        v["binding"]["domain"]["extra"] = serde_json::json!(true);
        cases.push(v);
        let mut v = value.clone();
        v["key"]["scope"]["extra"] = serde_json::json!(true);
        cases.push(v);
        let mut v = value.clone();
        v["expected"][0]["extra"] = serde_json::json!(true);
        cases.push(v);
        for v in cases {
            assert!(serde_json::from_value::<Intent>(v).is_err());
        }
        let raw = serde_json::to_string(&intent).unwrap();
        for (needle, replacement) in [
            (
                "\"kind\":\"global\"",
                "\"kind\":\"global\",\"kind\":\"global\"",
            ),
            (
                "\"prepared_version\":1",
                "\"prepared_version\":1,\"prepared_version\":1",
            ),
        ] {
            assert!(serde_json::from_str::<Intent>(&raw.replace(needle, replacement)).is_err());
        }
        let hash = storage::digest(b"head");
        let duplicate = raw.replace(
            &format!("\"sha256\":\"{hash}\""),
            &format!("\"sha256\":\"{hash}\",\"sha256\":\"{hash}\""),
        );
        assert!(serde_json::from_str::<Intent>(&duplicate).is_err());
        let mut v = value;
        let duplicate_head = v["expected"][0].clone();
        v["expected"].as_array_mut().unwrap().push(duplicate_head);
        let duplicated: Intent = serde_json::from_value(v).unwrap();
        assert!(duplicated.validate(profile).is_err());
    }
}
