//! Shared preference values. The caller owns current selection and mode admission.
use super::{inspection, merge_settings, validate, validate_value, Status, MAX_IO};
use crate::{config::Config, storage::digest};
use serde_json::{json, Map, Value};

type Result<T> = std::result::Result<T, Status>;

/// A bounded, validated V1 artifact. It may contain private personalization text.
#[derive(PartialEq, Eq)]
pub struct Artifact(Value);

impl Artifact {
    pub fn empty() -> Self {
        Self(json!({"portable_configuration_version": 1, "settings": {}}))
    }

    /// Use for wire input so duplicate members are rejected before materialization.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        validate(bytes).map(Self)
    }

    /// Use for payloads already decoded by a strict enclosing parser.
    pub fn from_value(value: Value) -> Result<Self> {
        if serde_json::to_vec(&value)
            .map_err(|_| Status::InvalidInput)?
            .len()
            > MAX_IO
        {
            return Err(Status::Unavailable);
        }
        validate_value(&value)?;
        Ok(Self(value))
    }

    pub fn value(&self) -> &Value {
        &self.0
    }

    pub fn sha256(&self) -> String {
        digest(&serde_json::to_vec(self.value()).expect("validated JSON artifact"))
    }

    /// Produce the cumulative raw declaration value for the next record revision.
    pub fn overlay(&self, patch: &Self) -> Result<Self> {
        let mut next = self.0.clone();
        merge_settings(
            next["settings"]
                .as_object_mut()
                .expect("validated portable settings"),
            patch.settings(),
        );
        Self::from_value(next)
    }

    fn settings(&self) -> &Map<String, Value> {
        self.0["settings"]
            .as_object()
            .expect("validated portable settings")
    }
}

/// The raw declarations and typed local Config captured from the same bytes.
pub struct LocalConfigSnapshot {
    raw: Map<String, Value>,
    config: Config,
}

impl LocalConfigSnapshot {
    /// Value validation only. The caller validates paths and acquires admission.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (config, raw) = inspection::document(bytes)?;
        let Value::Object(raw) = raw else {
            return Err(Status::InvalidInput);
        };
        Ok(Self { raw, config })
    }

    pub fn base_config(&self) -> &Config {
        &self.config
    }

    /// Compute the effective Config for a request without publishing a file.
    /// Pass None only after establishing absence under the current admission.
    pub fn effective(&self, artifact: Option<&Artifact>) -> Result<Config> {
        let Some(artifact) = artifact else {
            return Ok(self.config.clone());
        };
        let mut raw = self.raw.clone();
        merge_settings(&mut raw, artifact.settings());
        let bytes = serde_json::to_vec(&raw).map_err(|_| Status::InvalidInput)?;
        inspection::document(&bytes).map(|(config, _)| config)
    }
}

#[cfg(test)]
mod tests;
