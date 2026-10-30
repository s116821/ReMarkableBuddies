use super::*;
use remarkable_reader_buddy::{
    conversation::receiving::memory::*,
    memory::{self as domain, Provider as MemoryProvider},
};
use std::{collections::BTreeSet, sync::Mutex};

fn options() -> MemoryOptions {
    MemoryOptions {
        now_ms: 100,
        current_claims: vec![],
        max_matches: 8,
        max_context_bytes: 16384,
    }
}
fn combined_budget() -> CombinedBudget {
    CombinedBudget {
        max_encoded_bytes: 524288,
        max_tokens: 1_000_000,
    }
}
fn fact(profile: Uuid, scope: domain::Scope, claim: &str, value: &str) -> domain::Fact {
    domain::Fact {
        key: domain::Key {
            profile,
            scope,
            fact: Uuid::new_v4(),
        },
        claim: claim.into(),
        value: value.into(),
        subject: "acceleration".into(),
        tags: vec![],
        provenance: domain::Provenance {
            basis: domain::Basis::UserConfirmed,
            description: "Authored fixture assertion; not source verification".into(),
            conversation: None,
            turn: None,
            source: None,
        },
        confidence_basis_points: 9000,
        observed_ms: 10,
        expires_ms: None,
    }
}
fn put(memory: &domain::LocalMemory, fact: domain::Fact) {
    memory
        .put(domain::Mutation {
            operation: Uuid::new_v4(),
            expected: BTreeSet::new(),
            fact,
        })
        .unwrap();
}
fn blank(ledger: &Ledger, conversation: Uuid, request: Uuid) -> FrozenRequestContext {
    ledger
        .source_request_context(
            conversation,
            request,
            None,
            RequestSource::NoDocument,
            budget(),
            None,
            counter,
        )
        .unwrap()
}
struct MemorySpy {
    inner: domain::LocalMemory,
    calls: Mutex<Vec<domain::Query>>,
    damage: u8,
    effect: Option<Box<dyn Fn() + Send + Sync>>,
}
impl MemorySpy {
    fn new(ledger: &Ledger, profile: Uuid) -> Self {
        Self {
            inner: domain::LocalMemory::new_local(ledger.store().clone(), profile).unwrap(),
            calls: Mutex::new(vec![]),
            damage: 0,
            effect: None,
        }
    }
    fn input(&self, profile: Uuid) -> MemoryInput<'_> {
        MemoryInput::Query {
            provider: self,
            profile,
            options: options(),
        }
    }
}
impl MemoryProvider for MemorySpy {
    fn put(&self, mutation: domain::Mutation) -> Result<Uuid> {
        self.inner.put(mutation)
    }
    fn forget(&self, mutation: domain::Forget) -> Result<Uuid> {
        self.inner.forget(mutation)
    }
    fn inspect(&self, key: &domain::Key) -> Result<Vec<Envelope>> {
        self.inner.inspect(key)
    }
    fn query(&self, query: &domain::Query) -> Result<domain::QueryResult> {
        self.calls.lock().unwrap().push(query.clone());
        let mut result = self.inner.query(query)?;
        if let Some(effect) = &self.effect {
            effect();
        }
        match self.damage {
            0 => {}
            1 => result.context.kind = "source-evidence".into(),
            2 => result.context.provenance_authority = "verified".into(),
            3 => result.context.facts[0].fact.key.profile = Uuid::new_v4(),
            4 => {
                result.context.facts[0].fact.key.scope = domain::Scope::Document {
                    document: Uuid::new_v4(),
                }
            }
            5 => result.context.facts[0].record = Uuid::new_v4(),
            6 => result.context.facts[0].revision = Uuid::nil(),
            7 => result.context.facts[0].fact.expires_ms = Some(100),
            8 => result.context.facts.push(result.context.facts[0].clone()),
            9 => result.status = domain::Status::CompleteEmpty,
            10 => result.status = domain::Status::Disabled,
            11 => result.status = domain::Status::Unavailable,
            12 => result.diagnostics.snapshot_unavailable = true,
            13 => result.diagnostics.unsupported = 1,
            14 => result.status = domain::Status::Partial,
            15 => result.context.facts[0].fact.value = "x".repeat(8 * 1024 * 1024),
            16 => result.context.facts = vec![result.context.facts[0].clone(); 10000],
            17 => result.context.facts[0].fact.claim = "Non Canonical".into(),
            18 => result.context.facts[0].fact.value = "escaped \"\n".repeat(300),
            19 => {
                result.context.facts[0].fact.claim = "suppressed".into();
            }
            20 => bail!("private provider path/query must not escape"),
            _ => unreachable!(),
        }
        Ok(result)
    }
}

