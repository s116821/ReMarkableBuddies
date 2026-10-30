use super::*;
use crate::retrieval::{SourceFormat, SourceUnit};

fn document(texts: &[&str]) -> NormalizedDocument {
    NormalizedDocument {
        document: Uuid::from_u128(1),
        format: SourceFormat::Notebook,
        source_sha256: "a".repeat(64),
        extractor_revision: "test-v1".into(),
        units: texts
            .iter()
            .enumerate()
            .map(|(i, text)| SourceUnit {
                page: Some(i as u32 + 1),
                section: None,
                text: (*text).to_owned(),
            })
            .collect(),
    }
}
fn source(texts: &[&str]) -> ValidatedNotebook {
    let raw = serde_json::to_vec(&document(texts)).unwrap();
    ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap()
}
fn assert_coverage(source: &ValidatedNotebook, plan: &Coverage, expected_units: &[usize]) {
    let mut actual = Vec::new();
    let mut bytes = 0;
    for (ordinal, chunk) in plan.chunks().iter().enumerate() {
        assert_eq!(chunk.ordinal(), ordinal);
        let chunk_bytes: usize = chunk.authoritative().iter().map(Segment::bytes).sum();
        assert!(chunk_bytes <= MAX_RANGE_BYTES);
        bytes += chunk_bytes;
        for segment in chunk.authoritative() {
            let unit = segment.unit();
            let text = source.text(segment).unwrap();
            assert_eq!(
                text.as_bytes(),
                &source.document().units[unit].text.as_bytes()[segment.1..segment.2]
            );
            if let Some((prior, end)) = actual.last_mut() {
                if *prior == unit {
                    assert_eq!(
                        *end, segment.1,
                        "gapped or overlapping authoritative coverage"
                    );
                    *end = segment.2;
                    continue;
                }
                assert!(*prior < unit, "out-of-order original unit ordinal");
            }
            assert_eq!(segment.1, 0);
            actual.push((unit, segment.2));
        }
        for context in [chunk.before_context(), chunk.after_context()] {
            assert!(context.iter().map(Segment::bytes).sum::<usize>() <= MAX_CONTEXT_BYTES);
            for segment in context {
                assert!(expected_units.contains(&segment.unit()));
                assert!(!source.text(segment).unwrap().is_empty());
            }
        }
    }
    assert_eq!(
        actual.iter().map(|(unit, _)| *unit).collect::<Vec<_>>(),
        expected_units
    );
    for (unit, end) in actual {
        assert_eq!(end, source.document().units[unit].text.len());
    }
    assert_eq!(bytes, plan.selected_bytes());
    assert_eq!(expected_units.len(), plan.selected_units());
    assert_eq!(plan.required_calls(), plan.chunks().len() + 1);
}

#[test]
fn whole_units_and_duplicate_text_keep_distinct_original_coverage() {
    let s = source(&["same", "", "same", "third"]);
    let p = s.coverage(&TextScope::SuppliedNotebook, 2).unwrap();
    assert_eq!(p.chunks().len(), 1);
    assert_coverage(&s, &p, &[0, 1, 2, 3]);
    assert_eq!(
        p.chunks()[0].identity_recipe(),
        json!([
            "map",
            0,
            [[0, 0, 4], [1, 0, 0], [2, 0, 4], [3, 0, 5]],
            [],
            []
        ])
    );
}

#[test]
fn page_scope_retains_full_archive_and_never_borrows_unselected_context() {
    let body = "🦕".repeat(9000);
    let s = source(&["private excluded page", &body, "another excluded page"]);
    let p = s.coverage(&TextScope::Page { page: 2 }, 3).unwrap();
    assert_coverage(&s, &p, &[1]);
    assert_eq!(
        serde_json::from_slice::<NormalizedDocument>(s.canonical_bytes())
            .unwrap()
            .units
            .len(),
        3
    );
    assert!(p
        .chunks()
        .iter()
        .flat_map(|c| c.before_context().iter().chain(c.after_context()))
        .all(|r| r.unit() == 1));
}

