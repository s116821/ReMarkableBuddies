//! Every research Provider below is authored synthetic data. No network is used.
use super::*;
use remarkable_reader_buddy::{
    conversation::receiving::memory::{self as cm, research as rr},
    memory as domain,
};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
};

type Reply<'a> = dyn Fn(&rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> + 'a;
struct Synthetic<'a> {
    calls: Cell<usize>,
    reply: Box<Reply<'a>>,
}
impl<'a> Synthetic<'a> {
    fn new(reply: impl Fn(&rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> + 'a) -> Self {
        Self {
            calls: Cell::new(0),
            reply: Box::new(reply),
        }
    }
}
impl rr::Provider for Synthetic<'_> {
    fn research(&self, request: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
        self.calls.set(self.calls.get() + 1);
        (self.reply)(request)
    }
}
fn options() -> rr::ResearchOptions {
    rr::ResearchOptions {
        now_ms: 1000,
        max_queries: 4,
        max_sources: 8,
        max_evidence_bytes: 16384,
    }
}
fn input(provider: Option<&dyn rr::Provider>) -> rr::ResearchInput<'_> {
    rr::ResearchInput {
        policy: rr::ResearchPolicy::default(),
        directive: rr::ResearchDirective::default(),
        provider,
        options: options(),
    }
}
fn combined() -> cm::CombinedBudget {
    cm::CombinedBudget {
        max_encoded_bytes: 524288,
        max_tokens: 1_000_000,
    }
}
fn citation() -> rr::Citation {
    rr::Citation {
        url: "https://example.invalid/authored".into(),
        title: "Authored synthetic fixture".into(),
        excerpt: "Unverified authored excerpt café \"units\"\n".into(),
        published_ms: Some(10),
    }
}
fn response(request: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
    Ok(rr::ResearchResult {
        identity: request.identity.clone(),
        status: rr::Status::Complete,
        basis: rr::EvidenceBasis::Synthetic,
        diagnostic: None,
        queries: vec!["authored query café".into()],
        sources: vec![citation()],
    })
}
fn context(ledger: &Ledger, question: &str) -> cm::FrozenSourceMemoryContext {
    let conversation = create(ledger);
    let turn = append(ledger, user(conversation, question), vec![]);
    ledger
        .source_request_context(
            conversation,
            turn.id,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap()
        .with_memory(cm::MemoryInput::Disabled, combined(), counter)
        .unwrap()
}
fn refusal(value: Result<rr::FrozenResearchContext>) -> rr::ResearchRefusal {
    value
        .unwrap_err()
        .downcast::<rr::ResearchRefusal>()
        .unwrap()
}

#[test]
fn default_requires_one_provider_attempt_and_retains_exact_prior_bytes() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let prior = context(&ledger, "  café acceleration?\n");
    let old = prior.encoded_json().to_owned();
    let source = prior.source_context().encoded_json().to_owned();
    let provider = Synthetic::new(|r| {
        assert_eq!(r.context.encoded_json(), old);
        assert_eq!(r.identity.context_sha256, digest(old.as_bytes()));
        assert_eq!(
            r.context.source_context().request().text.as_deref(),
            Some("  café acceleration?\n")
        );
        assert_eq!(r.options.now_ms, 1000);
        response(r)
    });
    let before = snapshot(&fixture.0);
    let output = prior
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    assert_eq!(provider.calls.get(), 1);
    assert_eq!(output.research().policy(), rr::ResearchPolicy::Required);
    assert_eq!(output.research().status(), rr::Status::Complete);
    assert_eq!(
        output.research().basis(),
        Some(rr::EvidenceBasis::Synthetic)
    );
    assert_eq!(output.source_memory_context().encoded_json(), old);
    assert_eq!(
        output
            .source_memory_context()
            .source_context()
            .encoded_json(),
        source
    );
    assert_eq!(output.counted_tokens(), output.encoded_json().len());
    assert_eq!(snapshot(&fixture.0), before);
    let value: serde_json::Value = serde_json::from_str(output.encoded_json()).unwrap();
    assert_eq!(
        value["context"],
        serde_json::from_str::<serde_json::Value>(&old).unwrap()
    );
    assert_eq!(
        value["research"]["authority"],
        "provider-asserted-unverified"
    );
}

