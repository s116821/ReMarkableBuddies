//! Bounded, inert presentation structure for explicitly opted-in page OCR.
//! Syntax validity and declared coverage do not establish source accuracy.
//!
//! The caller must enforce raw response bytes and duplicate-free strict JSON,
//! then call `json_depth_ok` BEFORE deserializing these recursive DTOs. This
//! module neither interprets tokens nor repairs or confirms model output.
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_SPANS: usize = 64;
const MAX_NODES: usize = 128;
const MAX_KNOWN_BYTES: usize = 4096;
const MAX_TOKEN_BYTES: usize = 256;
const MAX_MATH_DEPTH: usize = 8;
const MAX_JSON_DEPTH: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StructuredPolicyResponse {
    pub pages: [StructuredPolicyPage; 1],
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum StructuredPolicyPage {
    Transcribed {
        page: u32,
        text: String,
    },
    Blank {
        page: u32,
        text: String,
    },
    Unreadable {
        page: u32,
        text: String,
    },
    Partial {
        page: u32,
        spans: Vec<super::PartialSpan>,
    },
    Structured {
        page: u32,
        spans: Vec<StructuredSpan>,
    },
}

// Intentionally no Debug implementations for content-bearing DTOs.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StructuredSpan {
    Known { text: String },
    Uncertain {},
    Math { root: MathNode },
}

/// Restricted Presentation MathML roles, not XML or an evaluation language.
/// `Unknown` is a project-specific opaque child, not a MathML token.
#[derive(Deserialize, Serialize)]
#[serde(tag = "tag", rename_all = "lowercase", deny_unknown_fields)]
pub enum MathNode {
    Unknown {},
    Mi { text: String },
    Mn { text: String },
    Mo { text: String },
    Mtext { text: String },
    Mrow { children: Vec<MathNode> },
    Mfrac { children: Vec<MathNode> },
    Msqrt { children: Vec<MathNode> },
    Mroot { children: Vec<MathNode> },
    Msub { children: Vec<MathNode> },
    Msup { children: Vec<MathNode> },
    Msubsup { children: Vec<MathNode> },
    Munder { children: Vec<MathNode> },
    Mover { children: Vec<MathNode> },
    Munderover { children: Vec<MathNode> },
}

/// Complete means no declared gaps, not verified correctness/completeness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    Complete,
    Partial,
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    known_bytes: usize,
    uncertain: bool,
}

impl Budget {
    fn add_text(&mut self, text: &str, max_bytes: usize) -> Option<()> {
        self.known_bytes = self.known_bytes.checked_add(text.len())?;
        (self.known_bytes <= max_bytes).then_some(())
    }
}

/// Count containers only: the root container is depth1; scalar leaves do not
/// add container depth. Recursion stops at the first container above depth64.
pub(super) fn json_depth_ok(value: &Value) -> bool {
    fn within(value: &Value, depth: usize) -> bool {
        match value {
            Value::Array(values) => {
                depth <= MAX_JSON_DEPTH && values.iter().all(|v| within(v, depth + 1))
            }
            Value::Object(values) => {
                depth <= MAX_JSON_DEPTH && values.values().all(|v| within(v, depth + 1))
            }
            _ => true,
        }
    }
    within(value, 1)
}

/// Validate one page, sharing all counters across its spans and math roots.
/// Requires positive math content and never merges/reorders/repairs spans.
pub(super) fn validate_spans(spans: &[StructuredSpan], max_bytes: usize) -> Option<Coverage> {
    if !(1..=MAX_SPANS).contains(&spans.len()) {
        return None;
    }
    let max_bytes = max_bytes.min(MAX_KNOWN_BYTES);
    let mut budget = Budget::default();
    let mut has_math = false;
    let mut previous_uncertain = false;
    for span in spans {
        match span {
            StructuredSpan::Known { text } => {
                if text.trim().is_empty() || !super::valid_text_controls(text) {
                    return None;
                }
                budget.add_text(text, max_bytes)?;
                previous_uncertain = false;
            }
            StructuredSpan::Uncertain {} => {
                if previous_uncertain {
                    return None;
                }
                budget.uncertain = true;
                previous_uncertain = true;
            }
            StructuredSpan::Math { root } => {
                if !validate_node(root, 1, &mut budget, max_bytes)? {
                    return None;
                }
                has_math = true;
                previous_uncertain = false;
            }
        }
    }
    has_math.then_some(if budget.uncertain {
        Coverage::Partial
    } else {
        Coverage::Complete
    })
}