#[test]
fn prefers_whole_units_and_paragraph_boundaries_with_exact_utf8_fallback() {
    let paragraph = format!("{}\n\n{}", "x".repeat(20_000), "é".repeat(18_000));
    let s = source(&["small preceding unit", &paragraph]);
    let p = s.coverage(&TextScope::SuppliedNotebook, 5).unwrap();
    assert_coverage(&s, &p, &[0, 1]);
    assert_eq!(p.chunks()[0].authoritative(), &[Segment(0, 0, 20)]);
    assert_eq!(p.chunks()[1].authoritative(), &[Segment(1, 0, 20_002)]);
    assert_eq!(p.chunks()[2].authoritative()[0].range(), [20_002, 52_770]);
}

#[test]
fn exact_selected_text_and_call_limits_refuse_without_truncation() {
    let full = "x".repeat(MAX_SELECTED_BYTES);
    let s = source(&[&full]);
    let p = s.coverage(&TextScope::SuppliedNotebook, MAX_CALLS).unwrap();
    assert_eq!(p.chunks().len(), MAX_MAPS);
    assert_coverage(&s, &p, &[0]);
    assert!(s
        .coverage(&TextScope::SuppliedNotebook, MAX_CALLS - 1)
        .is_err());
    let over = source(&[&(full + "x")]);
    assert!(over
        .coverage(&TextScope::SuppliedNotebook, MAX_CALLS)
        .is_err());
    for invalid in [0, 1, MAX_CALLS + 1] {
        assert!(source(&["one"])
            .coverage(&TextScope::SuppliedNotebook, invalid)
            .is_err());
    }
}

#[test]
fn empty_units_do_not_create_a_paid_empty_map() {
    let full = "x".repeat(MAX_RANGE_BYTES + 1);
    let s = source(&["", &full, ""]);
    let p = s.coverage(&TextScope::SuppliedNotebook, 3).unwrap();
    assert_eq!(p.chunks().len(), 2);
    assert!(p
        .chunks()
        .iter()
        .all(|c| c.authoritative().iter().map(Segment::bytes).sum::<usize>() > 0));
    assert_coverage(&s, &p, &[0, 1, 2]);
    assert!(source(&["", "  "])
        .coverage(&TextScope::SuppliedNotebook, 9)
        .is_err());
}

#[test]
fn selected_unit_limit_and_missing_page_are_explicit_refusals() {
    let texts = vec!["x"; MAX_SELECTED_UNITS + 1];
    let s = source(&texts);
    assert!(s.coverage(&TextScope::SuppliedNotebook, MAX_CALLS).is_err());
    assert!(s.coverage(&TextScope::Page { page: 0 }, MAX_CALLS).is_err());
    assert!(s
        .coverage(&TextScope::Page { page: 999 }, MAX_CALLS)
        .is_err());
    let allowed = source(&texts[..MAX_SELECTED_UNITS]);
    assert_coverage(
        &allowed,
        &allowed.coverage(&TextScope::SuppliedNotebook, 2).unwrap(),
        &(0..256).collect::<Vec<_>>(),
    );
}

#[test]
fn multi_scalar_boundaries_context_and_determinism_preserve_every_byte() {
    for seed in 0..12 {
        let text = format!(
            "{}{}{}",
            "a".repeat(32_765 + seed),
            "🦕é中\r\n".repeat(500),
            "z".repeat(seed)
        );
        let s = source(&[&text]);
        let p = s.coverage(&TextScope::SuppliedNotebook, 9).unwrap();
        assert_coverage(&s, &p, &[0]);
        assert_eq!(p, s.coverage(&TextScope::SuppliedNotebook, 9).unwrap());
        for pair in p.chunks().windows(2) {
            let a = pair[0].authoritative().last().unwrap();
            let b = pair[1].authoritative().first().unwrap();
            assert_eq!(a.2, b.1);
            assert_eq!(pair[0].after_context()[0].1, a.2);
            assert_eq!(pair[1].before_context().last().unwrap().2, b.1);
        }
    }
}

