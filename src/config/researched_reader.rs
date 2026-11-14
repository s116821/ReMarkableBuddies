//! Request-local destination selection, before a protected credential is read.
//!
//! The receiver still owns source, selected-policy, intent and submission gates.
//! A model selection is not permission to submit a request.
use super::{credential_name, read_secret_file, Config, Secret};
use crate::llm::openai::DEFAULT_MODEL;
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};

const ANSWER_ORIGIN: &str = "https://api.openai.com";
const SEARCH_ENDPOINT: &str = "https://api.openai.com/v1/responses";
const SEARCH_MODEL: &str = "gpt-5.6-terra";

/// Contains no secret and accepts no environment or command-line overrides.
pub(crate) struct Selection {
    model: String,
    credential: PathBuf,
    search_model: Option<String>,
}

impl Selection {
    /// Freeze only non-secret settings. In particular, a missing/unqualified
    /// search model does not prevent independently admitted answer recognition.
    /// The receiver must join the exact Config and source again at secret read.
    pub(crate) fn admit(config: &Config, search_model: Option<&str>) -> Result<Self> {
        config.validate()?;
        ensure!(
            config.base_url.as_deref().unwrap_or(ANSWER_ORIGIN) == ANSWER_ORIGIN,
            "researched Reader answer origin is unavailable"
        );
        let model = config.model.as_deref().unwrap_or(DEFAULT_MODEL);
        ensure!(
            !model.is_empty()
                && model.len() <= 256
                && model == model.trim()
                && !model.chars().any(char::is_control),
            "researched Reader answer model is invalid"
        );
        let name = config
            .model_credential
            .as_deref()
            .context("researched Reader needs a protected model credential")?;
        credential_name(name)?;
        // This validates shape only. Eligibility is deferred until independent
        // intent plus the frozen saved policy actually require web research.
        ensure!(
            search_model.is_none_or(|model| {
                !model.is_empty()
                    && model.len() <= 256
                    && model == model.trim()
                    && !model.chars().any(char::is_control)
            }),
            "researched Reader search declaration is invalid"
        );
        Ok(Self {
            model: model.to_owned(),
            credential: config.paths.credentials.join(name),
            search_model: search_model.map(str::to_owned),
        })
    }

    /// Consume the selection so this request reads the protected secret once.
    pub(crate) fn resolve(self) -> Result<Resolved> {
        self.resolve_with(read_secret_file)
    }

    fn resolve_with(self, read: impl FnOnce(&Path) -> Result<Vec<u8>>) -> Result<Resolved> {
        let bytes = read(&self.credential)?;
        let key =
            String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("invalid selected credential"))?;
        let key = key.trim();
        ensure!(
            !key.is_empty()
                && !key
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control()),
            "selected API credential is empty or invalid"
        );
        Ok(Resolved {
            model: self.model,
            search_model: self.search_model,
            key: Secret(key.to_owned()),
        })
    }
}

/// Deliberately neither Clone nor Serialize: retained only by this request.
pub(crate) struct Resolved {
    model: String,
    search_model: Option<String>,
    key: Secret,
}

impl Resolved {
    pub(crate) fn answer_engine(&self) -> crate::OpenAI {
        crate::OpenAI::new(
            self.model.clone(),
            self.key.expose().to_owned(),
            Some(ANSWER_ORIGIN.to_owned()),
        )
        .with_single_request()
        .with_timeout(std::time::Duration::from_secs(67))
    }

