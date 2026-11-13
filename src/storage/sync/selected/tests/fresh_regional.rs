//! Connected real slot, selected Store, and scripted three-submission consumer.
//! Declared historical/lease fixtures confer no native or provider authority.
use super::{corrections::CorrectionFixture, *};
use crate::{
    conversation::*,
    llm::ModelHandoff,
    workflow::resident_supervisor::writer_request::{FixtureSlot, RegionalRequest},
    writer::{self, selected::FreshOperation},
    LLMEngine,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

const CANDIDATE: &str = r#"{"subject":"Left concept","geometry":{"components":[[[0,0],[500000,0],[500000,1000000],[0,1000000]]]},"understanding":{"status":"interpreted"}}"#;
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    None,
    Uncertain,
    InvalidTarget,
    IneligibleFamily,
    CancelIdentify,
    BeforeIdentifyDeadline,
    Identify45,
    Admission60,
    Normalize150,
    Draft240,
    UncertainProvider,
    SourceChanged,
    HandoffBudget,
    FinishBudget,
    Context,
    ContextChanged,
    ForeignParent,
    LostP,
    LostI,
    LostX,
    PolicyChanged,
    Memory,
    MemoryChanged,
    MaskCollision,
    EmptyContext,
    EmptyContextChanged,
}
impl Fault {
    fn context(self) -> bool {
        matches!(
            self,
            Self::Context
                | Self::ContextChanged
                | Self::MaskCollision
                | Self::EmptyContext
                | Self::EmptyContextChanged
        )
    }
    fn success(self) -> bool {
        matches!(
            self,
            Self::None | Self::Context | Self::Memory | Self::EmptyContext
        )
    }
}
struct Model<'a> {
    store: &'a Store,
    scope: &'a SelectionScope,
    operation: Uuid,
    text: Vec<String>,
    images: Vec<String>,
    calls: Vec<(Vec<String>, Vec<String>)>,
    deadlines: Vec<Instant>,
    fault: Fault,
    before: Option<&'a dyn Fn()>,
    canceled: &'a std::cell::Cell<bool>,
}
impl LLMEngine for Model<'_> {
    fn guarded_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(90))
    }
    fn add_text_content(&mut self, text: &str) {
        self.text.push(text.into());
    }
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.into());
        if self.fault == Fault::BeforeIdentifyDeadline && self.calls.is_empty() {
            writer::selected::advance_for_test(Duration::from_secs(61));
        }
    }
    fn clear_content(&mut self) {
        self.text.clear();
        self.images.clear();
    }
    fn execute(&mut self) -> Result<String> {
        panic!("unguarded fresh Writer")
    }
    fn execute_with_deadline(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut ModelHandoff<'_>,
        deadline: Instant,
    ) -> Result<String> {
        self.deadlines.push(deadline);
        progress()?;
        let p = writer::selected::normalization_preparations(self.operation)[1];
        assert!(
            self.store.selected_receipt(self.scope, p)?.is_some(),
            "P before all inference"
        );
        assert_eq!(
            self.store
                .selected_receipt(self.scope, self.operation)?
                .is_some(),
            self.calls.len() == 2,
            "X absent through normalize, owned by draft"
        );
        assert_eq!(
            self.store
                .selected_receipt(
                    self.scope,
                    writer::stage(self.operation, "target-interpretation")
                )?
                .is_some(),
            !self.calls.is_empty()
        );
        if self.fault == Fault::CancelIdentify {
            self.canceled.set(true);
        }
        if self.calls.is_empty() {
            if let Some(before) = self.before {
                before();
            }
        }
        handoff(&mut || {
            self.calls.push((self.text.clone(), self.images.clone()));
            Ok(())
        })?;
        match (self.fault, self.calls.len()) {
            (Fault::Identify45, 1) => writer::selected::advance_for_test(Duration::from_secs(46)),
            (Fault::Admission60, 1) => writer::selected::advance_for_test(Duration::from_secs(61)),
            (Fault::Normalize150, 2) => {
                writer::selected::advance_for_test(Duration::from_secs(151))
            }
            (Fault::Draft240, 3) => writer::selected::advance_for_test(Duration::from_secs(241)),
            (Fault::LostI, 1) | (Fault::LostX, 2) => {
                self.store.set_fault(crate::storage::Fault::AfterCommit)?
            }
            (Fault::UncertainProvider, 1) => anyhow::bail!("lost original identification response"),
            (Fault::SourceChanged, 1) => {
                let current = self
                    .store
                    .selected_snapshot(self.scope, MAX_ITEMS)?
                    .unwrap();
                let p = SelectedDomainProjection::from_snapshot(&current)?;
                let capture = p
                    .inspect(writer::stage(self.operation, "fresh-root"))?
                    .into_iter()
                    .find_map(|r| {
                        if let Record::ResidentCapture(c) = r {
                            Some(c.id)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let (e, _) = p.head(Namespace::Source, capture).unwrap();
                let mut e = e.clone();
                e.parents = BTreeSet::from([e.revision_id]);
                e.revision_id = Uuid::new_v4();
                e.operation_id = Uuid::new_v4();
                self.store.commit_selected(
                    &current.token,
                    Uuid::new_v4(),
                    vec![e],
                    BTreeMap::new(),
                )?;
            }
            _ => {}
        }
        Ok(match self.calls.len() {
            1 if self.fault == Fault::InvalidTarget => "invalid target JSON".into(),
            1 if self.fault == Fault::Uncertain => r#"{"subject":"Unclear selection","geometry":null,"understanding":{"status":"uncertain","reason":"two plausible regions"}}"#.into(),
            1 => CANDIDATE.into(),
            2 => r#"{"pages":[{"page":1,"status":"transcribed","text":"x=7"}]}"#.into(),
            3 => "# Note\n\nx=7".into(),
            _ => panic!("extra fresh submission"),
        })
    }
}

#[test]
fn fresh_regional_slot_to_three_submissions_preserves_one_user_and_about() {
    run(Fault::None, false);
}
#[test]
fn fresh_regional_ambiguous_target_stops_after_one_submission() {
    run(Fault::Uncertain, false);
}
#[test]
fn fresh_regional_actual_receiver_finalizes_success() {
    run(Fault::None, true);
}
#[test]
fn fresh_regional_actual_receiver_finalizes_uncertainty() {
    run(Fault::Uncertain, true);
}
#[test]
fn fresh_regional_original_deadlines_and_uncertain_replay_do_not_repeat_inference() {
    for fault in [
        Fault::BeforeIdentifyDeadline,
        Fault::Identify45,
        Fault::Admission60,
        Fault::Normalize150,
        Fault::Draft240,
        Fault::UncertainProvider,
        Fault::SourceChanged,
        Fault::HandoffBudget,
        Fault::FinishBudget,
        Fault::ContextChanged,
        Fault::ForeignParent,
        Fault::LostP,
        Fault::LostX,
    ] {
        run(fault, false);
    }
}
#[test]
fn fresh_regional_retains_one_confirmed_context_across_identify_normalize_and_draft() {
    run(Fault::Context, false);
}
#[test]
fn fresh_regional_freezes_memory_empty_context_and_refuses_changed_policy_or_mask_collision() {
    for fault in [
        Fault::PolicyChanged,
        Fault::Memory,
        Fault::MemoryChanged,
        Fault::MaskCollision,
        Fault::EmptyContext,
        Fault::EmptyContextChanged,
    ] {
        run(fault, false);
    }
}
#[test]
fn fresh_regional_cancellation_and_invalid_target_cannot_fall_through_to_reader() {
    for fault in [Fault::CancelIdentify, Fault::InvalidTarget] {
        run(fault, false);
    }
}
#[test]
fn fresh_regional_ineligible_selected_family_refuses_before_capture_publication() {
    run(Fault::IneligibleFamily, false);
}
#[test]
fn fresh_regional_fresh_child_fences_both_historic_input_variants() {
    run(Fault::LostI, false);
}
fn run(fault: Fault, resident: bool) {
    let _clock = writer::selected::clock_for_test();
    let mut f = CorrectionFixture::new();
    if fault != Fault::IneligibleFamily {
        crate::storage::target_generation::prepare(
            Arc::get_mut(&mut f.a).unwrap(),
            &digest(b"fresh receiver fixture"),
            crate::storage::target_generation::BaseKind::CompoundSelected,
            |_| Ok(()),
        )
        .unwrap();
    }
    let registered = if fault.context() || fault == Fault::ForeignParent {
        let mut confirmation = super::corrections::confirm(f.owner, Uuid::new_v4());
        if matches!(
            fault,
            Fault::MaskCollision | Fault::EmptyContext | Fault::EmptyContextChanged
        ) {
            let original = resident_capture::tests::fixture_original(
                &resident_capture::tests::capture_for(f.document),
            );
            let sample = if fault == Fault::MaskCollision {
                target::parse_candidate(CANDIDATE)
                    .unwrap()
                    .geometry
                    .unwrap()
                    .mask_png(&original, [12, 20], &mut || Ok(()))
                    .unwrap()
            } else {
                original
            };
            confirmation["sample_png_base64"] = serde_json::json!(STANDARD.encode(sample));
        }
        let revision = super::corrections::observed(f.command(&f.a, &confirmation, true));
        f.drive(&mut f.engine(&f.a), revision);
        Some(revision)
    } else {
        None
    };
    let identity = catalog::Identity {
        group: f.binding.group,
        profile: f.owner.profile,
        binding_sha256: f.binding.digest.clone(),
    };
    let owner = catalog::Owner::Document {
        document: f.document,
    };
    let scope = identity.aggregate_scope(&owner).unwrap();
    let genesis = f
        .engine(&f.a)
        .refresh_root_catalog(&mut 32)
        .unwrap()
        .unwrap()
        .root_sha;
    let _seed = fixture_defer_pending_for_worker(
        f.a.clone(),
        scope.clone(),
        f.document,
        genesis_base(&genesis, &scope.key_sha256),
    );
    super::recovery::register_document(f.a.clone(), &identity, owner, &genesis);
    let memory = if matches!(fault, Fault::Memory | Fault::MemoryChanged) {
        let provider =
            crate::memory::selected::SelectedMemory::new(f.a.clone(), identity.clone()).unwrap();
        let mut original = fact(identity.profile);
        original.claim = "note convention".into();
        original.subject = "note".into();
        original.value = "fixture original shared convention".into();
        let registration = provider
            .initialize_aggregate(crate::memory::selected::InitializeAggregate {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"fresh independent memory"),
                fact: original.clone(),
            })
            .unwrap()
            .registration
            .unwrap();
        provider
            .register_selection(crate::memory::selected::RegisterSelection {
                operation: Uuid::new_v4(),
                registration,
            })
            .unwrap();
        Some((provider, original))
    } else {
        None
    };
    let memory_provider = memory
        .as_ref()
        .map(|(p, _)| p as &dyn crate::memory::Provider);
    let writer = SelectedAdmission::new(f.a.clone(), scope.clone()).unwrap();
    let capture = resident_capture::tests::capture_for(f.document);
    let request = RegionalRequest {
        document: f.document,
        page: capture.page(),
        operation: Uuid::new_v4(),
        request: "Make a note about the left concept".into(),
        guidance: None,
    };
    let slot = FixtureSlot::new(request.clone());
    assert!(slot.pending());
    let path = f.storage.0.join("fresh-regional.json");
    files::atomic_json(&path, &serde_json::json!({"paths":f.a.paths,"handwriting":{"enabled":fault.context(),"profile":f.owner.profile}})).unwrap();
    preference_command::set_fixture_preference(
        f.a.clone(),
        &path,
        f.binding.clone(),
        &f.wire,
        serde_json::json!({"product_settings":{"writer_enabled":true}}),
    )
    .unwrap();
    if memory.is_some() {
        let mut config: serde_json::Value = files::json(&path, MAX_METADATA as u64).unwrap();
        config["memory"] =
            serde_json::json!({"enabled":true,"profile":identity.profile,"selection":identity});
        files::atomic_json(&path, &config).unwrap();
    }
    let config_bytes = std::fs::read(&path).unwrap();
    let preferences = preference_consumer::RequestPreferences::writer_image_from_binding(
        f.a.clone(),
        path,
        f.binding.clone(),
        |_| panic!("unexpected transport"),
    )
    .unwrap();
    match fault {
        Fault::HandoffBudget => {
            preferences.exhaust_phase_reads_for_test(preference_consumer::NotebookPhase::Handoff)
        }
        Fault::FinishBudget => preferences
            .exhaust_phase_reads_for_test(preference_consumer::NotebookPhase::Publication),
        _ => {}
    }
    let before = || {
        let prior =
            f.a.selected_snapshot(&scope, MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token;
        if fault == Fault::PolicyChanged {
            std::fs::write(f.storage.0.join("fresh-regional.json"), b"{}\n").unwrap();
        } else if fault == Fault::MemoryChanged {
            let (provider, original) = memory.as_ref().unwrap();
            let expected = provider
                .inspect(&original.key)
                .unwrap()
                .into_iter()
                .map(|r| r.revision_id)
                .collect();
            let mut fact = original.clone();
            fact.value = "changed frozen convention".into();
            crate::memory::Provider::put(
                provider,
                crate::memory::Mutation {
                    operation: Uuid::new_v4(),
                    expected,
                    fact,
                },
            )
            .unwrap();
        } else {
            let operation = if fault == Fault::ForeignParent {
                request.operation
            } else {
                Uuid::new_v4()
            };
            let mut update = super::corrections::confirm(f.owner, operation);
            if fault == Fault::EmptyContextChanged {
                update["sample_png_base64"] = serde_json::json!(
                    STANDARD.encode(resident_capture::tests::fixture_original(&capture))
                );
            }
            update["command"] = serde_json::json!("update");
            update["expected_heads"] = serde_json::json!([registered.unwrap()]);
            update["corrected_text"] = serde_json::json!("20");
            super::corrections::observed(f.command(&f.a, &update, false));
        }
        assert_eq!(
            f.a.selected_snapshot(&scope, MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            prior,
            "joined change did not alter document selection"
        );
    };
    if fault == Fault::LostP {
        f.a.set_fault(crate::storage::Fault::AfterCommit).unwrap();
    }
    let started = Instant::now();
    let canceled = std::cell::Cell::new(false);
    let mut model = Model {
        store: &f.a,
        scope: &scope,
        operation: request.operation,
        text: vec![],
        images: vec![],
        calls: vec![],
        deadlines: vec![],
        fault,
        canceled: &canceled,
        before: matches!(
            fault,
            Fault::ContextChanged
                | Fault::ForeignParent
                | Fault::PolicyChanged
                | Fault::MemoryChanged
                | Fault::EmptyContextChanged
        )
        .then_some(&before as &dyn Fn()),
    };
    let result = if resident {
        crate::workflow::fresh_receiver_fixture(
            f.a.clone(),
            &writer,
            &slot.pin,
            preferences,
            &mut model,
        )
        .map(|_| None)
    } else {
        let operation = FreshOperation::new(&writer, preferences);
        if fault == Fault::IneligibleFamily {
            assert!(operation
                .err()
                .unwrap()
                .to_string()
                .contains("target family"));
            assert!(slot.pending());
            assert!(model.calls.is_empty());
            assert!(f
                .a
                .selected_receipt(
                    &scope,
                    writer::selected::normalization_preparations(request.operation)[1]
                )
                .unwrap()
                .is_none());
            return;
        }
        let mut operation = operation.unwrap();
        let token = operation.initial(&writer).unwrap();
        operation
            .run(
                &writer,
                &token,
                &capture,
                &slot.pin,
                &mut model,
                memory_provider,
                &mut || canceled.get(),
                &|| Ok(()),
                "fixture-model",
            )
            .map(Some)
    };
    if !fault.success() && fault != Fault::Uncertain {
        let expected = match fault {
            Fault::BeforeIdentifyDeadline
            | Fault::HandoffBudget
            | Fault::ContextChanged
            | Fault::ForeignParent
            | Fault::LostP
            | Fault::CancelIdentify
            | Fault::PolicyChanged
            | Fault::MemoryChanged
            | Fault::EmptyContextChanged => 0,
            Fault::Normalize150 | Fault::FinishBudget | Fault::LostX => 2,
            Fault::Draft240 => 3,
            _ => 1,
        };
        assert_eq!(model.calls.len(), expected, "{fault:?}");
        if let Ok(Some(report)) = &result {
            assert!(
                !matches!(report.report.status, writer::Status::Generated),
                "{fault:?}"
            );
        } else {
            assert!(result.is_err(), "{fault:?}");
        }
        assert_eq!(slot.pending(), fault == Fault::LostP, "{fault:?}");
        if matches!(fault, Fault::LostP | Fault::LostI | Fault::LostX) {
            let stage = match fault {
                Fault::LostI => writer::stage(request.operation, "target-interpretation"),
                Fault::LostX => request.operation,
                _ => writer::selected::normalization_preparations(request.operation)[1],
            };
            assert!(
                f.a.selected_receipt(&scope, stage).unwrap().is_some(),
                "{fault:?} retains original receipt"
            );
        }
        // A newly queued copy has no right to repeat the existing P/I/X. Use a
        // fresh process-style policy/owner and the actual latest selected token.
        writer::selected::advance_for_test(Duration::ZERO);
        canceled.set(false);
        f.a.set_fault(crate::storage::Fault::None).unwrap();
        std::fs::write(f.storage.0.join("fresh-regional.json"), &config_bytes).unwrap();
        let duplicate = FixtureSlot::new(request.clone());
        let preferences = preference_consumer::RequestPreferences::writer_image_from_binding(
            f.a.clone(),
            f.storage.0.join("fresh-regional.json"),
            f.binding.clone(),
            |_| panic!("unexpected transport"),
        )
        .unwrap();
        let mut replay = FreshOperation::new(&writer, preferences).unwrap();
        let token = replay.initial(&writer).unwrap();
        let repeated = replay.run(
            &writer,
            &token,
            &capture,
            &duplicate.pin,
            &mut model,
            memory_provider,
            &mut || false,
            &|| Ok(()),
            "fixture-model",
        );
        assert!(repeated.is_err(), "historical P is terminal: {fault:?}");
        assert_eq!(
            model.calls.len(),
            expected,
            "no repeated submission: {fault:?}"
        );
        if fault == Fault::LostI {
            drop(replay);
            let read = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
            let p = SelectedDomainProjection::from_snapshot(&read).unwrap();
            let root = writer::stage(request.operation, "fresh-root");
            let selection = writer::selected::Selection {
                accepted: read.token.clone(),
                root: p.current_reference(Namespace::Conversation, root).unwrap(),
                refine: None,
            };
            let target = p
                .inspect(root)
                .unwrap()
                .into_iter()
                .find_map(|r| {
                    if let Record::SourceTarget(t) = r {
                        Some(t)
                    } else {
                        None
                    }
                })
                .unwrap();
            let target_pin = p.current_reference(Namespace::Source, target.id).unwrap();
            let target_ref = target::RecordRef {
                namespace: target_pin.namespace,
                record: target_pin.record_id,
                revision: target_pin.revision_id,
                object: target_pin.object,
            };
            let image = SelectedWriterImage {
                source: p
                    .current_reference(
                        Namespace::Source,
                        writer::stage(request.operation, "fresh-capture"),
                    )
                    .unwrap(),
                turn: p
                    .current_reference(
                        Namespace::Conversation,
                        writer::stage(request.operation, "user"),
                    )
                    .unwrap(),
                ordinal: 0,
                page: 1,
            };
            assert!(f
                .a
                .selected_receipt(&scope, request.operation)
                .unwrap()
                .is_none());
            let mut reads = super::super::corrections::allowance();
            SelectedAdmission::with_gates(&[&writer], |store| {
                writer.selected_writer_image_locked(
                    store,
                    &read.token,
                    &selection.root,
                    &image,
                    &mut reads,
                )?;
                writer.selected_writer_region_locked(
                    store,
                    &read.token,
                    &selection.root,
                    &target_ref,
                    &mut reads,
                    &mut || Ok(()),
                )?;
                Ok(())
            })
            .unwrap();
            let policy = || {
                preference_consumer::RequestPreferences::writer_image_from_binding(
                    f.a.clone(),
                    f.storage.0.join("fresh-regional.json"),
                    f.binding.clone(),
                    |_| panic!("unexpected transport"),
                )
                .unwrap()
            };
            let capture_input = writer::selected::WriterImageRequest {
                operation: request.operation,
                request: request.request.clone(),
                image,
                guidance: None,
                model_profile: "fixture-model".into(),
            };
            assert!(writer::selected::run_image(
                &writer,
                &selection,
                &capture_input,
                &mut model,
                policy(),
                None,
                &mut || false
            )
            .is_err());
            let target_input = writer::selected::WriterTargetRequest {
                operation: request.operation,
                request: request.request.clone(),
                target: target_ref,
                guidance: None,
                model_profile: "fixture-model".into(),
            };
            assert!(writer::selected::run_target(
                &writer,
                &selection,
                &target_input,
                &mut model,
                policy(),
                None,
                &mut || false
            )
            .is_err());
            assert_eq!(
                model.calls.len(),
                expected,
                "fresh P fences both historic routes with valid sources and X absent"
            );
        }
        return;
    }
    assert!(!slot.pending(), "matching slot consumed before inference");
    let read = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    let projection = SelectedDomainProjection::from_snapshot(&read).unwrap();
    let root = writer::stage(request.operation, "fresh-root");
    let records = projection.inspect(root).unwrap();
    let turns = records
        .iter()
        .filter_map(|r| {
            if let Record::Turn(t) = r {
                Some(t)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(turns.iter().filter(|t| t.role == Role::User).count(), 1);
    let user = turns.iter().find(|t| t.role == Role::User).unwrap();
    assert_eq!(user.text.as_deref(), Some(request.request.as_str()));
    assert_eq!(user.id, writer::stage(request.operation, "user"));
    assert_eq!(user.sequence, 0);
    assert!(records
        .iter()
        .any(|r| matches!(r, Record::ResidentCapture(_))));
    if fault == Fault::Uncertain {
        assert!(result.err().unwrap().to_string().contains("uncertain"));
        assert_eq!(model.calls.len(), 1);
        assert_eq!(turns.len(), 1);
        assert!(f
            .a
            .selected_receipt(&scope, request.operation)
            .unwrap()
            .is_none());
        return;
    }
    if let Some(report) = result.unwrap() {
        assert!(
            matches!(report.report.status, writer::Status::Generated),
            "{:?}",
            report.report.status
        );
    }
    assert_eq!(model.calls.len(), 3);
    assert_eq!(
        model.calls[0].1.len(),
        if fault == Fault::Context { 2 } else { 1 }
    );
    assert_eq!(
        model.calls[1].1.len(),
        if fault == Fault::Context { 2 } else { 1 }
    );
    assert!(model.calls[2].1.is_empty());
    assert_ne!(model.calls[0].1, model.calls[1].1);
    let target = records
        .iter()
        .find_map(|r| {
            if let Record::SourceTarget(t) = r {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    let target_ref = projection
        .current_reference(Namespace::Source, target.id)
        .unwrap();
    let root_ref = projection
        .current_reference(Namespace::Conversation, root)
        .unwrap();
    let mut reads = super::super::corrections::allowance();
    let proof = SelectedAdmission::with_gates(&[&writer], |store| {
        writer.selected_writer_region_locked(
            store,
            &read.token,
            &root_ref,
            &target::RecordRef {
                namespace: target_ref.namespace,
                record: target_ref.record_id,
                revision: target_ref.revision_id,
                object: target_ref.object,
            },
            &mut reads,
            &mut || Ok(()),
        )
    })
    .unwrap();
    assert_eq!(
        model.calls[1].1.last().unwrap(),
        &STANDARD.encode(&proof.png)
    );
    if fault == Fault::Context {
        assert_eq!(
            model.calls[0].1[0], model.calls[1].1[0],
            "one frozen confirmed example"
        );
        assert_ne!(model.calls[0].1[0], *model.calls[0].1.last().unwrap());
    }
    assert_eq!(user.sources.len(), 2);
    let assistant = turns.iter().find(|t| t.role == Role::Assistant).unwrap();
    assert_eq!(assistant.sequence, 1);
    assert_eq!(assistant.sources, vec![user.sources[1]]);
    let body = assistant.text.as_deref().unwrap();
    assert_eq!(body.matches("About:").count(), 1);
    let used = records
        .iter()
        .find_map(|r| {
            if let Record::TargetUse(u) = r {
                Some(u)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        used.inline_body_sha256.as_deref(),
        Some(digest(body.as_bytes()).as_str())
    );
    let model_context = writer
        .context_range(
            &read.token,
            root,
            None,
            &ContextBudget {
                max_text_bytes: 64 * 1024,
                max_turns: 64,
                provider_token_limit: Some(64 * 1024),
            },
            |_| Ok(0),
        )
        .unwrap();
    assert!(
        !model_context
            .turns
            .iter()
            .any(|t| t.role == Role::Assistant),
        "model context recipe remains unchanged"
    );
    let context = writer
        .presentation_context(
            &read.token,
            root,
            &ContextBudget {
                max_text_bytes: 64 * 1024,
                max_turns: 64,
                provider_token_limit: Some(64 * 1024),
            },
            |_| Ok(0),
        )
        .unwrap();
    let about = crate::conversation_layout::about_entries(root, &context).unwrap();
    assert!(
        about.is_empty(),
        "exact inline body suppresses duplicate About on unbound reopen"
    );
    for misplaced in 0..3 {
        let mut changed = context.clone();
        let assistant = changed
            .turns
            .iter_mut()
            .find(|t| t.role == Role::Assistant)
            .unwrap();
        assistant.text = Some("# Note\n\nChanged body\nAbout: Left concept".into());
        if misplaced == 2 {
            assistant.text = Some("About: Left concept\nx=7".into());
        }
        if misplaced != 0 {
            changed.target_uses[0].inline_body_sha256 =
                Some(digest(assistant.text.as_deref().unwrap().as_bytes()));
        }
        let about = crate::conversation_layout::about_entries(root, &changed).unwrap();
        assert_eq!(
            about.len(),
            1,
            "changed hash or misplaced About must not suppress metadata"
        );
    }
    assert!(model.deadlines[0] <= started + Duration::from_secs(60));
    assert!(model.deadlines[1] <= started + Duration::from_secs(150));
    assert!(model.deadlines[2] <= started + Duration::from_secs(240));
}