#[test]
fn bound_query_preserves_exact_frame_and_uses_only_original_document_and_profile() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let (request, source) = with_source(
        &ledger,
        user(conversation, "café acceleration \"units\""),
        book,
    );
    let index = Index::build(document(book, "v1")).unwrap();
    let retrieval = Spy::new(&index);
    let memory = MemorySpy::new(&ledger, profile);
    put(
        &memory.inner,
        fact(
            profile,
            domain::Scope::Global,
            "global units",
            "acceleration SI",
        ),
    );
    put(
        &memory.inner,
        fact(
            profile,
            domain::Scope::Document { document: book },
            "document units",
            "acceleration m/s²",
        ),
    );
    put(
        &memory.inner,
        fact(
            profile,
            domain::Scope::Document {
                document: Uuid::new_v4(),
            },
            "other book",
            "acceleration secret",
        ),
    );
    let other_profile = Uuid::new_v4();
    let foreign = domain::LocalMemory::new_local(ledger.store().clone(), other_profile).unwrap();
    put(
        &foreign,
        fact(
            other_profile,
            domain::Scope::Global,
            "foreign profile",
            "acceleration secret",
        ),
    );
    let frozen = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&retrieval),
            counter,
        )
        .unwrap();
    let original = frozen.encoded_json().to_owned();
    let before = snapshot(&fixture.0);
    let combined = frozen
        .with_memory(memory.input(profile), combined_budget(), counter)
        .unwrap();
    assert_eq!(combined.source_context().encoded_json(), original);
    assert!(combined
        .encoded_json()
        .contains(&format!("\"request_context\":{original},")));
    assert_eq!(combined.source_context().request(), &request);
    assert_eq!(combined.memory().context().unwrap().facts.len(), 2);
    assert_eq!(combined.memory().status(), domain::Status::Complete);
    let calls = memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].text, request.text.unwrap());
    assert_eq!(calls[0].document, Some(book));
    assert_eq!(combined.memory().query().unwrap().document, Some(book));
    assert_eq!(combined.counted_tokens(), combined.encoded_json().len());
    assert_eq!(retrieval.calls.borrow().len(), 1);
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn disabled_and_no_document_global_only_and_large_question_incompatibility() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let profile = Uuid::new_v4();
    let memory = MemorySpy::new(&ledger, profile);
    put(
        &memory.inner,
        fact(
            profile,
            domain::Scope::Global,
            "global units",
            "acceleration SI",
        ),
    );
    put(
        &memory.inner,
        fact(
            profile,
            domain::Scope::Document {
                document: Uuid::new_v4(),
            },
            "book units",
            "acceleration secret",
        ),
    );
    let disabled = blank(&ledger, conversation, request.id)
        .with_memory(MemoryInput::Disabled, combined_budget(), counter)
        .unwrap();
    assert_eq!(disabled.memory().status(), domain::Status::Disabled);
    assert!(disabled.memory().context().is_none());
    assert!(disabled.memory().query().is_none());
    assert!(memory.calls.lock().unwrap().is_empty());
    let enabled = blank(&ledger, conversation, request.id)
        .with_memory(memory.input(profile), combined_budget(), counter)
        .unwrap();
    assert_eq!(enabled.memory().query().unwrap().document, None);
    assert_eq!(enabled.memory().context().unwrap().facts.len(), 1);
    for text in ["é".repeat(1024), format!("{}x", "é".repeat(1024))] {
        let conversation = create(&ledger);
        let request = append(&ledger, user(conversation, &text), vec![]);
        let start = memory.calls.lock().unwrap().len();
        let result = blank(&ledger, conversation, request.id).with_memory(
            memory.input(profile),
            combined_budget(),
            counter,
        );
        if text.len() == 2048 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.unwrap_err().downcast_ref::<CompositionRefusal>(),
                Some(&CompositionRefusal::IncompatibleRequest)
            );
            assert_eq!(memory.calls.lock().unwrap().len(), start);
            assert!(blank(&ledger, conversation, request.id)
                .with_memory(MemoryInput::Disabled, combined_budget(), counter)
                .is_ok());
        }
    }
}

