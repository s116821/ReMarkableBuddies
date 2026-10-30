//! Nonsecret role/backend policy. Missing declarations preserve historical behavior.
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportDestination {
    Mem,
    Disabled,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductConfiguration {
    pub schema_version: i128,
    pub reader_enabled: bool,
    pub writer_enabled: bool,
    pub export_destination: ExportDestination,
}
impl Default for ProductConfiguration {
    fn default() -> Self {
        Self {
            schema_version: 1,
            reader_enabled: true,
            writer_enabled: true,
            export_destination: ExportDestination::Mem,
        }
    }
}

// Map-only visitor: derived structs also accept positional arrays. Static errors
// never echo a supplied destination, member name, or unrelated private text.
impl<'de> Deserialize<'de> for ProductConfiguration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> de::Visitor<'de> for Object {
            type Value = ProductConfiguration;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("product settings object")
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut result = ProductConfiguration::default();
                let mut seen = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom("repeated product setting"));
                    }
                    let value: Value = map
                        .next_value()
                        .map_err(|_| de::Error::custom("invalid product setting"))?;
                    match key.as_str() {
                        "schema_version" => {
                            result.schema_version = value
                                .as_i64()
                                .map(i128::from)
                                .or_else(|| value.as_u64().map(i128::from))
                                .ok_or_else(|| {
                                    de::Error::custom("invalid product settings version")
                                })?
                        }
                        "reader_enabled" => {
                            result.reader_enabled = value
                                .as_bool()
                                .ok_or_else(|| de::Error::custom("invalid product setting"))?
                        }
                        "writer_enabled" => {
                            result.writer_enabled = value
                                .as_bool()
                                .ok_or_else(|| de::Error::custom("invalid product setting"))?
                        }
                        "export_destination" => {
                            result.export_destination = match value.as_str() {
                                Some("mem") => ExportDestination::Mem,
                                Some("disabled") => ExportDestination::Disabled,
                                _ => return Err(de::Error::custom("invalid export backend")),
                            }
                        }
                        _ => return Err(de::Error::custom("unsupported product setting")),
                    }
                }
                Ok(result)
            }
        }
        d.deserialize_map(Object)
            .map_err(|_| de::Error::custom("invalid product settings object"))
    }
}

impl super::Config {
    pub fn reader_enabled(&self) -> bool {
        self.product_settings
            .as_ref()
            .is_none_or(|p| p.reader_enabled)
    }
    /// Shared by every current new Writer entry; independent of the caller role.
    /// Original-only reconciliation deliberately does not call this new-work gate.
    pub fn admits_writer(&self, external_export: bool) -> bool {
        self.product_settings.as_ref().is_none_or(|p| {
            p.writer_enabled && (!external_export || p.export_destination == ExportDestination::Mem)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    #[test]
    fn absent_serialization_and_partial_runtime_defaults_are_independent_of_raw_leaves() {
        let old = Config::default();
        assert!(serde_json::to_value(&old)
            .unwrap()
            .get("product_settings")
            .is_none());
        for raw in [
            json!({}),
            json!({"product_settings":null}),
            json!({"product_settings":{}}),
            json!({"product_settings":{"schema_version":1}}),
        ] {
            let config: Config = serde_json::from_value(raw).unwrap();
            config.validate_values().unwrap();
            assert!(config.reader_enabled());
            assert!(config.admits_writer(false));
            assert!(config.admits_writer(true));
        }
        for reader in [false, true] {
            for writer in [false, true] {
                for backend in ["mem", "disabled"] {
                    let config:Config=serde_json::from_value(json!({"product_settings":{"reader_enabled":reader,"writer_enabled":writer,"export_destination":backend}})).unwrap();
                    assert_eq!(config.reader_enabled(), reader);
                    assert_eq!(config.admits_writer(false), writer);
                    assert_eq!(config.admits_writer(true), writer && backend == "mem");
                }
            }
        }
    }

    #[test]
    fn new_object_is_map_only_strict_and_errors_do_not_echo_supplied_values() {
        for invalid in [
            r#""PRIVATE_PARENT_STRING""#,
            "123456789",
            "true",
            r#"[]"#,
            r#"[1,true,true,"mem"]"#,
            r#"{"reader_enabled":null}"#,
            r#"{"schema_version":null}"#,
            r#"{"schema_version":1.0}"#,
            r#"{"writer_enabled":"PRIVATE_WRONG_TYPE"}"#,
            r#"{"export_destination":"PRIVATE_BACKEND"}"#,
            r#"{"PRIVATE_KEY":true}"#,
            r#"{"reader_enabled":true,"reader_enabled":false}"#,
        ] {
            let error = serde_json::from_str::<ProductConfiguration>(invalid)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("PRIVATE"), "{error}");
        }
        for version in ["-1", "0", "2", "18446744073709551615"] {
            let config: Config = serde_json::from_str(&format!(
                r#"{{"product_settings":{{"schema_version":{version}}}}}"#
            ))
            .unwrap();
            assert!(config.validate_values().is_err());
            assert_eq!(
                crate::config::inspection::document(
                    format!(r#"{{"product_settings":{{"schema_version":{version}}}}}"#).as_bytes()
                )
                .err(),
                Some(crate::config::inspection::Status::Unsupported)
            );
        }
    }
}