#[test]
fn raw_hash_and_canonical_retention_are_distinct_and_failures_are_sanitized() {
    let raw = serde_json::to_vec(&document(&["retained"])).unwrap();
    let mut decorated = raw.clone();
    decorated.push(b'\n');
    let s = ValidatedNotebook::from_bytes(&decorated, &digest(&decorated)).unwrap();
    assert_eq!(s.canonical_bytes(), raw);
    assert_eq!(s.raw_bytes(), raw.len() + 1);
    assert_ne!(s.raw_sha256(), digest(s.canonical_bytes()));
    assert!(ValidatedNotebook::from_bytes(&decorated, &digest(&raw)).is_err());
    assert!(ValidatedNotebook::from_bytes(&raw, &"A".repeat(64)).is_err());
    let malformed = br#"{"secret":"do not echo me"}"#;
    let error = ValidatedNotebook::from_bytes(malformed, &digest(malformed))
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "invalid normalized notebook JSON");
    let mut max = raw.clone();
    max.resize(MAX_CANONICAL_BYTES, b' ');
    assert!(ValidatedNotebook::from_bytes(&max, &digest(&max)).is_ok());
    max.push(b' ');
    assert!(ValidatedNotebook::from_bytes(&max, &digest(&max)).is_err());
}

#[test]
fn central_literal_unicode_source_identity_vectors_are_unchanged() {
    // Literal central Docs vectors970489; never regenerate from production code.
    let v: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/writer_notebook_identity_vectors.json"
    ))
    .unwrap();
    let raw = v["raw_document_utf8"].as_str().unwrap().as_bytes();
    let s = ValidatedNotebook::from_bytes(raw, v["raw_document_sha256"].as_str().unwrap()).unwrap();
    assert_eq!(
        digest(s.canonical_bytes()),
        v["canonical_document_sha256"].as_str().unwrap()
    );
    assert_eq!(s.units_sha256(), v["units_sha256"].as_str().unwrap());
    for (name, scope) in [
        ("source", TextScope::SuppliedNotebook),
        ("page_guidance_source", TextScope::Page { page: 1 }),
    ] {
        let fixture = &v[name];
        let input = fixture["input"].as_array().unwrap();
        let identity = s
            .source_identity(
                Uuid::parse_str(input[1].as_str().unwrap()).unwrap(),
                Uuid::parse_str(input[2].as_str().unwrap()).unwrap(),
                &scope,
                input.last().unwrap().as_str(),
            )
            .unwrap();
        assert_eq!(
            identity.source_identity_digest,
            fixture["sha256"].as_str().unwrap()
        );
        assert_eq!(
            identity.archive_id.to_string(),
            fixture["archive_id"].as_str().unwrap()
        );
    }
}

#[test]
fn source_identity_binds_original_turn_scope_and_guidance() {
    let s = source(&["one", "two"]);
    let conversation = Uuid::from_u128(2);
    let turn = Uuid::from_u128(3);
    let a = s
        .source_identity(conversation, turn, &TextScope::SuppliedNotebook, None)
        .unwrap();
    for b in [
        s.source_identity(Uuid::from_u128(4), turn, &TextScope::SuppliedNotebook, None)
            .unwrap(),
        s.source_identity(
            conversation,
            Uuid::from_u128(4),
            &TextScope::SuppliedNotebook,
            None,
        )
        .unwrap(),
        s.source_identity(conversation, turn, &TextScope::Page { page: 1 }, None)
            .unwrap(),
        s.source_identity(
            conversation,
            turn,
            &TextScope::SuppliedNotebook,
            Some("summarize"),
        )
        .unwrap(),
    ] {
        assert_ne!(a, b);
    }
    assert!(s
        .source_identity(Uuid::nil(), turn, &TextScope::SuppliedNotebook, None)
        .is_err());
    assert!(s
        .source_identity(conversation, turn, &TextScope::SuppliedNotebook, Some(" "))
        .is_err());
    // A range belonging to another source cannot panic a source reader.
    assert!(s.text(&Segment(999, 0, 1)).is_err());
    assert!(s.text(&Segment(0, 0, 999)).is_err());
}