#[test]
fn real_conflict_expiry_suppression_and_budget_diagnostics_stay_distinct() {
    for variant in 0..5 {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let conversation = create(&ledger);
        let request = append(&ledger, user(conversation, "acceleration"), vec![]);
        let profile = Uuid::new_v4();
        let memory = MemorySpy::new(&ledger, profile);
        let mut opts = options();
        if variant != 0 {
            let mut first = fact(profile, domain::Scope::Global, "units", "acceleration SI");
            if variant == 1 {
                first.expires_ms = Some(100);
            }
            if variant == 2 {
                opts.current_claims = vec!["units".into()];
            }
            put(&memory.inner, first);
            if variant >= 3 {
                put(
                    &memory.inner,
                    fact(
                        profile,
                        domain::Scope::Global,
                        if variant == 3 { "units" } else { "other units" },
                        "acceleration different",
                    ),
                );
            }
            if variant == 4 {
                opts.max_matches = 1;
            }
        }
        let combined = blank(&ledger, conversation, request.id)
            .with_memory(
                MemoryInput::Query {
                    provider: &memory,
                    profile,
                    options: opts,
                },
                combined_budget(),
                counter,
            )
            .unwrap();
        let report = combined.memory();
        if variant < 3 {
            assert_eq!(report.status(), domain::Status::CompleteEmpty);
            assert!(report.context().unwrap().facts.is_empty());
        } else {
            assert_eq!(report.status(), domain::Status::Partial);
        }
        match variant {
            1 => assert_eq!(report.diagnostics().expired, 1),
            2 => assert_eq!(report.diagnostics().suppressed, 1),
            3 => {
                assert_eq!(report.diagnostics().contradicted_claims, 1);
                assert!(report.context().unwrap().facts.is_empty());
            }
            4 => {
                assert_eq!(report.diagnostics().budget_omitted, 1);
                assert_eq!(report.context().unwrap().facts.len(), 1);
            }
            _ => {}
        }
    }
}

#[test]
fn complete_byte_and_token_boundaries_and_preflight_calls_are_exact() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "acceleration \"x\"\n"), vec![]);
    let profile = Uuid::new_v4();
    let memory = MemorySpy::new(&ledger, profile);
    put(
        &memory.inner,
        fact(
            profile,
            domain::Scope::Global,
            "units",
            "acceleration \"SI\"\n",
        ),
    );
    let build = || blank(&ledger, conversation, request.id);
    let full = build()
        .with_memory(memory.input(profile), combined_budget(), counter)
        .unwrap();
    let exact = CombinedBudget {
        max_encoded_bytes: full.encoded_json().len(),
        max_tokens: full.counted_tokens(),
    };
    assert_eq!(
        build()
            .with_memory(memory.input(profile), exact.clone(), counter)
            .unwrap()
            .encoded_json(),
        full.encoded_json()
    );
    let before = snapshot(&fixture.0);
    let mut less = exact.clone();
    less.max_encoded_bytes -= 1;
    assert_eq!(
        build()
            .with_memory(memory.input(profile), less, counter)
            .unwrap_err()
            .downcast_ref::<CompositionRefusal>(),
        Some(&CompositionRefusal::EncodedLimit)
    );
    let mut less = exact.clone();
    less.max_tokens -= 1;
    assert!(matches!(
        build()
            .with_memory(memory.input(profile), less, counter)
            .unwrap_err()
            .downcast_ref::<CompositionRefusal>(),
        Some(CompositionRefusal::TokenLimit { .. })
    ));
    for budget in [
        CombinedBudget {
            max_encoded_bytes: 512,
            max_tokens: 1000000,
        },
        CombinedBudget {
            max_encoded_bytes: 524288,
            max_tokens: 0,
        },
    ] {
        let calls = memory.calls.lock().unwrap().len();
        let counted = Cell::new(0);
        assert!(build()
            .with_memory(memory.input(profile), budget, |_| {
                counted.set(counted.get() + 1);
                Ok(1)
            })
            .is_err());
        assert_eq!(memory.calls.lock().unwrap().len(), calls);
        assert_eq!(counted.get(), 0);
    }
    for zero in [false, true] {
        let err = build()
            .with_memory(memory.input(profile), exact.clone(), |_| {
                if zero {
                    Ok(0)
                } else {
                    bail!("private counter error")
                }
            })
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<CompositionRefusal>(),
            Some(&CompositionRefusal::CounterUnavailable)
        );
        assert_eq!(err.chain().count(), 1);
    }
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn non_monotonic_counter_only_sees_final_encoding_and_may_reenter_store() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let profile = Uuid::new_v4();
    let memory = MemorySpy::new(&ledger, profile);
    put(
        &memory.inner,
        fact(profile, domain::Scope::Global, "units", "acceleration SI"),
    );
    let frozen = blank(&ledger, conversation, request.id);
    let original = frozen.encoded_json().to_owned();
    let counted = Cell::new(0);
    let mut limit = combined_budget();
    limit.max_tokens = 1;
    let combined = frozen
        .with_memory(memory.input(profile), limit, |text| {
            counted.set(counted.get() + 1);
            let hypothetical_count = if text.contains("\"memory\":null") {
                999999
            } else {
                1
            };
            ledger
                .delete(
                    conversation,
                    Uuid::new_v4(),
                    heads(&ledger, conversation),
                    None,
                )
                .unwrap();
            Ok(hypothetical_count)
        })
        .unwrap();
    assert_eq!(counted.get(), 1);
    assert_eq!(combined.counted_tokens(), 1);
    assert_eq!(combined.source_context().encoded_json(), original);
    assert!(ledger.inspect(conversation, false).is_err());
}

