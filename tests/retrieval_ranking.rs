//! Authored lexical relevance expectations, not a real-user/model evaluation.
use remarkable_reader_buddy::{
    retrieval::{
        disk::DiskCache, Cache, Index, NormalizedDocument, Provider, SourceFormat, SourceUnit,
    },
    storage::{digest, Store, StorePaths, Uuid},
};
use std::collections::BTreeSet;

fn document(texts: &[String]) -> NormalizedDocument {
    NormalizedDocument {
        document: Uuid::new_v4(),
        format: SourceFormat::Notebook,
        source_sha256: digest(b"authored normalized fixture source"),
        extractor_revision: "authored-text-v1".into(),
        units: texts
            .iter()
            .enumerate()
            .map(|(i, text)| SourceUnit {
                page: Some(i as u32 + 1),
                section: None,
                text: text.clone(),
            })
            .collect(),
    }
}
fn index(texts: &[&str]) -> Index {
    Index::build(document(
        &texts.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
    ))
    .unwrap()
}
fn pages(index: &Index, question: &str) -> Vec<u32> {
    let provider: &dyn Provider = index;
    provider
        .query(index.key(), question, 16, 16384, false)
        .unwrap()
        .passages
        .iter()
        .map(|x| x.page.unwrap())
        .collect()
}
fn terms(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|x| !x.is_empty())
        .map(str::to_lowercase)
        .collect()
}
// Retained old policy as an independent order baseline on one-chunk fixtures.
fn overlap(texts: &[String], question: &str) -> Vec<u32> {
    let query = terms(question);
    let mut values: Vec<_> = texts
        .iter()
        .enumerate()
        .filter_map(|(i, text)| {
            let count = terms(text).intersection(&query).count();
            (count > 0).then_some((i as u32 + 1, count))
        })
        .collect();
    values.sort_by_key(|&(page, score)| (std::cmp::Reverse(score), page));
    values.into_iter().map(|(p, _)| p).collect()
}

