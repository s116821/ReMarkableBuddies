//! Offline document discovery only; never configuration/runtime admission.
use anyhow::Result;
use std::io::Write;

pub const MAX_OUTPUT: usize = 65_536;
pub const DOCUMENT: &[u8] = include_bytes!("configuration-schema.json");
const _: () = assert!(DOCUMENT.len() <= MAX_OUTPUT);

/// Static bytes only: no stdin, selected config, environment or Store argument.
pub fn write_to(output: &mut impl Write) -> Result<()> {
    output.write_all(DOCUMENT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn document_identity_bounds_and_current_root_coverage_are_independent_of_defaults() {
        let schema: Value = serde_json::from_slice(DOCUMENT).unwrap();
        assert_eq!(
            schema["$id"],
            "urn:remarkable-buddies:configuration-object-schema:v1"
        );
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["additionalProperties"], false);
        assert!(
            DOCUMENT.len() <= MAX_OUTPUT
                && DOCUMENT.ends_with(b"\n")
                && !DOCUMENT.ends_with(b"\n\n")
        );
        assert!(!DOCUMENT.contains(&b'\r'));
        let defaults = serde_json::to_value(super::super::Config::default()).unwrap();
        let mut keys: std::collections::BTreeSet<_> = defaults
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.extend(["research_policy", "product_settings"]);
        assert_eq!(
            keys,
            schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect()
        );
        assert!(schema.get("required").is_none());
        let mut output = Vec::new();
        write_to(&mut output).unwrap();
        assert_eq!(output, DOCUMENT);
    }

    #[test]
    fn shared_original_byte_corpus_exercises_typed_inspector_without_changing_loader() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/configuration-schema-corpus.json"
        ))
        .unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let accepted =
                super::super::inspection::document(case["raw_utf8"].as_str().unwrap().as_bytes())
                    .is_ok();
            assert_eq!(
                accepted,
                case["typed_valid"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
        assert!(serde_json::from_slice::<super::super::Config>(b"[]").is_ok());
    }

    #[test]
    fn a_failed_output_write_is_not_retried_or_replaced_with_another_document() {
        struct Stop(usize);
        impl Write for Stop {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                Err(std::io::Error::other("closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut output = Stop(0);
        assert!(write_to(&mut output).is_err());
        assert_eq!(output.0, 1);
    }
}