/// Returns whether a subtree has positive content. A known structural node
/// counts even with unknown children, while grouping unknowns alone does not.
fn validate_node(
    node: &MathNode,
    depth: usize,
    budget: &mut Budget,
    max_bytes: usize,
) -> Option<bool> {
    if depth > MAX_MATH_DEPTH {
        return None;
    }
    budget.nodes = budget.nodes.checked_add(1)?;
    if budget.nodes > MAX_NODES {
        return None;
    }
    let (children, arity, mut positive) = match node {
        MathNode::Unknown {} => {
            budget.uncertain = true;
            return Some(false);
        }
        MathNode::Mi { text }
        | MathNode::Mn { text }
        | MathNode::Mo { text }
        | MathNode::Mtext { text } => {
            if text.trim().is_empty()
                || text.len() > MAX_TOKEN_BYTES
                || text.chars().any(char::is_control)
            {
                return None;
            }
            budget.add_text(text, max_bytes)?;
            return Some(true);
        }
        MathNode::Mrow { children } => (children, None, false),
        MathNode::Msqrt { children } => (children, Some(1), true),
        MathNode::Mfrac { children }
        | MathNode::Mroot { children }
        | MathNode::Msub { children }
        | MathNode::Msup { children }
        | MathNode::Munder { children }
        | MathNode::Mover { children } => (children, Some(2), true),
        MathNode::Msubsup { children } | MathNode::Munderover { children } => {
            (children, Some(3), true)
        }
    };
    if match arity {
        Some(arity) => children.len() != arity,
        None => !(2..=32).contains(&children.len()),
    } {
        return None;
    }
    for child in children {
        // Always validate every child; do not short-circuit after positive=true.
        positive |= validate_node(child, depth + 1, budget, max_bytes)?;
    }
    Some(positive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spans(value: Value) -> Vec<StructuredSpan> {
        assert!(json_depth_ok(&value));
        serde_json::from_value(value).expect("test spans")
    }

    fn math(root: Value) -> Vec<StructuredSpan> {
        spans(json!([{"kind":"math","root":root}]))
    }

    fn unknown() -> Value {
        json!({"tag":"unknown"})
    }

    fn sqrt(child: Value) -> Value {
        json!({"tag":"msqrt","children":[child]})
    }

    fn token(text: &str) -> Value {
        json!({"tag":"mi","text":text})
    }

    #[test]
    fn opaque_children_preserve_roles_and_exact_inert_bytes() {
        // Authored representability control: no claim that a model saw this.
        let bytes = r#"[{"kind":"math","root":{"tag":"mfrac","children":[{"tag":"unknown"},{"tag":"msub","children":[{"tag":"msqrt","children":[{"tag":"unknown"}]},{"tag":"unknown"}]}]}}]"#;
        let value: Vec<StructuredSpan> = serde_json::from_str(bytes).expect("bounded test input");
        assert_eq!(validate_spans(&value, 4096), Some(Coverage::Partial));
        assert_eq!(serde_json::to_string(&value).expect("serialize"), bytes);
        let mut budget = Budget::default();
        if let StructuredSpan::Math { root } = &value[0] {
            assert_eq!(validate_node(root, 1, &mut budget, 4096), Some(true));
        } else {
            panic!("expected math");
        }
        assert_eq!(budget.nodes, 6);
        assert_eq!(budget.known_bytes, 0);
    }

    #[test]
    fn known_wrong_symbols_and_wrong_attachments_remain_valid_syntax() {
        let relations = math(json!({"tag":"mrow","children":[
            {"tag":"mo","text":"="},{"tag":"mo","text":"≤"}]}));
        assert_eq!(validate_spans(&relations, 4096), Some(Coverage::Complete));
        // Wrong for the retained B source, but structurally valid in general.
        let internal_script = math(json!({"tag":"mfrac","children":[unknown(),
            sqrt(json!({"tag":"msub","children":[unknown(),unknown()]}))]}));
        assert_eq!(
            validate_spans(&internal_script, 4096),
            Some(Coverage::Partial)
        );
        let external_to_fraction = math(json!({"tag":"msub","children":[
            {"tag":"mfrac","children":[unknown(),sqrt(unknown())]},unknown()]}));
        assert_eq!(
            validate_spans(&external_to_fraction, 4096),
            Some(Coverage::Partial)
        );
        // A syntax-only validator intentionally has no source/gold oracle.
    }

    #[test]
    fn closed_nodes_spans_and_response_variants_reject_extra_payloads() {
        for root in [
            json!({"tag":"unknown","text":"guessed"}),
            json!({"tag":"unknown","children":[unknown()]}),
            json!({"tag":"mi","text":"d","confidence":0.9}),
            json!({"tag":"msqrt","children":[unknown()],"style":"display:none"}),
            json!({"tag":"annotation-xml","children":[unknown()]}),
        ] {
            assert!(serde_json::from_value::<MathNode>(root).is_err());
        }
        assert!(
            serde_json::from_value::<StructuredSpan>(json!({"kind":"uncertain","text":"d"}))
                .is_err()
        );
        assert!(serde_json::from_value::<StructuredSpan>(
            json!({"kind":"known","text":"d","root":unknown()})
        )
        .is_err());
        assert!(
            serde_json::from_value::<StructuredPolicyResponse>(json!({"pages":[],"extra":0}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<StructuredPolicyResponse>(json!({"pages":[
            {"status":"blank","page":1,"text":""},{"status":"blank","page":2,"text":""}]}))
            .is_err()
        );
        assert!(
            serde_json::from_value::<StructuredPolicyResponse>(json!({"pages":[
            {"status":"structured","page":1,"spans":[],"coverage":"complete"}]}))
            .is_err()
        );
        for page in [
            json!({"status":"transcribed","page":1,"text":"d"}),
            json!({"status":"blank","page":1,"text":""}),
            json!({"status":"unreadable","page":1,"text":""}),
            json!({"status":"partial","page":1,"spans":[{"kind":"known","text":"d"},{"kind":"uncertain"}]}),
            json!({"status":"structured","page":1,"spans":[{"kind":"math","root":sqrt(unknown())}]}),
        ] {
            assert!(
                serde_json::from_value::<StructuredPolicyResponse>(json!({"pages":[page]})).is_ok()
            );
        }
    }

    #[test]
    fn every_structural_tag_has_exact_arity_and_checks_every_child() {
        for (tag, arity) in [
            ("msqrt", 1),
            ("mfrac", 2),
            ("mroot", 2),
            ("msub", 2),
            ("msup", 2),
            ("munder", 2),
            ("mover", 2),
            ("msubsup", 3),
            ("munderover", 3),
        ] {
            for size in [arity - 1, arity, arity + 1] {
                let root = json!({"tag":tag,"children":vec![unknown();size]});
                assert_eq!(validate_spans(&math(root), 4096).is_some(), size == arity);
            }
            let mut children = vec![unknown(); arity];
            children[arity - 1] = token("");
            assert_eq!(
                validate_spans(&math(json!({"tag":tag,"children":children})), 4096),
                None
            );
        }
        for size in [0, 1, 2, 32, 33] {
            let root = json!({"tag":"mrow","children":vec![token("x");size]});
            assert_eq!(
                validate_spans(&math(root), 4096).is_some(),
                (2..=32).contains(&size)
            );
        }
    }

    #[test]
    fn opaque_only_grouping_is_not_positive_but_real_structure_is() {
        assert_eq!(validate_spans(&math(unknown()), 4096), None);
        assert_eq!(
            validate_spans(
                &math(json!({"tag":"mrow","children":[unknown(),unknown()]})),
                4096
            ),
            None
        );
        assert_eq!(
            validate_spans(&math(sqrt(unknown())), 4096),
            Some(Coverage::Partial)
        );
        assert_eq!(
            validate_spans(&spans(json!([{"kind":"known","text":"d"}])), 4096),
            None
        );
        assert_eq!(
            validate_spans(&math(token("d")), 4096),
            Some(Coverage::Complete)
        );
    }

    #[test]
    fn node_budget_is_shared_between_roots_and_unknown_counts_once() {
        let value = json!({"kind":"math","root":sqrt(unknown())});
        assert_eq!(
            validate_spans(&spans(Value::Array(vec![value.clone(); 64])), 4096),
            Some(Coverage::Partial)
        );
        let mut values = vec![value; 64];
        values[63] = json!({"kind":"math","root":sqrt(sqrt(unknown()))});
        assert_eq!(validate_spans(&spans(Value::Array(values)), 4096), None);
        assert_eq!(validate_spans(&[], 4096), None);
        assert_eq!(
            validate_spans(
                &spans(Value::Array(vec![
                    json!({"kind":"math","root":token("x")});
                    65
                ])),
                4096
            ),
            None
        );
    }

    #[test]
    fn known_text_budget_is_shared_and_cannot_be_widened_by_caller() {
        let value = json!({"kind":"math","root":token(&"x".repeat(256))});
        let mut values = vec![value; 16];
        assert_eq!(
            validate_spans(&spans(Value::Array(values.clone())), 4096),
            Some(Coverage::Complete)
        );
        assert_eq!(
            validate_spans(&spans(Value::Array(values.clone())), 4095),
            None
        );
        values.push(json!({"kind":"known","text":"x"}));
        assert_eq!(
            validate_spans(&spans(Value::Array(values)), usize::MAX),
            None
        );
        assert_eq!(
            validate_spans(&math(token(&"λ".repeat(128))), 4096),
            Some(Coverage::Complete)
        );
        assert_eq!(validate_spans(&math(token(&"λ".repeat(129))), 4096), None);
    }

    #[test]
    fn page_gaps_controls_and_unicode_bytes_remain_explicit() {
        let value = spans(json!([
            {"kind":"known","text":"before\r\n\tafter"},
            {"kind":"uncertain"}, {"kind":"math","root":token("∈")}
        ]));
        assert_eq!(validate_spans(&value, 4096), Some(Coverage::Partial));
        assert_eq!(
            validate_spans(
                &spans(json!([
                    {"kind":"uncertain"},{"kind":"uncertain"},{"kind":"math","root":token("x")}
                ])),
                4096
            ),
            None
        );
        for text in ["", " \n\t", "d\n", "d\r", "d\t", "d\u{7f}", "d\u{85}"] {
            assert_eq!(validate_spans(&math(token(text)), 4096), None);
        }
        assert_eq!(
            validate_spans(
                &spans(json!([
                    {"kind":"known","text":"x\u{0}"},{"kind":"math","root":token("x")}
                ])),
                4096
            ),
            None
        );
        let bytes = r#"[{"kind":"math","root":{"tag":"mi","text":"<script>&λé"}}]"#;
        let value: Vec<StructuredSpan> = serde_json::from_str(bytes).expect("literal token");
        assert_eq!(validate_spans(&value, 4096), Some(Coverage::Complete));
        assert_eq!(serde_json::to_string(&value).expect("serialize"), bytes);
        // These characters are inert strings; no HTML/XML/Unicode repair occurs.
    }

    #[test]
    fn math_depth_eight_is_inclusive() {
        let mut root = unknown();
        for _ in 0..7 {
            root = sqrt(root);
        }
        assert_eq!(
            validate_spans(&math(root.clone()), 4096),
            Some(Coverage::Partial)
        );
        assert_eq!(validate_spans(&math(sqrt(root)), 4096), None);
    }

    #[test]
    fn json_container_depth_is_checked_before_recursive_dto_construction() {
        for object in [false, true] {
            let mut value = Value::Null;
            for _ in 0..64 {
                value = if object {
                    json!({"x":value})
                } else {
                    json!([value])
                };
            }
            assert!(json_depth_ok(&value));
            value = if object {
                json!({"x":value})
            } else {
                json!([value])
            };
            assert!(!json_depth_ok(&value));
        }
    }
}