#[test]
fn rare_subject_term_beats_common_background_under_one_passage_budget() {
    let texts: Vec<String> = [
        "electron overview",
        "tunneling barrier penetration",
        "electron charge",
        "electron mass",
        "electron orbit",
        "electron field",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(overlap(&texts, "electron tunneling")[0], 1);
    let index = Index::build(document(&texts)).unwrap();
    let result = index
        .query(index.key(), "electron tunneling", 1, 4096, false)
        .unwrap();
    assert_eq!(result.passages[0].page, Some(2));
    assert_eq!(result.passages[0].text, texts[1]);
    assert!(result.budget_limited);
}

#[test]
fn focused_technical_explanations_beat_earlier_long_glossary_overlap() {
    let texts = vec![
        format!(
            "capacitance dielectric inductance magnetic resistance dissipation {}",
            "glossary background ".repeat(70)
        ),
        "capacitance dielectric stores electric charge".into(),
        "inductance magnetic stores field energy".into(),
        "resistance dissipation converts current into heat".into(),
    ];
    let index = Index::build(document(&texts)).unwrap();
    for (question, expected) in [
        ("capacitance dielectric", 2),
        ("inductance magnetic", 3),
        ("resistance dissipation", 4),
    ] {
        assert_eq!(overlap(&texts, question)[0], 1);
        assert_eq!(pages(&index, question)[0], expected);
    }
}

#[test]
fn repeated_terms_saturate_instead_of_overriding_the_distinctive_topic() {
    let texts: Vec<String> = vec![
        "charge ".repeat(200),
        "charge dielectric capacitor".into(),
        "charge voltage".into(),
        "charge current".into(),
        "charge particle".into(),
    ];
    let ranked = Index::build(document(&texts)).unwrap();
    assert_eq!(pages(&ranked, "charge dielectric")[0], 2);
    // A second occurrence is useful at equal length; repetitions are not discarded.
    let index = index(&["charge filler filler filler", "charge charge filler filler"]);
    assert_eq!(pages(&index, "charge"), vec![2, 1]);
}

#[test]
fn exact_ties_query_duplicates_and_unicode_lowercase_are_stable() {
    let ranked = index(&["ÉLECTRON 線圈 field", "électron 線圈 field", "unrelated"]);
    assert_eq!(pages(&ranked, "ÉLECTRON 線圈"), vec![1, 2]);
    assert_eq!(pages(&ranked, "électron électron 線圈 線圈"), vec![1, 2]);
    for _ in 0..4 {
        assert_eq!(pages(&ranked, "ÉLECTRON 線圈"), vec![1, 2]);
    }
    // Existing lowercase semantics do not expand ß to ss.
    let index = index(&["Straße", "STRASSE"]);
    assert_eq!(pages(&index, "strasse"), vec![2]);
}

#[test]
fn nonsearchable_chunks_do_not_distort_rank_and_common_only_queries_still_match() {
    let small = index(&["field field filler", "field filler filler"]);
    let padded = index(&[
        "field field filler",
        "field filler filler",
        "  ... \t\n",
        "",
    ]);
    assert_eq!(pages(&small, "field"), vec![1, 2]);
    assert_eq!(pages(&padded, "field"), vec![1, 2]);
    let empty = index(&["  !!!", ""]);
    let result = empty.query(empty.key(), "field", 1, 1024, true).unwrap();
    assert!(result.passages.is_empty() && !result.budget_limited);
    assert!(pages(&small, "unmatched").is_empty());
}

#[test]
fn higher_ranked_escaped_passage_that_cannot_fit_does_not_hide_fitting_match() {
    let texts = vec![
        format!("dielectric {}", "\"\\\t".repeat(500)),
        format!("dielectric {}", "background ".repeat(70)),
    ];
    let index = Index::build(document(&texts)).unwrap();
    // Non-token escaping has no effect on lexical length; first is strongest.
    assert_eq!(pages(&index, "dielectric")[0], 1);
    let result = index
        .query(index.key(), "dielectric", 1, 1536, true)
        .unwrap();
    assert_eq!(result.passages[0].page, Some(2));
    assert!(result.budget_limited);
    assert!(serde_json::to_vec(&result).unwrap().len() <= 1536);
    let only = Index::build(document(&texts[..1])).unwrap();
    assert!(only.query(only.key(), "dielectric", 1, 512, false).is_err());
}

#[test]
fn direct_first_neighbors_deduplicate_and_keep_exact_utf8_ranges() {
    let mut input = document(&[
        format!("{} distinctive {}", "中 ".repeat(700), "文 ".repeat(700)),
        "different page".into(),
    ]);
    input.units[0].section = Some("Authored Unicode".into());
    let index = Index::build(input.clone()).unwrap();
    let result = index
        .query(index.key(), "distinctive", 16, 16384, true)
        .unwrap();
    assert!(result.passages.len() > 1 && result.passages[0].direct_match);
    let mut seen = BTreeSet::new();
    for passage in result.passages {
        assert_eq!(passage.unit, 0);
        assert_eq!(passage.section.as_deref(), Some("Authored Unicode"));
        assert!(seen.insert(passage.range));
        assert_eq!(
            passage.text,
            input.units[0].text[passage.range[0]..passage.range[1]]
        );
    }
}

#[test]
fn maximum_admitted_text_and_query_have_finite_stable_order_and_exact_bounds() {
    let query = (0..64)
        .map(|i| format!("term{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut text = query.clone();
    text.push_str(&" x".repeat((1024 - text.len()) / 2));
    text.push_str(&" ".repeat(1024 - text.len()));
    assert_eq!(text.len(), 1024);
    let texts = vec![text; 4096]; // Exactly4MiB,4096units; all equal finite scores.
    let input = document(&texts);
    let index = Index::build(input.clone()).unwrap();
    let result = index.query(index.key(), &query, 16, 16384, false).unwrap();
    assert!(result.budget_limited && !result.passages.is_empty());
    assert_eq!(result.passages[0].page, Some(1));
    assert!(serde_json::to_vec(&result).unwrap().len() <= 16384);
    assert_eq!(pages(&index, &query), pages(&index, &query));
    assert!(index
        .query(index.key(), &(query + " term64"), 16, 16384, false)
        .is_err());
    let mut too_large = input;
    too_large.units[0].text.push('x');
    assert!(Index::build(too_large).is_err());
}

#[test]
fn source_key_cache_rebuild_and_failed_replacement_preserve_material_identity() {
    let root = std::env::temp_dir().join(format!("buddy-ranking-{}", Uuid::new_v4()));
    let store = Store::open(StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    })
    .unwrap();
    let input = document(&[
        "field filler filler filler".into(),
        "field field filler filler".into(),
    ]);
    let expected = Index::build(input.clone()).unwrap().key().clone();
    assert_eq!(
        expected.normalized_sha256,
        digest(&serde_json::to_vec(&input.units).unwrap())
    );
    {
        let cache = DiskCache::open(&store).unwrap();
        assert_eq!(cache.install(input.clone()).unwrap(), expected);
    }
    {
        let cache = DiskCache::open(&store).unwrap();
        let rebuilt = cache.load(&expected).unwrap().unwrap();
        assert_eq!(rebuilt.key(), &expected);
        assert_eq!(pages(&rebuilt, "field"), vec![2, 1]);
        let mut foreign = expected.clone();
        foreign.source_sha256 = digest(b"changed");
        assert!(rebuilt.query(&foreign, "field", 1, 4096, false).is_err());
        let mut invalid = input.clone();
        invalid.units[0].page = Some(0);
        assert!(cache.install(invalid).is_err());
        assert_eq!(
            pages(&cache.load(&expected).unwrap().unwrap(), "field"),
            vec![2, 1]
        );
    }
    let mut memory = Cache::default();
    memory.install(input.clone()).unwrap();
    let mut changed = input;
    changed.units[1].text = "different subject".into();
    memory.install(changed).unwrap();
    assert!(memory
        .get(expected.document)
        .unwrap()
        .query(&expected, "field", 1, 4096, false)
        .is_err());
    drop(store);
    let resolved = root.canonicalize().unwrap();
    assert_eq!(
        resolved.parent(),
        Some(std::env::temp_dir().canonicalize().unwrap().as_path())
    );
    assert!(resolved
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("buddy-ranking-"));
    std::fs::remove_dir_all(resolved).unwrap();
}
