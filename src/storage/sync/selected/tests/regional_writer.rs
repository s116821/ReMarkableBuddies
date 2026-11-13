//! Actual selected regional adapter controls; no device/provider authority.
use super::corrections::CorrectionFixture;
use super::*;
use crate::{
    conversation::*,
    llm::ModelHandoff,
    writer::{
        self,
        selected::{Selection, WriterTargetRequest},
    },
    LLMEngine,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::cell::Cell;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Case {
    Good,
    Cancel,
    SourceChanged,
    AdmissionExpired,
    AdmissionCrossed,
    AdmissionBudget,
    HandoffBudget,
    PublicationBudget,
    ReservedAbout,
    DecoratedTooLarge,
    NormalizeExpired,
    DraftExpired,
    FinalExpired,
    Delayed,
}

#[test]
fn regional_writer_uses_exact_mask_fixed_slots_and_original_custody() {
    for case in [
        Case::Good,
        Case::Cancel,
        Case::SourceChanged,
        Case::AdmissionExpired,
        Case::AdmissionCrossed,
        Case::AdmissionBudget,
        Case::HandoffBudget,
        Case::PublicationBudget,
        Case::ReservedAbout,
        Case::DecoratedTooLarge,
        Case::NormalizeExpired,
        Case::DraftExpired,
        Case::FinalExpired,
        Case::Delayed,
    ] {
        let clock = writer::selected::clock_for_test();
        let mut f = CorrectionFixture::new();
        crate::storage::target_generation::prepare(
            Arc::get_mut(&mut f.a).unwrap(),
            &digest(b"regional fixture"),
            crate::storage::target_generation::BaseKind::CompoundSelected,
            |_| Ok(()),
        )
        .unwrap();
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
        let (token, _, _) = fixture_defer_pending_for_worker(
            f.a.clone(),
            scope.clone(),
            f.document,
            genesis_base(&genesis, &scope.key_sha256),
        );
        super::recovery::register_document(f.a.clone(), &identity, owner, &genesis);
        let writer = SelectedAdmission::new(f.a.clone(), scope.clone()).unwrap();
        let captured = selected_reader_prepare::ResidentPrepared::new(
            &f.a,
            &token,
            &resident_capture::tests::capture_for(f.document),
            Uuid::new_v4(),
            2,
        )
        .unwrap();
        let (_, capture) = SelectedAdmission::with_gates(&[&writer], |store| {
            captured.publish_locked(store, &token, &mut super::super::corrections::allowance())
        })
        .unwrap();
        let snapshot = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        let candidate=serde_json::from_value(serde_json::json!({"subject":"Left concept","geometry":{"components":[[[0,0],[500000,0],[500000,1000000],[0,1000000]]]},"understanding":{"status":"interpreted"}})).unwrap();
        let interpreted = writer
            .publish_reader_text(
                &snapshot.token,
                SelectedReaderWrite {
                    operation: Uuid::new_v4(),
                    root: capture.root,
                    user: capture.user,
                    updated_ms: 3,
                    text: "Explain the left concept".into(),
                    kind: SelectedReaderText::InterpretTarget { candidate },
                },
            )
            .unwrap();
        let SelectedReaderPublication::Published { acknowledgment, .. } = interpreted else {
            panic!("fresh interpretation")
        };
        let snapshot = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        let projection = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
        let target = projection
            .inspect(acknowledgment.root.record_id)
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
        let pin = projection
            .current_reference(Namespace::Source, target.id)
            .unwrap();
        let reference = target::RecordRef {
            namespace: pin.namespace,
            record: pin.record_id,
            revision: pin.revision_id,
            object: pin.object,
        };
        let selection = Selection {
            accepted: snapshot.token,
            root: projection
                .current_reference(Namespace::Conversation, target.conversation)
                .unwrap(),
            refine: None,
        };
        let path = f.storage.0.join("regional.json");
        files::atomic_json(
            &path,
            &serde_json::json!({"paths":f.a.paths,"handwriting":{"enabled":false}}),
        )
        .unwrap();
        preference_command::set_fixture_preference(
            f.a.clone(),
            &path,
            f.binding.clone(),
            &f.wire,
            serde_json::json!({"product_settings":{"writer_enabled":true}}),
        )
        .unwrap();
        let policy = || {
            preference_consumer::RequestPreferences::writer_image_from_binding(
                f.a.clone(),
                path.clone(),
                f.binding.clone(),
                |_| panic!("unexpected transport"),
            )
            .unwrap()
        };
        let preferences = policy();
        let mut budget = super::super::corrections::allowance();
        let proof = SelectedAdmission::with_gates(&[&writer], |store| {
            writer.selected_writer_region_locked(
                store,
                &selection.accepted,
                &selection.root,
                &reference,
                &mut budget,
                &mut || Ok(()),
            )
        })
        .unwrap();
        let expected = STANDARD.encode(&proof.png);
        let input = WriterTargetRequest {
            operation: Uuid::new_v4(),
            request: "Make a regional note".into(),
            target: reference,
            guidance: None,
            model_profile: "fixture".into(),
        };
        let canceled = Cell::new(false);
        struct Model<'a> {
            case: Case,
            store: &'a Store,
            scope: &'a SelectionScope,
            source: Uuid,
            canceled: &'a Cell<bool>,
            calls: Vec<Vec<String>>,
            images: Vec<String>,
            deadlines: Vec<Instant>,
        }
        impl LLMEngine for Model<'_> {
            fn guarded_timeout(&self) -> Option<Duration> {
                Some(Duration::from_secs(1))
            }
            fn add_text_content(&mut self, _: &str) {}
            fn add_image_content(&mut self, s: &str) {
                self.images.push(s.into());
                if self.case == Case::Delayed && self.calls.is_empty() {
                    writer::selected::advance_for_test(Duration::from_secs(70));
                }
            }
            fn clear_content(&mut self) {
                self.images.clear();
                if self.case == Case::FinalExpired && self.calls.len() == 2 {
                    writer::selected::advance_for_test(Duration::from_secs(301));
                }
            }
            fn execute(&mut self) -> Result<String> {
                panic!("unguarded region")
            }
            fn execute_with_deadline(
                &mut self,
                progress: &mut dyn FnMut() -> Result<()>,
                handoff: &mut ModelHandoff<'_>,
                deadline: Instant,
            ) -> Result<String> {
                self.deadlines.push(deadline);
                self.execute_with_handoff(progress, handoff)
            }
            fn execute_with_handoff(
                &mut self,
                progress: &mut dyn FnMut() -> Result<()>,
                handoff: &mut ModelHandoff<'_>,
            ) -> Result<String> {
                progress()?;
                if self.case == Case::Cancel {
                    self.canceled.set(true);
                }
                handoff(&mut || {
                    self.calls.push(self.images.clone());
                    Ok(())
                })?;
                let stage = self.calls.len();
                if self.case == Case::SourceChanged && stage == 1 {
                    let read = self
                        .store
                        .selected_snapshot(self.scope, MAX_ITEMS)?
                        .unwrap();
                    let p = SelectedDomainProjection::from_snapshot(&read)?;
                    let (e, _) = p.head(Namespace::Source, self.source).unwrap();
                    let mut changed = e.clone();
                    changed.parents = BTreeSet::from([changed.revision_id]);
                    changed.revision_id = Uuid::new_v4();
                    changed.operation_id = Uuid::new_v4();
                    self.store.commit_selected(
                        &read.token,
                        Uuid::new_v4(),
                        vec![changed],
                        BTreeMap::new(),
                    )?;
                }
                if self.case == Case::NormalizeExpired && stage == 1 {
                    writer::selected::advance_for_test(Duration::from_secs(151));
                }
                if self.case == Case::DraftExpired && stage == 2 {
                    writer::selected::advance_for_test(Duration::from_secs(241));
                }
                Ok(if stage == 1 {
                    r#"{"pages":[{"page":1,"status":"transcribed","text":"x=7"}]}"#.into()
                } else if self.case == Case::ReservedAbout {
                    "# Note\n\nAbout: False model provenance\n\nx=7".into()
                } else if self.case == Case::DecoratedTooLarge {
                    // Fits the draft bound before the required provenance is added.
                    format!("# Note\n\n{}", "x".repeat(24 * 1024 - 8))
                } else {
                    "# Note\n\nx=7".into()
                })
            }
        }
        let mut model = Model {
            case,
            store: &f.a,
            scope: &scope,
            source: target.source.record.record,
            canceled: &canceled,
            calls: vec![],
            images: vec![],
            deadlines: vec![],
        };
        if case == Case::AdmissionExpired {
            writer::selected::advance_for_test(Duration::from_secs(61));
        }
        if case == Case::AdmissionCrossed {
            let admission = preferences.regional_cutoff(60).unwrap();
            clock.hold_at(admission - Duration::from_secs(1));
            writer::selected::advance_at_admission_recheck_for_test(
                admission + Duration::from_secs(1),
            );
        }
        let norm_deadline = preferences.regional_cutoff(150).unwrap();
        match case {
            Case::AdmissionBudget => preferences
                .exhaust_phase_reads_for_test(preference_consumer::NotebookPhase::Admission),
            Case::HandoffBudget => preferences
                .exhaust_phase_reads_for_test(preference_consumer::NotebookPhase::Handoff),
            Case::PublicationBudget => preferences
                .exhaust_phase_reads_for_test(preference_consumer::NotebookPhase::Publication),
            _ => {}
        }
        let result = writer::selected::run_target(
            &writer,
            &selection,
            &input,
            &mut model,
            preferences,
            None,
            &mut || canceled.get(),
        );
        if case == Case::AdmissionCrossed {
            assert!(
                !clock.admission_recheck_pending(),
                "must cross during the locked continuation reread"
            );
        }
        writer::selected::advance_for_test(Duration::ZERO);
        let snapshot = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        let records = SelectedDomainProjection::from_snapshot(&snapshot)
            .unwrap()
            .inspect(target.conversation)
            .unwrap();
        let generated = records.iter().find_map(|r| match r {
            Record::Turn(t)
                if t.exchange == input.operation
                    && t.role == Role::Assistant
                    && t.outcome == Outcome::Generated =>
            {
                Some(t)
            }
            _ => None,
        });
        if matches!(case, Case::Good | Case::Delayed) {
            let report = result.unwrap();
            assert_eq!(report.report.status, writer::Status::Generated, "{case:?}");
            assert_eq!(model.calls.len(), 2);
            assert!(model.calls[0].len() == 1 && model.calls[0][0].ends_with(&expected));
            assert!(model.calls[1].is_empty());
            assert!(generated.unwrap().text.as_ref().unwrap().contains("About:"));
            assert!(records.iter().any(|r|matches!(r,Record::RegionSource(s) if s.target==input.target&&s.derived.sha256==digest(&proof.png))));
            if case == Case::Delayed {
                assert_eq!(model.deadlines[0], norm_deadline);
            }
            let latest = Selection {
                accepted: snapshot.token.clone(),
                root: SelectedDomainProjection::from_snapshot(&snapshot)
                    .unwrap()
                    .current_reference(Namespace::Conversation, target.conversation)
                    .unwrap(),
                refine: None,
            };
            assert!(writer::selected::run_target(
                &writer,
                &latest,
                &input,
                &mut model,
                policy(),
                None,
                &mut || false
            )
            .is_err());
            assert_eq!(model.calls.len(), 2);
            if case == Case::Good {
                let source = generated.unwrap().sources[0];
                let draft = generated.unwrap().id;
                let refine =
                    |snapshot: &crate::storage::selection::SelectedSnapshot, source, draft| {
                        let projection = SelectedDomainProjection::from_snapshot(snapshot).unwrap();
                        Selection {
                            accepted: snapshot.token.clone(),
                            root: projection
                                .current_reference(Namespace::Conversation, target.conversation)
                                .unwrap(),
                            refine: Some((
                                projection
                                    .current_reference(Namespace::Source, source)
                                    .unwrap(),
                                projection
                                    .current_reference(Namespace::Conversation, draft)
                                    .unwrap(),
                            )),
                        }
                    };
                let report = writer::selected::run_with_preferences(
                    &writer,
                    &refine(&snapshot, source, draft),
                    &mut model,
                    &writer::Request::Refine {
                        conversation: target.conversation,
                        operation: Uuid::new_v4(),
                        request: "Make it shorter".into(),
                        draft,
                    },
                    policy(),
                    None,
                    &mut || false,
                );
                assert_eq!(report.report.status, writer::Status::Generated);
                assert_eq!(model.calls.len(), 3);
                assert!(model.calls[2].is_empty());
                let refined = report.report.draft.unwrap();
                let latest = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
                let projection = SelectedDomainProjection::from_snapshot(&latest).unwrap();
                let Some((_, Some(Record::Turn(turn)))) =
                    projection.head(Namespace::Conversation, refined)
                else {
                    panic!("refined turn")
                };
                assert_eq!(turn.sources, vec![source]);
                assert_eq!(
                    turn.text
                        .as_ref()
                        .unwrap()
                        .matches("About: Left concept")
                        .count(),
                    1
                );
                model.case = Case::ReservedAbout;
                let refused = writer::selected::run_with_preferences(
                    &writer,
                    &refine(&latest, source, refined),
                    &mut model,
                    &writer::Request::Refine {
                        conversation: target.conversation,
                        operation: Uuid::new_v4(),
                        request: "Keep the provenance".into(),
                        draft: refined,
                    },
                    policy(),
                    None,
                    &mut || false,
                );
                assert_eq!(refused.report.status, writer::Status::Failed);
                assert_eq!(refused.report.failure, Some(writer::Failure::InvalidOutput));
                assert!(refused.report.draft.is_none());
                assert_eq!(model.calls.len(), 4);
            }
        } else {
            assert!(generated.is_none(), "{case:?}");
            if matches!(case, Case::ReservedAbout | Case::DecoratedTooLarge) {
                let report = result.unwrap().report;
                assert_eq!(report.status, writer::Status::Failed);
                assert_eq!(report.failure, Some(writer::Failure::InvalidOutput));
                assert!(report.draft.is_none());
                assert_eq!(model.calls.len(), 2);
                continue;
            }
            assert!(
                result.is_err() || result.unwrap().publication.is_none(),
                "{case:?}"
            );
            if matches!(
                case,
                Case::AdmissionExpired
                    | Case::AdmissionCrossed
                    | Case::AdmissionBudget
                    | Case::HandoffBudget
                    | Case::Cancel
            ) {
                assert!(model.calls.is_empty());
            }
            if matches!(case, Case::AdmissionExpired | Case::AdmissionCrossed) {
                assert!(f
                    .a
                    .selected_receipt(&scope, input.preparation())
                    .unwrap()
                    .is_none());
                assert!(!records
                    .iter()
                    .any(|r| matches!(r, Record::Turn(t) if t.exchange == input.preparation())));
            }
            if matches!(case, Case::NormalizeExpired | Case::SourceChanged) {
                assert_eq!(model.calls.len(), 1);
                assert!(!records.iter().any(|r|matches!(r,Record::RegionSource(s) if s.turn==writer::stage(input.operation,"user"))));
            }
        }
    }
}