#[test]
fn missing_error_and_unavailable_never_become_offline_or_empty_success() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    assert_eq!(
        refusal(context(&ledger, "question").with_research(input(None), combined(), counter)),
        rr::ResearchRefusal::Unavailable
    );
    for failing in [true, false] {
        let provider = Synthetic::new(|r| {
            if failing {
                bail!("private path URL question must not escape")
            }
            let mut result = response(r)?;
            result.status = rr::Status::Unavailable;
            Ok(result)
        });
        let error = context(&ledger, "question")
            .with_research(input(Some(&provider)), combined(), counter)
            .unwrap_err();
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(error.to_string(), "declared research unavailable");
        assert_eq!(
            *error.downcast_ref::<rr::ResearchRefusal>().unwrap(),
            rr::ResearchRefusal::Unavailable
        );
    }
}

#[test]
fn no_internet_dominates_policy_even_invalid_unused_options_with_zero_calls() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let provider = Synthetic::new(|_| panic!("offline must not call Provider"));
    for policy in [rr::ResearchPolicy::Required, rr::ResearchPolicy::Disabled] {
        let mut request = input(Some(&provider));
        request.policy = policy;
        request.directive = rr::ResearchDirective::NoInternet;
        request.options = rr::ResearchOptions {
            now_ms: 0,
            max_queries: 0,
            max_sources: 0,
            max_evidence_bytes: 0,
        };
        let output = context(&ledger, "explicit opt-out, preserved")
            .with_research(request, combined(), counter)
            .unwrap();
        let report = output.research();
        assert_eq!(report.status(), rr::Status::Disabled);
        assert_eq!(
            report.disabled_reason(),
            Some(rr::DisabledReason::RequestNoInternet)
        );
        assert!(
            report.identity().is_none()
                && report.basis().is_none()
                && report.queries().is_empty()
                && report.sources().is_empty()
        );
    }
    assert_eq!(provider.calls.get(), 0);
}

#[test]
fn configured_disabled_is_distinct_and_does_not_infer_a_request_opt_out() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let mut request = input(None);
    request.policy = rr::ResearchPolicy::Disabled;
    let output = context(&ledger, "question unchanged")
        .with_research(request, combined(), counter)
        .unwrap();
    assert_eq!(
        output.research().disabled_reason(),
        Some(rr::DisabledReason::ConfiguredDisabled)
    );
    assert_eq!(
        output.research().directive(),
        rr::ResearchDirective::UsePolicy
    );
}

#[test]
fn complete_empty_partial_and_provider_asserted_basis_remain_distinct() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    for (status, empty, diagnostic) in [
        (rr::Status::Complete, false, None),
        (rr::Status::CompleteEmpty, true, None),
        (
            rr::Status::Partial,
            true,
            Some(rr::Diagnostic::SourceUnavailable),
        ),
        (
            rr::Status::Partial,
            false,
            Some(rr::Diagnostic::BudgetLimited),
        ),
    ] {
        let provider = Synthetic::new(|r| {
            let mut v = response(r)?;
            v.status = status;
            v.diagnostic = diagnostic;
            if empty {
                v.sources.clear()
            };
            Ok(v)
        });
        let output = context(&ledger, "question")
            .with_research(input(Some(&provider)), combined(), counter)
            .unwrap();
        assert_eq!(output.research().status(), status);
        assert_eq!(output.research().sources().is_empty(), empty);
        assert_eq!(output.research().diagnostic(), diagnostic);
        assert_eq!(output.research().queries(), ["authored query café"]);
    }
    let provider = Synthetic::new(|r| {
        let mut v = response(r)?;
        v.basis = rr::EvidenceBasis::ProviderAsserted;
        Ok(v)
    });
    let output = context(&ledger, "question")
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    assert_eq!(
        output.research().basis(),
        Some(rr::EvidenceBasis::ProviderAsserted)
    );
    assert!(output
        .encoded_json()
        .contains("provider-asserted-unverified"));
}

