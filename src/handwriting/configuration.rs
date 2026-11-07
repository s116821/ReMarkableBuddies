//! Startup declaration; saved settings are not runtime admission.
use crate::storage::Uuid;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Serialize)]
pub struct Configuration {
    pub enabled: bool,
    pub profile: Option<Uuid>,
}
impl<'de> Deserialize<'de> for Configuration {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> serde::de::Visitor<'de> for Object {
            type Value = Configuration;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("handwriting configuration object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error;
                let mut result = Configuration::default();
                let mut seen = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(A::Error::custom("duplicate handwriting setting"));
                    }
                    match key.as_str() {
                        "enabled" => result.enabled = map.next_value()?,
                        "profile" => {
                            result.profile = map
                                .next_value::<Option<String>>()?
                                .map(|s| {
                                    let id = Uuid::parse_str(&s).map_err(|_| {
                                        A::Error::custom("invalid handwriting profile")
                                    })?;
                                    if id.is_nil() || id.to_string() != s {
                                        return Err(A::Error::custom(
                                            "invalid handwriting profile",
                                        ));
                                    }
                                    Ok(id)
                                })
                                .transpose()?
                        }
                        _ => return Err(A::Error::custom("unknown handwriting setting")),
                    }
                }
                Ok(result)
            }
        }
        d.deserialize_map(Object)
    }
}
impl Configuration {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.enabled || self.profile.is_some(),
            "handwriting profile absent"
        );
        anyhow::ensure!(
            self.profile.is_none_or(|p| !p.is_nil()),
            "nil handwriting profile"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_configuration_defaults_strict_objects_profile_and_restart_snapshot() {
        use crate::config::Config;
        let default: Config = serde_json::from_str("{}").unwrap();
        assert!(!default.handwriting.enabled && default.handwriting.profile.is_none());
        for raw in [
            r#"{"handwriting":null}"#,
            r#"{"handwriting":[]}"#,
            r#"{"handwriting":{"enabled":false,"enabled":true}}"#,
            r#"{"handwriting":{"unknown":1}}"#,
            r#"{"handwriting":{"profile":"00000000-0000-0000-0000-000000000000"}}"#,
        ] {
            assert!(serde_json::from_str::<Config>(raw).is_err());
        }
        let profile = Uuid::new_v4();
        let mut config = Config {
            handwriting: Configuration {
                enabled: true,
                profile: Some(profile),
            },
            ..Config::default()
        };
        assert!(config.validate_values().is_ok());
        let snapshot = config.handwriting.clone();
        config.handwriting.profile = Some(Uuid::new_v4());
        assert_eq!(snapshot.profile, Some(profile));
        let roundtrip: Config =
            serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(roundtrip.handwriting.profile, config.handwriting.profile);
        config.sync.enabled = true;
        assert!(config.validate_values().is_err());
        config.sync.enabled = false;
        config.handwriting.profile = None;
        assert!(config.validate_values().is_err());
        config.handwriting.enabled = false;
        assert!(config.validate_values().is_ok());
    }
    #[test]
    fn runtime_configuration_keeps_legacy_positional_config_prefix() {
        let object = serde_json::to_value(crate::config::Config::default()).unwrap();
        let names = [
            "config_schema_version",
            "product_settings",
            "model",
            "base_url",
            "trigger_corner",
            "log_level",
            "debug_dump",
            "paths",
            "model_credential",
            "sync",
            "memory",
            "reader",
            "personalization",
            "research_policy",
        ];
        let positional =
            serde_json::Value::Array(names.iter().map(|name| object[*name].clone()).collect());
        let loaded: crate::config::Config = serde_json::from_value(positional).unwrap();
        assert!(!loaded.handwriting.enabled && loaded.handwriting.profile.is_none());
        assert!(loaded.validate_values().is_ok());
    }
}