#[test]
fn malformed_custom_results_refuse_before_counter_without_private_error_chains() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let profile = Uuid::new_v4();
    let mut memory = MemorySpy::new(&ledger, profile);
    put(
        &memory.inner,
        fact(profile, domain::Scope::Global, "units", "acceleration SI"),
    );
    let before = snapshot(&fixture.0);
    for damage in 1..=20 {
        memory.damage = damage;
        let mut opts = options();
        if damage == 18 {
            opts.max_context_bytes = 1024;
        }
        if damage == 19 {
            opts.current_claims.push("suppressed".into());
        }
        let counted = Cell::new(0);
        let err = blank(&ledger, conversation, request.id)
            .with_memory(
                MemoryInput::Query {
                    provider: &memory,
                    profile,
                    options: opts,
                },
                combined_budget(),
                |_| {
                    counted.set(1);
                    Ok(1)
                },
            )
            .unwrap_err();
        assert!(
            err.downcast_ref::<CompositionRefusal>().is_some(),
            "damage {damage}: {err:#}"
        );
        assert_eq!(counted.get(), 0, "damage {damage}");
        assert_eq!(err.chain().count(), 1);
        assert!(!format!("{err:#}").contains("private"));
    }
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn invalid_options_refuse_without_query_or_counter_and_source_is_not_rewritten() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let profile = Uuid::new_v4();
    let memory = MemorySpy::new(&ledger, profile);
    for mode in 0..6 {
        let mut opts = options();
        match mode {
            0 => opts.max_matches = 0,
            1 => opts.max_matches = 17,
            2 => opts.max_context_bytes = 255,
            3 => opts.current_claims = vec!["Not Canonical".into()],
            4 => opts.current_claims = vec!["x".into(); 65],
            _ => {}
        }
        let err = blank(&ledger, conversation, request.id)
            .with_memory(
                MemoryInput::Query {
                    provider: &memory,
                    profile: if mode == 5 { Uuid::nil() } else { profile },
                    options: opts,
                },
                combined_budget(),
                |_| panic!("must not count invalid input"),
            )
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<CompositionRefusal>(),
            Some(&CompositionRefusal::InvalidMemoryOptions)
        );
    }
    assert!(memory.calls.lock().unwrap().is_empty());
}