#[test]
fn foreign_context_conversation_turn_or_observation_time_refuses() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    for damage in 0..4 {
        let provider = Synthetic::new(|r| {
            let mut v = response(r)?;
            match damage {
                0 => v.identity.context_sha256 = digest(b"foreign"),
                1 => v.identity.conversation = Uuid::new_v4(),
                2 => v.identity.request_turn = Uuid::new_v4(),
                _ => v.identity.observed_ms += 1,
            };
            Ok(v)
        });
        let counts = Cell::new(0);
        assert_eq!(
            refusal(context(&ledger, "question").with_research(
                input(Some(&provider)),
                combined(),
                |_| {
                    counts.set(counts.get() + 1);
                    Ok(1)
                }
            )),
            rr::ResearchRefusal::InvalidEvidence
        );
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(counts.get(), 0);
    }
}

#[test]
fn contradictory_outcome_shapes_and_missing_query_claims_refuse() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    for damage in 0..7 {
        let provider = Synthetic::new(|r| {
            let mut v = response(r)?;
            match damage {
                0 => v.status = rr::Status::Disabled,
                1 => v.status = rr::Status::CompleteEmpty,
                2 => v.sources.clear(),
                3 => v.diagnostic = Some(rr::Diagnostic::ProviderFailure),
                4 => v.status = rr::Status::Partial,
                5 => {
                    v.status = rr::Status::CompleteEmpty;
                    v.sources.clear();
                    v.diagnostic = Some(rr::Diagnostic::ProviderFailure)
                }
                _ => v.queries.clear(),
            };
            Ok(v)
        });
        assert_eq!(
            refusal(context(&ledger, "question").with_research(
                input(Some(&provider)),
                combined(),
                counter
            )),
            rr::ResearchRefusal::InvalidEvidence
        );
        assert_eq!(provider.calls.get(), 1);
    }
}

#[test]
fn bounded_untrusted_fields_urls_duplicates_and_dates_refuse_without_repair() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    for damage in 0..24 {
        let provider = Synthetic::new(|r| {
            let mut v = response(r)?;
            match damage {
                0 => v.queries = vec!["q".into(); 5],
                1 => v.sources = vec![citation(); 9],
                2 => v.queries[0] = "q".repeat(2049),
                3 => v.queries[0] = "  \n".into(),
                4 => v.queries[0] = "private\0query".into(),
                5 => v.queries.push(v.queries[0].clone()),
                6 => v.sources[0].title = "x".repeat(513),
                7 => v.sources[0].excerpt = "x".repeat(4097),
                8 => v.sources[0].url = format!("https://example.invalid/{}", "x".repeat(2048)),
                9 => v.sources[0].url = "http://example.invalid/".into(),
                10 => v.sources[0].url = "/relative".into(),
                11 => v.sources[0].url = "https://user@example.invalid/".into(),
                12 => v.sources[0].url = "https://user:password@example.invalid/".into(),
                13 => v.sources[0].url = "https://example.invalid/#fragment".into(),
                14 => v.sources[0].url = " https://example.invalid/".into(),
                15 => v.sources[0].url = "https://example.invalid/\n".into(),
                16 => v.sources[0].published_ms = Some(0),
                17 => v.sources[0].published_ms = Some(1001),
                18 => v.sources[0].title = "private\u{7}text".into(),
                19 => v.sources[0].excerpt = "private\0text".into(),
                20 => v.sources[0].title = "   ".into(),
                21 => v.sources[0].excerpt.clear(),
                22 => v.sources.push(citation()),
                _ => {
                    let mut duplicate = citation();
                    duplicate.url = "https://EXAMPLE.invalid:443/authored".into();
                    v.sources.push(duplicate)
                }
            };
            Ok(v)
        });
        let error = context(&ledger, "question")
            .with_research(input(Some(&provider)), combined(), counter)
            .unwrap_err();
        assert_eq!(
            *error.downcast_ref::<rr::ResearchRefusal>().unwrap(),
            rr::ResearchRefusal::InvalidEvidence,
            "case{damage}"
        );
        assert!(!error.to_string().contains("private"));
        assert_eq!(provider.calls.get(), 1);
    }
}

