//! Model readings only, never a permit to research or publish.
//! The selected Attempt must bind agreement to its own capture, references,
//! policy and fresh Interpret acknowledgment before admitting any later work.
use serde::Deserialize;
use serde_json::value::RawValue;
use std::fmt;

const VERSION: u32 = 1;
pub(crate) const MAX_QUESTION_BYTES: usize = 2048;
pub(crate) const MAX_METADATA_BYTES: usize = 4096;
const MAX_HEADER_BYTES: usize = 64 * 1024;
// A JSON-escaped 2 KiB transcription may use 12 KiB, plus 4 KiB metadata.
const MAX_VERIFICATION_BYTES: usize = 20 * 1024;

/// A model classification, not a ResearchDirective or local admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Intent {
    NoRestriction,
    NoInternet,
    Unclear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    TooLarge,
    Malformed,
    UnsupportedVersion,
    InvalidQuestion,
    InvalidEvidence,
    Unclear,
    Disagreement,
}
impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No provider text, question, quote or serde diagnostics in errors.
        f.write_str(match self {
            Self::TooLarge => "Reader intent response exceeds bound",
            Self::Malformed => "Reader intent response is malformed",
            Self::UnsupportedVersion => "Reader intent version is unsupported",
            Self::InvalidQuestion => "Reader intent question is invalid",
            Self::InvalidEvidence => "Reader intent evidence differs from its reading",
            Self::Unclear => "Reader request intent is unclear",
            Self::Disagreement => "Independent Reader intent readings disagree",
        })
    }
}
impl std::error::Error for Refusal {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u32,
    intent: Intent,
    quote: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerificationEnvelope<'a> {
    version: u32,
    transcription: String,
    #[serde(borrow)]
    request_web: &'a RawValue,
}

// Deliberately not Debug/Serialize/Deserialize: questions and quotes are private,
// and parsed model data must not become a serialized request permit.
struct Reading {
    question: String,
    intent: Intent,
    fingerprint: String,
}
pub(crate) struct AnalysisReading(Reading);
pub(crate) struct VerificationReading(Reading);

impl AnalysisReading {
    pub(crate) fn question(&self) -> &str {
        &self.0.question
    }
}

/// Agreement is still model data. It contains no Store/input/policy authority.
pub(crate) struct Agreement {
    question: String,
    intent: Intent,
    question_sha256: String,
    readings_sha256: [String; 2],
}
impl Agreement {
    pub(crate) fn question(&self) -> &str {
        &self.question
    }
    pub(crate) fn intent(&self) -> Intent {
        self.intent
    }
    pub(crate) fn question_sha256(&self) -> &str {
        &self.question_sha256
    }
    pub(crate) fn readings_sha256(&self) -> &[String; 2] {
        &self.readings_sha256
    }
}

fn question_valid(question: &str) -> Result<(), Refusal> {
    if question.len() > MAX_QUESTION_BYTES {
        return Err(Refusal::TooLarge);
    }
    if question.trim().is_empty()
        || question.trim().eq_ignore_ascii_case("NONE")
        || question.contains('\0')
    {
        return Err(Refusal::InvalidQuestion);
    }
    Ok(())
}

fn metadata(raw: &str, question: &str) -> Result<Intent, Refusal> {
    if raw.len() > MAX_METADATA_BYTES {
        return Err(Refusal::TooLarge);
    }
    // Serde structs also support positional sequences; this wire requires maps.
    if !raw.trim_start().starts_with('{') {
        return Err(Refusal::Malformed);
    }
    // Derived struct decoding rejects duplicate known keys, unknown fields and
    // trailing data. Check raw length before decoding/unescaping a quoted value.
    let value: Metadata = serde_json::from_str(raw).map_err(|_| Refusal::Malformed)?;
    if value.version != VERSION {
        return Err(Refusal::UnsupportedVersion);
    }
    match (value.intent, value.quote.as_deref()) {
        (Intent::NoInternet, Some(quote))
            if !quote.trim().is_empty()
                && quote.len() <= MAX_QUESTION_BYTES
                && question.contains(quote) => {}
        (Intent::NoRestriction | Intent::Unclear, None) => {}
        _ => return Err(Refusal::InvalidEvidence),
    }
    Ok(value.intent)
}