    /// Validate the exact initially qualified search pair before lending the
    /// credential to it. The caller must already own the current request's
    /// independent intent admission and joined selected gates; disabled/no-web
    /// requests must not call this method or construct a search transport.
    pub(crate) fn required_search(&self) -> Result<(&str, &str, &str)> {
        ensure!(
            self.search_model.as_deref() == Some(SEARCH_MODEL),
            "researched Reader search profile is unavailable"
        );
        Ok((SEARCH_MODEL, SEARCH_ENDPOINT, self.key.expose()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn configuration() -> Config {
        Config {
            model: Some("gpt-6.1-terra".into()),
            model_credential: Some("fixture_key".into()),
            ..Config::default()
        }
    }

    #[test]
    fn local_pdf_opt_in_requires_an_object_and_selected_document() {
        use crate::reader_runtime::ResearchedPdf;
        for raw in [
            "[]",
            "[null]",
            r#"{"research_model":null,"research_model":"gpt-5.6-terra"}"#,
            r#"{"unexpected":true}"#,
        ] {
            assert!(serde_json::from_str::<ResearchedPdf>(raw).is_err());
        }
        let mut config = configuration();
        assert!(config.reader.researched_pdf.is_none());
        assert_eq!(
            serde_json::to_value(&config.reader).unwrap(),
            serde_json::json!({"selection": null}),
            "absent opt-in must preserve delivered ordinary typed bytes"
        );
        config.reader.researched_pdf = Some(serde_json::from_str("{}").unwrap());
        assert!(config.validate_values().is_err());
        let raw = serde_json::json!({"reader":{"researched_pdf":[null]}});
        assert!(serde_json::from_value::<Config>(raw).is_err());
    }

    #[test]
    fn answer_destination_is_admitted_before_any_secret_reader() {
        let mut config = configuration();
        for origin in [
            "http://api.openai.com",
            "https://api.openai.com.other.invalid",
            "https://api.openai.com/v1/responses",
            "https://user@api.openai.com",
            "https://api.openai.com/",
        ] {
            config.base_url = Some(origin.into());
            assert!(Selection::admit(&config, Some(SEARCH_MODEL)).is_err());
        }
        config.base_url = None;
        config.model_credential = None;
        assert!(Selection::admit(&config, Some(SEARCH_MODEL)).is_err());
        config.model_credential = Some("../other_key".into());
        assert!(Selection::admit(&config, Some(SEARCH_MODEL)).is_err());
    }

    #[test]
    fn no_search_profile_is_needed_for_answering_and_one_secret_is_read() {
        let config = configuration();
        let calls = Cell::new(0);
        let selected = Selection::admit(&config, None).unwrap();
        let resolved = selected
            .resolve_with(|path| {
                calls.set(calls.get() + 1);
                assert_eq!(path, config.paths.credentials.join("fixture_key"));
                Ok(b"fixture-only-secret\n".to_vec())
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(resolved.model, "gpt-6.1-terra");
        assert!(resolved.required_search().is_err());
        // Constructing the answer model performs no I/O or provider request.
        let _engine = resolved.answer_engine();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn search_validation_is_deferred_and_does_not_infer_answer_model_support() {
        let config = configuration();
        for model in [None, Some("gpt-6.1-terra"), Some("different-model")] {
            let resolved = Selection::admit(&config, model)
                .unwrap()
                .resolve_with(|_| Ok(b"fixture-only-secret".to_vec()))
                .unwrap();
            assert!(resolved.required_search().is_err());
        }
        let resolved = Selection::admit(&config, Some(SEARCH_MODEL))
            .unwrap()
            .resolve_with(|_| Ok(b"fixture-only-secret".to_vec()))
            .unwrap();
        assert_eq!(
            resolved.required_search().unwrap(),
            (SEARCH_MODEL, SEARCH_ENDPOINT, "fixture-only-secret")
        );
    }

    #[test]
    fn failed_or_invalid_credential_read_does_not_retry() {
        for value in [
            Vec::new(),
            b"two tokens".to_vec(),
            b"token\0tail".to_vec(),
            vec![0xff],
        ] {
            let calls = Cell::new(0);
            let result = Selection::admit(&configuration(), None)
                .unwrap()
                .resolve_with(|_| {
                    calls.set(calls.get() + 1);
                    Ok(value)
                });
            assert!(result.is_err());
            assert_eq!(calls.get(), 1);
        }
    }
}