#[test]
fn complete_evidence_bound_counts_identity_labels_and_escaping_exactly() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let provider = Synthetic::new(|r| {
        let mut v = response(r)?;
        v.sources[0].excerpt = "\"\\\n".repeat(500);
        Ok(v)
    });
    let baseline = context(&ledger, "question")
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    let bytes = serde_json::to_vec(baseline.research()).unwrap().len();
    assert!((256..=16384).contains(&bytes));
    let mut exact = input(Some(&provider));
    exact.options.max_evidence_bytes = bytes;
    assert!(context(&ledger, "question")
        .with_research(exact, combined(), counter)
        .is_ok());
    let mut less = input(Some(&provider));
    less.options.max_evidence_bytes = bytes - 1;
    assert_eq!(
        refusal(context(&ledger, "question").with_research(less, combined(), counter)),
        rr::ResearchRefusal::InvalidEvidence
    );
    assert_eq!(provider.calls.get(), 3);
}

#[test]
fn entire_wrapper_exact_byte_and_token_bounds_never_drop_research() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let provider = Synthetic::new(response);
    let baseline = context(&ledger, "question")
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    let bytes = baseline.encoded_json().len();
    let exact = cm::CombinedBudget {
        max_encoded_bytes: bytes,
        max_tokens: bytes,
    };
    assert!(context(&ledger, "question")
        .with_research(input(Some(&provider)), exact.clone(), counter)
        .is_ok());
    let mut less = exact.clone();
    less.max_encoded_bytes -= 1;
    assert_eq!(
        refusal(context(&ledger, "question").with_research(input(Some(&provider)), less, counter)),
        rr::ResearchRefusal::EncodedLimit
    );
    let mut less = exact;
    less.max_tokens -= 1;
    assert_eq!(
        refusal(context(&ledger, "question").with_research(input(Some(&provider)), less, counter)),
        rr::ResearchRefusal::TokenLimit {
            tokens: bytes,
            limit: bytes - 1
        }
    );
}

#[test]
fn byte_preflight_blocks_provider_but_token_counter_is_final_only_and_nonmonotonic() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let provider = Synthetic::new(response);
    let counts = Cell::new(0);
    let small = cm::CombinedBudget {
        max_encoded_bytes: 512,
        max_tokens: 1,
    };
    assert_eq!(
        refusal(
            context(&ledger, "question").with_research(input(Some(&provider)), small, |_| {
                counts.set(counts.get() + 1);
                Ok(1)
            })
        ),
        rr::ResearchRefusal::EncodedLimit
    );
    assert_eq!(provider.calls.get(), 0);
    assert_eq!(counts.get(), 0);
    let mut one = combined();
    one.max_tokens = 1;
    let output = context(&ledger, "question")
        .with_research(input(Some(&provider)), one, |text| {
            counts.set(counts.get() + 1);
            Ok(if text.contains("\"research\":{") {
                1
            } else {
                usize::MAX
            })
        })
        .unwrap();
    assert_eq!(output.counted_tokens(), 1);
    assert_eq!(provider.calls.get(), 1);
    assert_eq!(counts.get(), 1);
}

