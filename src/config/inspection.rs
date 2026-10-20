//! Saved declarations only. No Store admission, environment resolution or root probes.
use super::Config;
use crate::storage::{digest, files, MAX_RECORD};
use anyhow::{ensure, Result};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Serialize,
};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::Write,
    path::PathBuf,
};

pub const MAX_OUTPUT: usize = 128 * 1024;
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Operation {
    Inspect,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    Unavailable,
    InvalidInput,
    Unsupported,
}
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Origin {
    Default,
    Declared,
    DeclaredNull,
}
#[derive(Serialize)]
struct Field {
    origin: Origin,
    value: Value,
}
#[derive(Serialize)]
struct Snapshot {
    schema_version: u32,
    config_sha256: String,
    byte_basis: &'static str,
    validation_basis: &'static str,
    declared_roots_validated: bool,
    fields: BTreeMap<&'static str, Field>,
    unsupported: [&'static str; 6],
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: &'static str,
    pub status: Status,
    runtime_activation: &'static str,
    snapshot: Option<Snapshot>,
}
impl Report {
    fn refusal(status: Status) -> Self {
        Self {
            report_version: 1,
            operation: "inspect",
            status,
            runtime_activation: "unverified",
            snapshot: None,
        }
    }
    pub fn write_to(&self, output: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_OUTPUT,
            "configuration report exceeds bound"
        );
        output.write_all(&bytes)?;
        Ok(())
    }
}

/// No stdin parameter: this observation cannot acquire input or initialize a Store.
pub fn run(_operation: Operation, path: Option<PathBuf>) -> Report {
    let Some(path) = path.filter(|p| p.is_absolute()) else {
        return Report::refusal(Status::Unavailable);
    };
    let Ok(bytes) = files::read(&path, MAX_RECORD as u64) else {
        return Report::refusal(Status::Unavailable);
    };
    inspect_bytes(&bytes)
}

pub(crate) fn document(bytes: &[u8]) -> std::result::Result<(Config, Value), Status> {
    if bytes.len() > MAX_RECORD {
        return Err(Status::Unavailable);
    }
    // Deserialize directly, before any map can collapse duplicate typed fields.
    let Ok(config) = serde_json::from_slice::<Config>(bytes) else {
        return Err(Status::InvalidInput);
    };
    let Ok(raw) = strict_json(bytes) else {
        return Err(Status::InvalidInput);
    };
    if !raw.is_object()
        || ["paths", "memory", "sync", "personalization"]
            .iter()
            .any(|key| raw.get(key).is_some_and(|v| !v.is_object()))
        || raw
            .pointer("/memory/selection")
            .is_some_and(|v| !v.is_null() && !v.is_object())
    {
        return Err(Status::InvalidInput);
    }
    if config.config_schema_version != 1 || config.personalization.schema_version != 1 {
        return Err(Status::Unsupported);
    }
    if let Some(namespaces) = raw.pointer("/sync/namespaces").and_then(Value::as_array) {
        let unique: BTreeSet<_> = namespaces.iter().filter_map(Value::as_str).collect();
        if unique.len() != namespaces.len() {
            return Err(Status::InvalidInput);
        }
    }
    // Intentionally excludes Config::validate/StorePaths::validate and filesystem I/O.
    if config.validate_values().is_err() {
        return Err(Status::InvalidInput);
    }
    Ok((config, raw))
}

pub(crate) fn strict_json(bytes: &[u8]) -> serde_json::Result<Value> {
    serde_json::from_slice::<StrictValue>(bytes).map(|value| value.0)
}

fn inspect_bytes(bytes: &[u8]) -> Report {
    let (config, raw) = match document(bytes) {
        Ok(value) => value,
        Err(status) => return Report::refusal(status),
    };
    let mut fields = BTreeMap::new();
    let mut add = |name: &'static str, source: &str, value: Value| {
        let declared = source.split('.').try_fold(&raw, |v, key| v.get(key));
        let origin = match declared {
            None => Origin::Default,
            Some(Value::Null) => Origin::DeclaredNull,
            Some(_) => Origin::Declared,
        };
        fields.insert(name, Field { origin, value });
    };
    add(
        "config_schema_version",
        "config_schema_version",
        json!(config.config_schema_version),
    );
    add("model", "model", json!(config.model.is_some()));
    add("base_url", "base_url", json!(config.base_url.is_some()));
    add(
        "model_credential",
        "model_credential",
        json!(config.model_credential.is_some()),
    );
    add(
        "trigger_corner",
        "trigger_corner",
        json!(config.trigger_corner),
    );
    add("log_level", "log_level", json!(config.log_level));
    add("debug_dump", "debug_dump", json!(config.debug_dump));
    for name in ["paths.data", "paths.cache", "paths.credentials"] {
        add(name, "paths", json!(true));
    }
    add(
        "memory.enabled",
        "memory.enabled",
        json!(config.memory.enabled),
    );
    add(
        "memory.profile",
        "memory.profile",
        json!(config.memory.profile.is_some()),
    );
    add(
        "memory.selection",
        "memory.selection",
        json!(config.memory.selection.is_some()),
    );
    add("sync.enabled", "sync.enabled", json!(config.sync.enabled));
    add("sync.media", "sync.media", json!(config.sync.media));
    add(
        "sync.create_new",
        "sync.create_new",
        json!(config.sync.create_new),
    );
    let mut namespaces: Vec<_> = config
        .sync
        .namespaces
        .iter()
        .map(|n| {
            serde_json::to_value(n)
                .expect("namespace serialization")
                .as_str()
                .expect("namespace string")
                .to_owned()
        })
        .collect();
    namespaces.sort();
    add("sync.namespaces", "sync.namespaces", json!(namespaces));
    add(
        "sync.max_media_bytes",
        "sync.max_media_bytes",
        json!(config.sync.max_media_bytes),
    );
    add(
        "sync.poll_seconds",
        "sync.poll_seconds",
        json!(config.sync.poll_seconds),
    );
    add(
        "sync.batch_items",
        "sync.batch_items",
        json!(config.sync.batch_items),
    );
    add(
        "sync.collection",
        "sync.collection",
        json!(config.sync.collection.is_some()),
    );
    add(
        "personalization.schema_version",
        "personalization.schema_version",
        json!(config.personalization.schema_version),
    );
    add(
        "personalization.enabled",
        "personalization.enabled",
        json!(config.personalization.enabled),
    );
    add(
        "personalization.text",
        "personalization.text",
        json!(!config.personalization.text.is_empty()),
    );
    add(
        "personalization.text_bytes",
        "personalization.text",
        json!(config.personalization.text.len()),
    );
    Report {
        report_version: 1,
        operation: "inspect",
        status: Status::Ok,
        runtime_activation: "unverified",
        snapshot: Some(Snapshot {
            schema_version: 1,
            config_sha256: digest(bytes),
            byte_basis: "saved-file-bytes",
            validation_basis: "typed-values-only",
            declared_roots_validated: false,
            fields,
            unsupported: [
                "reader-enablement",
                "writer-enablement",
                "retrieval-policy",
                "research-policy",
                "mem-destination",
                "handwriting-settings",
            ],
        }),
    }
}

