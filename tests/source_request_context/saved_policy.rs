//! Actual saved config/Store/source/memory; research remains authored synthetic data.
use super::*;
use remarkable_reader_buddy::{
    config::{research_settings as saved, Config},
    conversation::receiving::memory::{self as cm, research as rr},
    memory as domain,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn limits() -> cm::CombinedBudget {
    cm::CombinedBudget {
        max_encoded_bytes: 524288,
        max_tokens: 1_000_000,
    }
}
fn frozen(f: &Fixture) -> (cm::FrozenSourceMemoryContext, StorePaths, Turn, Turn) {
    let ledger = f.ledger();
    let conversation = create(&ledger);
    let history = append(
        &ledger,
        user(conversation, "earlier exact history\n"),
        vec![],
    );
    let book = Uuid::new_v4();
    let (request, source) =
        with_source(&ledger, user(conversation, "  café acceleration?\n"), book);
    let index = Index::build(document(book, "authored-policy-source")).unwrap();
    let profile = Uuid::new_v4();
    let memory = domain::LocalMemory::new_local(ledger.store().clone(), profile).unwrap();
    domain::Provider::put(
        &memory,
        domain::Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact: domain::Fact {
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
            },
        },
    )
    .unwrap();
    let context = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&index),
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
            limits(),
            counter,
        )
        .unwrap();
    assert_eq!(context.memory().context().unwrap().facts.len(), 1);
    let paths = ledger.store().paths.clone();
    (context, paths, request, history)
}
fn report(report: saved::Report) -> Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn save(file: &Path, policy: Option<&str>) {
    let current = report(saved::inspect(Some(file.into())));
    let changes = match policy {
        Some(policy) => json!({"research_policy":{"action":"set","value":policy}}),
        None => json!({"research_policy":{"action":"clear"}}),
    };
    let prepare = json!({"research_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],
        "config_path_sha256":current["data"]["config_path_sha256"],"changes":changes});
    let prepared = report(saved::run_stdin(
        saved::Operation::Prepare,
        Some(file.into()),
        &mut serde_json::to_vec(&prepare).unwrap().as_slice(),
    ));
    if prepared["outcome"] == "no-change" {
        return;
    }
    assert_eq!(prepared["outcome"], "prepared");
    let apply = json!({"intent":prepared["data"]["intent"],"changes":changes});
    let applied = report(saved::run_stdin(
        saved::Operation::Apply,
        Some(file.into()),
        &mut serde_json::to_vec(&apply).unwrap().as_slice(),
    ));
    assert_eq!(applied["outcome"], "verified");
    assert_eq!(applied["runtime_activation"], "unverified");
}
struct Research<'a> {
    calls: Cell<usize>,
    request: &'a Turn,
    history: &'a Turn,
    encoded: &'a str,
    fail: bool,
}
impl rr::Provider for Research<'_> {
    fn research(&self, input: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(input.context.encoded_json(), self.encoded);
        assert_eq!(input.context.source_context().request(), self.request);
        assert_eq!(
            input.context.source_context().history(),
            std::slice::from_ref(self.history)
        );
        assert_eq!(input.context.memory().context().unwrap().facts.len(), 1);
        assert!(!input
            .context
            .source_context()
            .retrieval()
            .unwrap()
            .passages
            .is_empty());
        if self.fail {
            bail!("PRIVATE_SYNTHETIC_FAILURE");
        }
        Ok(rr::ResearchResult {
            identity: input.identity.clone(),
            status: rr::Status::CompleteEmpty,
            basis: rr::EvidenceBasis::Synthetic,
            diagnostic: None,
            queries: vec!["authored verification query".into()],
            sources: vec![],
        })
    }
}

#[test]
fn saved_policy_feeds_real_frozen_source_memory_with_exact_history_and_opt_out_priority() {
    for (policy, no_internet, fail) in [
        (None, false, false),
        (Some("required"), false, false),
        (Some("disabled"), false, false),
        (Some("required"), true, false),
        (Some("disabled"), true, false),
        (Some("required"), false, true),
    ] {
        let f = Fixture::new();
        let (prior, paths, request, history) = frozen(&f);
        let file = f.0.join("saved-policy.json");
        // The unset case exercises an actual clear; no Store lease survives frozen().
        fs::write(&file,serde_json::to_vec(&json!({"paths":paths,"research_policy":if policy==Some("disabled"){"required"}else{"disabled"}})).unwrap()).unwrap();
        save(&file, policy);
        let config = Config::load(Some(file)).unwrap();
        let saved = config.research_policy_snapshot().unwrap();
        assert_eq!(
            saved.policy(),
            if policy == Some("disabled") {
                rr::ResearchPolicy::Disabled
            } else {
                rr::ResearchPolicy::Required
            }
        );
        let original = prior.encoded_json().to_owned();
        let before = snapshot(&f.0);
        let research = Research {
            calls: Cell::new(0),
            request: &request,
            history: &history,
            encoded: &original,
            fail,
        };
        let result = prior.with_research(
            rr::ResearchInput {
                policy: saved.policy(),
                directive: if no_internet {
                    rr::ResearchDirective::NoInternet
                } else {
                    rr::ResearchDirective::UsePolicy
                },
                provider: Some(&research),
                options: rr::ResearchOptions {
                    now_ms: 1000,
                    max_queries: 4,
                    max_sources: 8,
                    max_evidence_bytes: 16384,
                },
            },
            limits(),
            counter,
        );
        let skipped = no_internet || policy == Some("disabled");
        assert_eq!(research.calls.get(), usize::from(!skipped));
        if fail {
            let error = result.unwrap_err();
            assert_eq!(
                error.downcast_ref::<rr::ResearchRefusal>(),
                Some(&rr::ResearchRefusal::Unavailable)
            );
            assert!(!format!("{error:#}").contains("PRIVATE_"));
        } else {
            let context = result.unwrap();
            assert_eq!(context.source_memory_context().encoded_json(), original);
            assert_eq!(
                context.research().status(),
                if skipped {
                    rr::Status::Disabled
                } else {
                    rr::Status::CompleteEmpty
                }
            );
            assert_eq!(
                context.research().disabled_reason(),
                if no_internet {
                    Some(rr::DisabledReason::RequestNoInternet)
                } else if skipped {
                    Some(rr::DisabledReason::ConfiguredDisabled)
                } else {
                    None
                }
            );
        }
        assert_eq!(snapshot(&f.0), before);
    }
}