#[test]
fn counter_error_zero_and_exhaustion_are_sanitized_without_provider_retry() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    for damage in 0..3 {
        let provider = Synthetic::new(response);
        let calls = Cell::new(0);
        let mut limit = combined();
        limit.max_tokens = 1;
        let error = context(&ledger, "question")
            .with_research(input(Some(&provider)), limit, |_| {
                calls.set(calls.get() + 1);
                match damage {
                    0 => bail!("private counter failure"),
                    1 => Ok(0),
                    _ => Ok(2),
                }
            })
            .unwrap_err();
        assert!(!error.to_string().contains("private"));
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(calls.get(), 1);
        assert_eq!(
            *error.downcast_ref::<rr::ResearchRefusal>().unwrap(),
            if damage < 2 {
                rr::ResearchRefusal::CounterUnavailable
            } else {
                rr::ResearchRefusal::TokenLimit {
                    tokens: 2,
                    limit: 1,
                }
            }
        );
    }
}

#[test]
fn invalid_options_or_combined_budget_prevent_provider_and_counter() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let provider = Synthetic::new(|_| panic!("invalid preflight must not query"));
    for damage in 0..7 {
        let mut request = input(Some(&provider));
        match damage {
            0 => request.options.now_ms = 0,
            1 => request.options.max_queries = 0,
            2 => request.options.max_queries = 5,
            3 => request.options.max_sources = 0,
            4 => request.options.max_sources = 9,
            5 => request.options.max_evidence_bytes = 255,
            _ => request.options.max_evidence_bytes = 16385,
        };
        assert_eq!(
            refusal(
                context(&ledger, "question")
                    .with_research(request, combined(), |_| panic!("no counter"))
            ),
            rr::ResearchRefusal::InvalidOptions
        );
    }
    for budget in [
        cm::CombinedBudget {
            max_encoded_bytes: 511,
            max_tokens: 1,
        },
        cm::CombinedBudget {
            max_encoded_bytes: 524289,
            max_tokens: 1,
        },
        cm::CombinedBudget {
            max_encoded_bytes: 524288,
            max_tokens: 0,
        },
    ] {
        assert_eq!(
            refusal(context(&ledger, "question").with_research(
                input(Some(&provider)),
                budget,
                |_| panic!("no counter")
            )),
            rr::ResearchRefusal::InvalidBudget
        );
    }
    assert_eq!(provider.calls.get(), 0);
}

#[test]
fn maximum_no_document_question_is_forwarded_whole_without_a_query_summary() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let text = "q".repeat(65536);
    let prior = context(&ledger, &text);
    let encoded = prior.encoded_json().to_owned();
    let provider = Synthetic::new(|r| {
        assert_eq!(
            r.context.source_context().request().text.as_ref().unwrap(),
            &text
        );
        assert_eq!(r.context.encoded_json(), encoded);
        response(r)
    });
    let output = prior
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    assert_eq!(
        output
            .source_memory_context()
            .source_context()
            .request()
            .text
            .as_ref()
            .unwrap()
            .len(),
        65536
    );
    assert_eq!(provider.calls.get(), 1);
}

struct CountMemory {
    inner: domain::LocalMemory,
    calls: AtomicUsize,
}
impl domain::Provider for CountMemory {
    fn put(&self, value: domain::Mutation) -> Result<Uuid> {
        domain::Provider::put(&self.inner, value)
    }
    fn forget(&self, value: domain::Forget) -> Result<Uuid> {
        domain::Provider::forget(&self.inner, value)
    }
    fn inspect(&self, value: &domain::Key) -> Result<Vec<Envelope>> {
        domain::Provider::inspect(&self.inner, value)
    }
    fn query(&self, value: &domain::Query) -> Result<domain::QueryResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        domain::Provider::query(&self.inner, value)
    }
}