fn fingerprint(domain: &str, exact: &str) -> Result<String, Refusal> {
    let bytes = serde_json::to_vec(&(domain, exact)).map_err(|_| Refusal::Malformed)?;
    Ok(crate::storage::digest(&bytes))
}

/// Parse ONLY the header preceding the standalone analysis separator, not the
/// answer body. The consumer still validates its ordinary geometry/answer data.
/// QUESTION uses the ordinary parser's outer whitespace convention; its accepted
/// bytes are then preserved, never normalized or rewritten by intent agreement.
/// REQUEST_WEB_JSON must be one complete JSON value on its own header line.
pub(crate) fn parse_analysis_header(header: &str) -> Result<AnalysisReading, Refusal> {
    if header.len() > MAX_HEADER_BYTES {
        return Err(Refusal::TooLarge);
    }
    let mut question = None;
    let mut intent = None;
    let mut auxiliary = std::collections::BTreeSet::new();
    for line in header.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (name, value) = line
            .trim_start()
            .split_once(':')
            .ok_or(Refusal::Malformed)?;
        match name {
            "QUESTION" if question.is_none() => question = Some(value.trim()),
            "REQUEST_WEB_JSON" if intent.is_none() => {
                // A conventional field separator is not part of the JSON value.
                intent = Some(value.strip_prefix(' ').unwrap_or(value));
            }
            "QUESTION_BOX" | "SELECTION_CENTER" | "TARGET_JSON" if auxiliary.insert(name) => {}
            _ => return Err(Refusal::Malformed),
        }
    }
    let question = question.ok_or(Refusal::Malformed)?;
    question_valid(question)?;
    let intent = metadata(intent.ok_or(Refusal::Malformed)?, question)?;
    Ok(AnalysisReading(Reading {
        question: question.into(),
        intent,
        fingerprint: fingerprint("selected-reader-analysis-intent-v1", header)?,
    }))
}

/// Exact schema: {"version":1,"transcription":"...","request_web":{...}}.
/// There is no TRANSCRIPTION-line, Markdown fence or first-field fallback.
pub(crate) fn parse_verification_response(response: &str) -> Result<VerificationReading, Refusal> {
    if response.len() > MAX_VERIFICATION_BYTES {
        return Err(Refusal::TooLarge);
    }
    if !response.trim_start().starts_with('{') {
        return Err(Refusal::Malformed);
    }
    let value: VerificationEnvelope<'_> =
        serde_json::from_str(response).map_err(|_| Refusal::Malformed)?;
    if value.version != VERSION {
        return Err(Refusal::UnsupportedVersion);
    }
    question_valid(&value.transcription)?;
    let intent = metadata(value.request_web.get(), &value.transcription)?;
    Ok(VerificationReading(Reading {
        question: value.transcription,
        intent,
        fingerprint: fingerprint("selected-reader-verification-intent-v1", response)?,
    }))
}