/// The second parse preserves origins and rejects even repeated null Option fields.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Strict;
        impl<'de> Visitor<'de> for Strict {
            type Value = StrictValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without repeated fields")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut map = Map::new();
                while let Some(key) = a.next_key::<String>()? {
                    if map.contains_key(&key) {
                        return Err(de::Error::custom("repeated configuration field"));
                    }
                    map.insert(key, a.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(map)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = a.next_element::<StrictValue>()? {
                    values.push(value.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
        }
        d.deserialize_any(Strict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn retained_valid_wire_and_refusals_remain_byte_identical() {
        use base64::Engine;
        let corpus: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/saved-configuration-4bc7eb0-corpus.json"
        ))
        .unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let input = &case["input"];
            let bytes = if let Some(text) = input["utf8"].as_str() {
                text.as_bytes().to_vec()
            } else if let Some(encoded) = input["base64"].as_str() {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap()
            } else {
                let mut bytes = input["prefix_utf8"].as_str().unwrap().as_bytes().to_vec();
                bytes.resize(
                    input["total_bytes"].as_u64().unwrap() as usize,
                    input["padding_byte"].as_u64().unwrap() as u8,
                );
                bytes
            };
            let mut actual = Vec::new();
            inspect_bytes(&bytes).write_to(&mut actual).unwrap();
            assert_eq!(
                actual,
                case["stdout_utf8"].as_str().unwrap().as_bytes(),
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn schema_struct_arrays_refuse_without_narrowing_collection_arrays() {
        // The scoped maintenance guard does not rewrite legacy Config deserialization.
        assert!(serde_json::from_slice::<Config>(b"[]").is_ok());
        for raw in [
            r#"[]"#,
            r#"{"paths":["/a/b","/c/d","/e/f"]}"#,
            r#"{"memory":[]}"#,
            r#"{"sync":[]}"#,
            r#"{"personalization":[]}"#,
            r#"{"memory":{"profile":"a138b641-a4aa-4ac3-b1d1-091ba27f1470","selection":["b2f1cc78-a8d8-4dc1-a3ce-a40cdff053ba","a138b641-a4aa-4ac3-b1d1-091ba27f1470","a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5"]}}"#,
        ] {
            assert_eq!(inspect_bytes(raw.as_bytes()).status, Status::InvalidInput);
        }
        assert_eq!(inspect_bytes(br#"{"sync":{"namespaces":["source","conversation"]},"memory":{"selection":null},"debug_dump":null}"#).status, Status::Ok);
    }

    #[test]
    fn input_bound_is_inclusive_and_writer_never_emits_a_second_report() {
        let mut bytes = b"{}".to_vec();
        bytes.resize(MAX_RECORD, b' ');
        let report = inspect_bytes(&bytes);
        assert_eq!(report.status, Status::Ok);
        let mut complete = Vec::new();
        report.write_to(&mut complete).unwrap();
        assert!(complete.len() < MAX_OUTPUT);
        assert_eq!(complete.iter().filter(|&&b| b == b'\n').count(), 1);
        bytes.push(b' ');
        assert_eq!(inspect_bytes(&bytes).status, Status::Unavailable);
        struct Prefix {
            bytes: Vec<u8>,
            calls: usize,
        }
        impl Write for Prefix {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.calls += 1;
                if self.calls > 1 {
                    return Err(io::Error::other("writer stopped"));
                }
                self.bytes.extend_from_slice(&bytes[..17]);
                Ok(17)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut prefix = Prefix {
            bytes: Vec::new(),
            calls: 0,
        };
        assert!(report.write_to(&mut prefix).is_err());
        assert_eq!(prefix.calls, 2);
        assert_eq!(prefix.bytes, complete[..17]);
        assert!(serde_json::from_slice::<Value>(&prefix.bytes).is_err());
    }

    #[test]
    fn duplicate_null_options_and_nested_names_refuse_before_projection() {
        for raw in [
            r#"{"model":null,"model":null}"#,
            r#"{"memory":{"profile":null,"profile":null}}"#,
            r#"{"sync":{"collection":null,"collection":null}}"#,
            r#"{"personalization":{"text":"","text":""}}"#,
            r#"{"sync":{"namespaces":["source","source"]}}"#,
        ] {
            assert_eq!(inspect_bytes(raw.as_bytes()).status, Status::InvalidInput);
        }
    }
}