#[test]
fn actual_index_memory_history_and_source_are_seen_without_new_queries_or_writes() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let history = append(
        &ledger,
        user(conversation, "earlier exact history\n"),
        vec![],
    );
    let book = Uuid::new_v4();
    let (request, source) =
        with_source(&ledger, user(conversation, "  café acceleration?\n"), book);
    let index = Index::build(document(book, "fixture-v1")).unwrap();
    let retrieval = Spy::new(&index);
    let profile = Uuid::new_v4();
    let memory = CountMemory {
        inner: domain::LocalMemory::new_local(ledger.store().clone(), profile).unwrap(),
        calls: AtomicUsize::new(0),
    };
    let fact = domain::Fact {
        key: domain::Key {
            profile,
            scope: domain::Scope::Document { document: book },
            fact: Uuid::new_v4(),
        },
        claim: "fixture-claim".into(),
        value: "acceleration memory assertion".into(),
        subject: "acceleration".into(),
        tags: vec![],
        provenance: domain::Provenance {
            basis: domain::Basis::UserConfirmed,
            description: "authored assertion".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 10,
        expires_ms: None,
    };
    domain::Provider::put(
        &memory,
        domain::Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact,
        },
    )
    .unwrap();
    let prior = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&retrieval),
            counter,
        )
        .unwrap()
        .with_memory(
            cm::MemoryInput::Query {
                provider: &memory,
                profile,
                options: cm::MemoryOptions {
                    now_ms: 100,
                    current_claims: vec![],
                    max_matches: 8,
                    max_context_bytes: 16384,
                },
            },
            combined(),
            counter,
        )
        .unwrap();
    assert_eq!(prior.memory().context().unwrap().facts.len(), 1);
    let old = prior.encoded_json().to_owned();
    let before = snapshot(&fixture.0);
    let provider = Synthetic::new(|r| {
        assert_eq!(r.context.source_context().request(), &request);
        assert_eq!(
            r.context.source_context().history(),
            std::slice::from_ref(&history)
        );
        assert_eq!(
            &r.context.source_context().retrieval().unwrap().source,
            index.key()
        );
        assert_eq!(r.context.memory().profile(), Some(profile));
        assert_eq!(r.context.memory().context().unwrap().facts.len(), 1);
        response(r)
    });
    let output = prior
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    assert_eq!(output.source_memory_context().encoded_json(), old);
    assert_eq!(retrieval.calls.borrow().len(), 1);
    assert_eq!(memory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.get(), 1);
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn reentrant_store_mutation_cannot_replace_captured_context_or_identity() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let prior = context(&ledger, "original exact request");
    let conversation = prior.source_context().request().conversation;
    let old = prior.encoded_json().to_owned();
    let before = snapshot(&fixture.0);
    let provider = Synthetic::new(|r| {
        append(
            &ledger,
            user(conversation, "later callback mutation"),
            vec![],
        );
        assert_eq!(r.context.encoded_json(), old);
        assert_eq!(r.identity.context_sha256, digest(old.as_bytes()));
        response(r)
    });
    let output = prior
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    assert_eq!(output.source_memory_context().encoded_json(), old);
    assert_ne!(snapshot(&fixture.0), before);
    assert!(output
        .source_memory_context()
        .source_context()
        .history()
        .is_empty());
    assert_eq!(provider.calls.get(), 1);
}

#[test]
fn provider_json_cannot_add_a_verified_authority_field_or_unknown_basis() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let provider = Synthetic::new(|r| {
        let value = response(r)?;
        let mut json = serde_json::to_value(&value)?;
        json["authority"] = "source-verified".into();
        assert!(serde_json::from_value::<rr::ResearchResult>(json).is_err());
        let mut json = serde_json::to_value(&value)?;
        json["basis"] = "verified-live".into();
        assert!(serde_json::from_value::<rr::ResearchResult>(json).is_err());
        Ok(value)
    });
    let output = context(&ledger, "question")
        .with_research(input(Some(&provider)), combined(), counter)
        .unwrap();
    assert_eq!(
        output.research().basis(),
        Some(rr::EvidenceBasis::Synthetic)
    );
}