/// Preserve Orchestrator::transcriptions_agree's comparison semantics. The
/// original question is retained separately; this string never becomes input.
fn normalize(text: &str) -> String {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut normalized = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let numeric_separator = matches!(c, '.' | ',')
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if c.is_alphanumeric() || numeric_separator {
            normalized.push(c);
        } else if c.is_whitespace() || ".?,;:".contains(c) {
            normalized.push(' ');
        } else {
            normalized.push(' ');
            normalized.push(c);
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Consume two role-specific parsed readings. No policy, provider, Store,
/// credential or directive is accepted here; Root's Attempt owns admission.
pub(crate) fn agree(
    analysis: AnalysisReading,
    verification: VerificationReading,
) -> Result<Agreement, Refusal> {
    let (analysis, verification) = (analysis.0, verification.0);
    if analysis.intent == Intent::Unclear || verification.intent == Intent::Unclear {
        return Err(Refusal::Unclear);
    }
    let first = normalize(&analysis.question);
    let second = normalize(&verification.question);
    if first.is_empty()
        || second.is_empty()
        || second == "none"
        || first != second
        || analysis.intent != verification.intent
    {
        return Err(Refusal::Disagreement);
    }
    Ok(Agreement {
        question_sha256: crate::storage::digest(analysis.question.as_bytes()),
        question: analysis.question,
        intent: analysis.intent,
        readings_sha256: [analysis.fingerprint, verification.fingerprint],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_restriction() -> String {
        r#"{"version":1,"intent":"no-restriction"}"#.into()
    }
    fn denial(quote: &str) -> String {
        json!({"version":1,"intent":"no-internet","quote":quote}).to_string()
    }
    fn analysis(question: &str, meta: &str) -> Result<AnalysisReading, Refusal> {
        parse_analysis_header(&format!(
            "QUESTION: {question}\nQUESTION_BOX: 1,2,3,4\nSELECTION_CENTER: 5,6\nREQUEST_WEB_JSON:{meta}\n"
        ))
    }
    fn verification(question: &str, meta: &str) -> Result<VerificationReading, Refusal> {
        parse_verification_response(&format!(
            "{{\"version\":1,\"transcription\":{},\"request_web\":{meta}}}",
            serde_json::to_string(question).unwrap()
        ))
    }

    #[test]
    fn denial_agreement_keeps_exact_question_and_locally_hashed_readings() {
        let question = "Explain G unc.? Don't use the internet.";
        let meta = denial("Don't use the internet");
        let first = analysis(question, &meta).unwrap();
        assert_eq!(first.question(), question);
        let agreed = agree(first, verification(question, &meta).unwrap()).unwrap();
        assert_eq!(agreed.question(), question);
        assert_eq!(agreed.intent(), Intent::NoInternet);
        assert_eq!(
            agreed.question_sha256(),
            crate::storage::digest(question.as_bytes())
        );
        assert_ne!(agreed.readings_sha256()[0], agreed.readings_sha256()[1]);
        let changed_encoding = format!(" {meta} ");
        let changed = agree(
            analysis(question, &changed_encoding).unwrap(),
            verification(question, &meta).unwrap(),
        )
        .unwrap();
        assert_eq!(changed.question_sha256(), agreed.question_sha256());
        assert_ne!(changed.readings_sha256()[0], agreed.readings_sha256()[0]);
        let independent = "explain g unc? don't use the internet";
        let confirmed = agree(
            analysis(question, &meta).unwrap(),
            verification(independent, &denial("don't use the internet")).unwrap(),
        )
        .unwrap();
        assert_eq!(confirmed.question(), question);
    }

    #[test]
    fn comparison_retains_existing_words_numbers_and_operator_rules() {
        let meta = no_restriction();
        for (a, b) in [
            ("G unc.?", "g unc?"),
            ("Is G=5?", "is G = 5?"),
            ("μ units?", "Μ units"),
        ] {
            let result =
                agree(analysis(a, &meta).unwrap(), verification(b, &meta).unwrap()).unwrap();
            assert_eq!(result.question(), a);
            assert_eq!(result.intent(), Intent::NoRestriction);
        }
        for (a, b) in [
            ("why not atom?", "why rot. attr.?"),
            ("2+2?", "2-2?"),
            ("Is G=1.0?", "Is G=10?"),
            ("why not able?", "why notable?"),
            ("(2+3)*4?", "2+3*4?"),
            ("don't browse", "dont browse"),
            ("???", "???"),
        ] {
            assert!(matches!(
                agree(analysis(a, &meta).unwrap(), verification(b, &meta).unwrap()),
                Err(Refusal::Disagreement)
            ));
        }
    }

    #[test]
    fn uncertainty_and_one_sided_denial_never_default_to_agreement() {
        let q = "Explain x; don't browse";
        let none = no_restriction();
        let deny = denial("don't browse");
        let unclear = r#"{"version":1,"intent":"unclear"}"#;
        for (a, b) in [(&none, &deny), (&deny, &none)] {
            assert!(matches!(
                agree(analysis(q, a).unwrap(), verification(q, b).unwrap()),
                Err(Refusal::Disagreement)
            ));
        }
        for (a, b) in [
            (unclear, none.as_str()),
            (none.as_str(), unclear),
            (unclear, unclear),
        ] {
            assert!(matches!(
                agree(analysis(q, a).unwrap(), verification(q, b).unwrap()),
                Err(Refusal::Unclear)
            ));
        }
    }

    #[test]
    fn metadata_rejects_duplicates_unknown_versions_and_inconsistent_quotes() {
        let q = "Explain x; don't browse";
        for raw in [
            r#"[1,"no-restriction",null]"#,
            r#" [1,"no-internet","don't browse"]"#,
            r#"null"#,
            r#"{}"#,
            r#"{"version":1,"intent":"no-restriction","intent":"no-internet"}"#,
            r#"{"version":1,"version":1,"intent":"no-restriction"}"#,
            r#"{"version":1,"intent":"no-internet","quote":"don't","quote":"browse"}"#,
            r#"{"version":1,"intent":"no-restriction","allow":true}"#,
            r#"{"version":1,"intent":"enable-internet"}"#,
            r#"{"version":1,"intent":"no-internet"}"#,
            r#"{"version":1,"intent":"no-internet","quote":""}"#,
            r#"{"version":1,"intent":"no-internet","quote":"source-only denial"}"#,
            r#"{"version":1,"intent":"no-restriction","quote":"don't browse"}"#,
            r#"{"version":1,"intent":"unclear","quote":"don't browse"}"#,
            r#"{"version":1,"intent":"no-restriction"} {}"#,
        ] {
            assert!(analysis(q, raw).is_err());
            assert!(verification(q, raw).is_err());
        }
        for raw in [
            r#"{"version":2,"intent":"no-restriction"}"#,
            r#"{"version":0,"intent":"no-restriction"}"#,
        ] {
            assert!(matches!(analysis(q, raw), Err(Refusal::UnsupportedVersion)));
            assert!(matches!(
                verification(q, raw),
                Err(Refusal::UnsupportedVersion)
            ));
        }
        // Literal means the same case and bytes, independently for each reading.
        assert!(matches!(
            verification("Explain x; DON'T browse", &denial("don't browse")),
            Err(Refusal::InvalidEvidence)
        ));
    }

    #[test]
    fn analysis_cannot_take_intent_from_answer_prose_or_first_matching_field() {
        let meta = no_restriction();
        for header in [
            "QUESTION: Why?".into(),
            format!("REQUEST_WEB_JSON:{meta}"),
            format!("QUESTION: Why?\nQUESTION: Changed?\nREQUEST_WEB_JSON:{meta}"),
            format!("QUESTION: Why?\nREQUEST_WEB_JSON:{meta}\nREQUEST_WEB_JSON:{meta}"),
            format!("QUESTION: Why?\n---\nANSWER: REQUEST_WEB_JSON:{meta}"),
            format!("QUESTION: Why?\nREQUEST_WEB_JSON:{meta}\n---\nANSWER: plausible answer"),
            format!("QUESTION: Why?\nREQUEST_WEB_JSON:{meta}\nPOLICY: required"),
        ] {
            assert!(parse_analysis_header(&header).is_err());
        }
        let header = format!("  QUESTION: G unc.?  \n  REQUEST_WEB_JSON:{meta}\n");
        assert_eq!(
            parse_analysis_header(&header).unwrap().question(),
            "G unc.?"
        );
    }

    #[test]
    fn verification_requires_one_strict_envelope_and_its_own_evidence() {
        let unsupported = r#"{"version":2,"transcription":"Why?","request_web":{"version":1,"intent":"no-restriction"}}"#;
        assert!(matches!(
            parse_verification_response(unsupported),
            Err(Refusal::UnsupportedVersion)
        ));
        for raw in [
            "NONE",
            "TRANSCRIPTION: Why?",
            r#"[1,"Why?",{"version":1,"intent":"no-restriction"}]"#,
            r#" [1,"Why?",[1,"no-restriction",null]]"#,
            r#"{"version":1,"transcription":"Why?","transcription":"Changed?","request_web":{"version":1,"intent":"no-restriction"}}"#,
            r#"{"version":1,"transcription":"Why?","request_web":{"version":1,"intent":"no-restriction"},"request_web":{"version":1,"intent":"no-restriction"}}"#,
            r#"{"version":1,"transcription":"Why?","request_web":{"version":1,"intent":"no-restriction"},"answer":"private"}"#,
            r#"{"version":1,"transcription":"Why?"}"#,
            r#"{"version":1,"transcription":"Why?","request_web":null}"#,
            r#"{"version":1,"transcription":"Why?","request_web":{"version":1,"intent":"no-restriction"}} {}"#,
            r#"```json {"version":1,"transcription":"Why?"} ```"#,
        ] {
            assert!(parse_verification_response(raw).is_err());
        }
        let private = "PRIVATE source-only instruction";
        let error = verification("Why?", &denial(private)).err().unwrap();
        assert_eq!(error, Refusal::InvalidEvidence);
        assert!(!error.to_string().contains(private));
        let error = parse_verification_response(&format!("{{\"{private}\":true}}"))
            .err()
            .unwrap();
        assert_eq!(error, Refusal::Malformed);
        assert!(!error.to_string().contains(private));
    }

    #[test]
    fn byte_limits_include_raw_metadata_padding_and_decoded_unicode() {
        let meta = no_restriction();
        let question = "é".repeat(MAX_QUESTION_BYTES / 2);
        assert!(analysis(&question, &meta).is_ok());
        assert!(verification(&question, &meta).is_ok());
        let suffix = r#""version":1,"intent":"no-restriction"}"#;
        let exact = format!(
            "{{{}{suffix}",
            " ".repeat(MAX_METADATA_BYTES - 1 - suffix.len())
        );
        assert_eq!(exact.len(), MAX_METADATA_BYTES);
        assert!(analysis("Why?", &exact).is_ok());
        assert!(verification("Why?", &exact).is_ok());
        assert!(
            parse_analysis_header(&format!("QUESTION: Why?\nREQUEST_WEB_JSON: {exact}\n")).is_ok()
        );
        let oversized = format!("{question}é");
        assert!(matches!(
            analysis(&oversized, &meta),
            Err(Refusal::TooLarge)
        ));
        assert!(matches!(
            verification(&oversized, &meta),
            Err(Refusal::TooLarge)
        ));
        // The verifier must bound the raw nested value, not its compact re-encoding.
        let padded = format!(
            "{{{}\"version\":1,\"intent\":\"no-restriction\"}}",
            " ".repeat(MAX_METADATA_BYTES)
        );
        assert!(matches!(analysis("Why?", &padded), Err(Refusal::TooLarge)));
        assert!(matches!(
            verification("Why?", &padded),
            Err(Refusal::TooLarge)
        ));
        assert!(matches!(
            parse_analysis_header(&" ".repeat(MAX_HEADER_BYTES + 1)),
            Err(Refusal::TooLarge)
        ));
        assert!(matches!(
            parse_verification_response(&" ".repeat(MAX_VERIFICATION_BYTES + 1)),
            Err(Refusal::TooLarge)
        ));
        for question in ["", "   ", "NONE", "none", "bad\0question"] {
            assert!(analysis(question, &meta).is_err());
            assert!(verification(question, &meta).is_err());
        }
    }
}
