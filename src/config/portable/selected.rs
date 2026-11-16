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

    /// A closed display view; private text is disclosed only by explicit request.
    /// Origins follow the same sparse parent replacement as effective().
    pub(crate) fn view(&self, artifact: Option<&Artifact>, reveal_text: bool) -> Result<Value> {
        let effective = self.effective(artifact)?;
        let product = effective.product_settings.as_ref().and_then(Option::as_ref);
        let policy = effective.research_policy.flatten().unwrap_or_default();
        let mut view = json!({
            "reader_enabled": effective.reader_enabled(),
            "writer_enabled": product.is_none_or(|p| p.writer_enabled),
            "export_destination": product.map(|p| p.export_destination).unwrap_or(super::super::product::ExportDestination::Mem),
            "personalization_enabled": effective.personalization.enabled,
            "personalization_text_present": !effective.personalization.text.is_empty(),
            "research_policy": policy,
        });
        if reveal_text {
            view["personalization_text"] = if effective.personalization.text.is_empty() {
                Value::Null
            } else {
                Value::String(effective.personalization.text.clone())
            };
        }
        let mut origins = Map::new();
        for (field, parent, child) in [
            ("reader_enabled", "product_settings", Some("reader_enabled")),
            ("writer_enabled", "product_settings", Some("writer_enabled")),
            (
                "export_destination",
                "product_settings",
                Some("export_destination"),
            ),
            (
                "personalization_enabled",
                "personalization",
                Some("enabled"),
            ),
            ("personalization_text", "personalization", Some("text")),
            ("research_policy", "research_policy", None),
        ] {
            let local = self.raw.get(parent);
            let shared = artifact.and_then(|a| a.settings().get(parent));
            let declares = |value: Option<&Value>| {
                value.is_some_and(|v| child.is_none_or(|c| !v.is_object() || v.get(c).is_some()))
            };
            let origin = if declares(shared) {
                "shared"
            } else if shared.is_some_and(Value::is_object) && local.is_none_or(|v| !v.is_object()) {
                "default"
            } else if declares(local) {
                "local"
            } else {
                "default"
            };
            origins.insert(field.into(), Value::String(origin.into()));
        }
        view["origins"] = Value::Object(origins);
        Ok(view)
    }
}

#[cfg(test)]
mod tests;