#[test]
fn provider_callback_mutation_retains_historical_source_and_memory_without_locks() {
    let fixture = Fixture::new();
    let ledger = Arc::new(fixture.ledger());
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let index = Index::build(document(book, "v1")).unwrap();
    let retrieval = Spy::new(&index);
    let profile = Uuid::new_v4();
    let mut memory = MemorySpy::new(&ledger, profile);
    put(
        &memory.inner,
        fact(profile, domain::Scope::Global, "units", "acceleration SI"),
    );
    let effect_ledger = ledger.clone();
    memory.effect = Some(Box::new(move || {
        effect_ledger
            .delete(
                conversation,
                Uuid::new_v4(),
                heads(&effect_ledger, conversation),
                None,
            )
            .unwrap();
        let provider =
            domain::LocalMemory::new_local(effect_ledger.store().clone(), profile).unwrap();
        put(
            &provider,
            fact(
                profile,
                domain::Scope::Global,
                "later observation",
                "acceleration changed",
            ),
        );
    }));
    let frozen = ledger
        .source_request_context(
            conversation,
            request.id,
            None,
            association(&request, &source, index.key()),
            budget(),
            Some(&retrieval),
            counter,
        )
        .unwrap();
    let original = frozen.encoded_json().to_owned();
    let combined = frozen
        .with_memory(memory.input(profile), combined_budget(), counter)
        .unwrap();
    assert_eq!(combined.source_context().encoded_json(), original);
    assert_eq!(combined.memory().context().unwrap().facts.len(), 1);
    assert_eq!(memory.calls.lock().unwrap().len(), 1);
    assert_eq!(retrieval.calls.borrow().len(), 1);
    assert_eq!(
        combined.source_context().retrieval().unwrap().source,
        *index.key()
    );
    assert!(ledger.inspect(conversation, false).is_err());
    assert_eq!(
        memory
            .inner
            .query(&domain::Query::new("acceleration".into(), 100))
            .unwrap()
            .context
            .facts
            .len(),
        2
    );
}

#[test]
fn selected_query_result_is_refused_without_dropping_admission_into_ordinary_data() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let request = append(&ledger, user(conversation, "acceleration"), vec![]);
    let frozen = blank(&ledger, conversation, request.id);
    let selected_fixture = Fixture::new();
    let selected_ledger = selected_fixture.ledger();
    let identity = domain::catalog::Identity {
        profile: Uuid::new_v4(),
        group: Uuid::new_v4(),
        binding_sha256: digest(b"fixture binding"),
    };
    let provider =
        domain::selected::SelectedMemory::new(selected_ledger.store().clone(), identity.clone())
            .unwrap();
    let registration = provider
        .initialize_aggregate(domain::selected::InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"aggregate descriptor"),
            fact: fact(
                identity.profile,
                domain::Scope::Global,
                "units",
                "acceleration SI",
            ),
        })
        .unwrap()
        .registration
        .unwrap();
    provider
        .initialize_selection(domain::selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"catalog descriptor"),
            registrations: vec![registration],
        })
        .unwrap();
    let before = snapshot(&selected_fixture.0);
    let err = frozen
        .with_memory(
            MemoryInput::Query {
                provider: &provider,
                profile: identity.profile,
                options: options(),
            },
            combined_budget(),
            |_| panic!("selected results must not reach counter"),
        )
        .unwrap_err();
    assert_eq!(
        err.downcast_ref::<CompositionRefusal>(),
        Some(&CompositionRefusal::SelectedMemory)
    );
    assert_eq!(snapshot(&selected_fixture.0), before);
}

#[test]
fn restart_rebuilds_the_same_historical_combination_without_new_records() {
    let fixture = Fixture::new();
    let profile = Uuid::new_v4();
    let conversation;
    let request;
    let expected;
    {
        let ledger = fixture.ledger();
        conversation = create(&ledger);
        request = append(&ledger, user(conversation, "acceleration"), vec![]).id;
        let memory = MemorySpy::new(&ledger, profile);
        put(
            &memory.inner,
            fact(profile, domain::Scope::Global, "units", "acceleration SI"),
        );
        expected = blank(&ledger, conversation, request)
            .with_memory(memory.input(profile), combined_budget(), counter)
            .unwrap()
            .encoded_json()
            .to_owned();
    }
    let ledger = fixture.ledger();
    let memory = MemorySpy::new(&ledger, profile);
    let before = snapshot(&fixture.0);
    let combined = blank(&ledger, conversation, request)
        .with_memory(memory.input(profile), combined_budget(), counter)
        .unwrap();
    assert_eq!(combined.encoded_json(), expected);
    assert_eq!(snapshot(&fixture.0), before);
}
