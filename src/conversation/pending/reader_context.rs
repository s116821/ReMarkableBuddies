//! Controlled pending-only construction. No production native implementer.
use super::*;
use crate::conversation::reader_dispatch::Entered;
use crate::conversation::{reader_dispatch::DispatchState, ReaderHandoff, ReaderPlan};
use std::rc::Rc;

pub(crate) trait ReaderSourceAdmission: SourceAdmission {
    fn backend_binding(&self) -> &crate::workflow::selected_backend::ReaderBackendBinding;
    fn verify_lower(
        &self,
        request: &PendingIntentRequest,
        outer: &ReaderHandoff,
        ordinal: usize,
        lower: &ReaderHandoff,
    ) -> Result<()>;
    fn verify_plan(&self, request: &PendingIntentRequest, plan: &ReaderPlan) -> Result<()>;
    fn verify_handoff(&self, request: &PendingIntentRequest, handoff: &ReaderHandoff)
        -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct RecordingSource {
        source: SourceObservation,
        binding: crate::workflow::selected_backend::ReaderBackendBinding,
        revoked: Cell<bool>,
        plan_checks: Cell<usize>,
        lower_checks: Cell<usize>,
        refuse_lower: Option<usize>,
    }
    impl sealed::Sealed for RecordingSource {}
    impl SourceAdmission for RecordingSource {
        fn verify_current(&self, request: &PendingIntentRequest) -> Result<()> {
            ensure!(
                !self.revoked.get() && self.source == request.source,
                "test source revoked"
            );
            Ok(())
        }
    }
    impl ReaderSourceAdmission for RecordingSource {
        fn backend_binding(&self) -> &crate::workflow::selected_backend::ReaderBackendBinding {
            &self.binding
        }
        fn verify_lower(
            &self,
            request: &PendingIntentRequest,
            _: &ReaderHandoff,
            _: usize,
            _: &ReaderHandoff,
        ) -> Result<()> {
            self.lower_checks.set(self.lower_checks.get() + 1);
            ensure!(
                self.refuse_lower != Some(self.lower_checks.get()),
                "test lower native refusal"
            );
            self.verify_current(request)
        }

        fn verify_plan(&self, request: &PendingIntentRequest, _: &ReaderPlan) -> Result<()> {
            self.plan_checks.set(self.plan_checks.get() + 1);
            self.verify_current(request)
        }
        fn verify_handoff(&self, request: &PendingIntentRequest, _: &ReaderHandoff) -> Result<()> {
            self.verify_current(request)
        }
    }
    struct BackendRecorder {
        trace: Rc<std::cell::RefCell<Vec<String>>>,
        panic_navigation: bool,
    }
    impl BackendRecorder {
        fn record(&self, value: String) {
            self.trace.borrow_mut().push(value);
        }
    }
    impl crate::device::backend::DeviceBackend for BackendRecorder {
        fn capture(&mut self) -> Result<crate::device::backend::Frame> {
            self.record("capture".into());
            anyhow::bail!("unexpected capture")
        }
        fn detail_images(&self) -> Result<Vec<String>> {
            self.record("details".into());
            anyhow::bail!("unexpected details")
        }
        fn wait_for_trigger(&mut self) -> Result<()> {
            self.record("trigger".into());
            Ok(())
        }
        fn prepare_reader_trigger(&mut self) -> Result<()> {
            self.record("prepare".into());
            Ok(())
        }
        fn navigate(
            &mut self,
            direction: crate::workflow::xochitl_integration::NavigationDirection,
        ) -> Result<crate::device::backend::NavigationCompletion> {
            self.record(format!("navigate:{direction:?}"));
            assert!(!self.panic_navigation, "test native backend panic");
            Ok(crate::device::backend::NavigationCompletion::Settled)
        }
        fn render_text(&mut self, text: &str) -> Result<()> {
            self.record(format!("text:{text}"));
            Ok(())
        }
        fn body_mode(&mut self) -> Result<()> {
            self.record("body".into());
            Ok(())
        }
        fn line(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
            self.record(format!("line:{from:?}:{to:?}"));
            Ok(())
        }
        fn erase(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
            self.record(format!("erase:{from:?}:{to:?}"));
            Ok(())
        }
        fn bitmap(&mut self, bitmap: &[Vec<bool>]) -> Result<()> {
            self.record(format!("bitmap:{}:{}", bitmap.len(), bitmap[0].len()));
            Ok(())
        }
        fn progress(&mut self, text: Option<&str>) -> Result<()> {
            self.record(format!("progress:{text:?}"));
            Ok(())
        }
        fn status_stroke(&mut self, _: crate::workflow::indicator::Stroke) -> Result<()> {
            self.record("statusstroke".into());
            Ok(())
        }
        fn status_clear(&mut self, _: &[crate::workflow::indicator::Stroke]) -> Result<()> {
            self.record("statusclear".into());
            Ok(())
        }
        fn monotonic(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
        fn load_header(&self) -> Option<image::DynamicImage> {
            self.record("loadheader".into());
            None
        }
        fn save_header(&mut self, _: &image::DynamicImage) -> Result<()> {
            self.record("saveheader".into());
            Ok(())
        }
        fn delay(&mut self, _: std::time::Duration) {
            self.record("delay".into());
        }
    }
    fn plan() -> ReaderPlan {
        ReaderPlan::new(vec![ReaderHandoff::NextPage, ReaderHandoff::PreviousPage]).unwrap()
    }
    fn independent_unbound_root_case(add_unbound: bool) {
        let fixture = super::super::tests::Fixture::new();
        let (store, handle, mut token, mut request) = fixture.setup();
        if add_unbound {
            let id = Uuid::new_v4();
            let root = new_record(
                store.actor_id,
                id,
                BTreeSet::new(),
                Record::Root(Root {
                    id,
                    next_sequence: 0,
                    binding: None,
                    created_ms: 1,
                    updated_ms: 1,
                }),
                vec![],
            )
            .unwrap();
            let publication = store
                .commit_selected(&token, Uuid::new_v4(), vec![root], BTreeMap::new())
                .unwrap();
            token = publication.token;
            request.selection = IntentSelectionEvidence::from_token(&token);
            let snapshot = store
                .selected_snapshot(handle.scope(), MAX_ITEMS)
                .unwrap()
                .unwrap();
            let owners = SelectedDomainProjection::from_snapshot(&snapshot)
                .unwrap()
                .document_ownership()
                .unwrap();
            assert_eq!(
                owners.get(&request.conversation),
                Some(&DocumentOwnership::Document(request.source.document))
            );
            assert_eq!(owners.get(&id), Some(&DocumentOwnership::DeferredMissing));
            assert!(!snapshot.selected_records.iter().any(|e| matches!(
                Ledger::decode(e).unwrap(),
                Record::Receipt(_) | Record::OutcomeFact(_)
            )));
        }
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: crate::workflow::selected_backend::ReaderBackendBinding::for_test(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let admission = Arc::new(handle);
        let result =
            admission.prepare_reader(Some(&token), request.clone(), source.clone(), plan());
        if result.is_err() {
            assert!(matches!(
                admission
                    .publish_pending(Some(&token), request, Some(source.as_ref()))
                    .unwrap(),
                PendingIntentPublication::Published { .. }
            ));
            eprintln!("ordinary pending publication accepts the same unbound-root aggregate");
        }
        assert!(
            matches!(result, Ok(ReaderPreparation::Fresh(_))),
            "unbound_root={add_unbound}: {:?}",
            result.err()
        );
    }
    #[test]
    fn independent_bound_only_control_mints() {
        independent_unbound_root_case(false);
    }
    #[test]
    fn independent_unbound_root_without_uncertainty_does_not_block_bound_reader() {
        independent_unbound_root_case(true);
    }
    #[test]
    fn actual_publication_mints_once_and_historical_retry_checks_no_live_plan() {
        let fixture = super::super::tests::Fixture::new();
        let (_store, handle, token, request) = fixture.setup();
        let admission = Arc::new(handle);
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: crate::workflow::selected_backend::ReaderBackendBinding::for_test(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let ReaderPreparation::Fresh(context) = admission
            .prepare_reader(Some(&token), request.clone(), source.clone(), plan())
            .unwrap()
        else {
            panic!("new publication must mint")
        };
        let entries = Cell::new(0);
        context
            .dispatch(0, &ReaderHandoff::NextPage, || {
                entries.set(entries.get() + 1);
                Ok(())
            })
            .unwrap();
        assert!(context
            .dispatch(0, &ReaderHandoff::NextPage, || {
                entries.set(entries.get() + 1);
                Ok(())
            })
            .is_err());
        assert!(context
            .dispatch(1, &ReaderHandoff::PreviousPage, || {
                entries.set(entries.get() + 1);
                Ok(())
            })
            .is_err());
        assert_eq!(entries.get(), 1);
        source.revoked.set(true);
        let before = source.plan_checks.get();
        let ReaderPreparation::Historical(original) = admission
            .prepare_reader(None, request.clone(), source.clone(), plan())
            .unwrap()
        else {
            panic!("retry must remain historical")
        };
        assert_eq!(original.receipt.acknowledgment.operation, request.operation);
        assert_eq!(source.plan_checks.get(), before);
    }
    #[test]
    fn native_revocation_and_backend_error_stop_later_entries() {
        for revoke in [false, true] {
            let fixture = super::super::tests::Fixture::new();
            let (_store, handle, token, request) = fixture.setup();
            let source = Rc::new(RecordingSource {
                source: request.source.clone(),
                binding: crate::workflow::selected_backend::ReaderBackendBinding::for_test(),
                revoked: Cell::new(false),
                plan_checks: Cell::new(0),
                lower_checks: Cell::new(0),
                refuse_lower: None,
            });
            let ReaderPreparation::Fresh(context) = Arc::new(handle)
                .prepare_reader(Some(&token), request, source.clone(), plan())
                .unwrap()
            else {
                panic!("new publication must mint")
            };
            source.revoked.set(revoke);
            let entries = Cell::new(0);
            assert!(context
                .dispatch(0, &ReaderHandoff::NextPage, || -> Result<()> {
                    entries.set(entries.get() + 1);
                    anyhow::bail!("backend refusal")
                })
                .is_err());
            assert!(context
                .dispatch(1, &ReaderHandoff::PreviousPage, || {
                    entries.set(entries.get() + 1);
                    Ok(())
                })
                .is_err());
            assert_eq!(entries.get(), usize::from(!revoke));
        }
    }
    #[test]
    fn actual_pending_smart_erase_checks_each_exact_lower_call_and_stops_on_loss() {
        use image::{DynamicImage, GrayImage, Luma};
        let fixture = super::super::tests::Fixture::new();
        let (_store, handle, token, request) = fixture.setup();
        let mut image = GrayImage::from_pixel(768, 1024, Luma([255]));
        image.put_pixel(5, 2, Luma([0]));
        image.put_pixel(5, 3, Luma([0]));
        let mut png = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let input =
            crate::conversation::SmartEraseInput::new([4, 1, 4, 5], png.into_inner()).unwrap();
        assert!(input.rectangles().len() >= 2);
        let outer = ReaderHandoff::SmartErase(input.clone());
        let admission = Arc::new(handle);
        let trace = Rc::new(std::cell::RefCell::new(Vec::new()));
        let facade = crate::workflow::selected_backend::SelectedBackendFacade::new(
            admission.clone(),
            Box::new(BackendRecorder {
                trace: trace.clone(),
                panic_navigation: false,
            }),
        );
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: facade.binding(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: Some(2),
        });
        let ReaderPreparation::Fresh(context) = admission
            .prepare_reader(
                Some(&token),
                request,
                source.clone(),
                ReaderPlan::new(vec![outer.clone(), ReaderHandoff::NextPage]).unwrap(),
            )
            .unwrap()
        else {
            panic!("fresh expected")
        };
        let mut workflow = crate::workflow::Workflow::with_selected(facade, false);
        assert!(workflow.dispatch_reader(Some(&context), 0, &outer).is_err());
        let (from, to) = input.rectangles()[0];
        assert_eq!(*trace.borrow(), vec![format!("erase:{from:?}:{to:?}")]);
        assert_eq!(source.lower_checks.get(), 2);
        assert!(workflow
            .dispatch_reader(Some(&context), 1, &ReaderHandoff::NextPage)
            .is_err());
        assert_eq!(trace.borrow().len(), 1);
    }
    #[test]
    fn actual_replacement_between_steps_from_another_handle_refuses_later_entry() {
        let fixture = super::super::tests::Fixture::new();
        let (store, handle, token, request) = fixture.setup();
        let admission = Arc::new(handle);
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: crate::workflow::selected_backend::ReaderBackendBinding::for_test(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let ReaderPreparation::Fresh(context) = admission
            .prepare_reader(Some(&token), request, source, plan())
            .unwrap()
        else {
            panic!("fresh expected")
        };
        let entries = Cell::new(0);
        context
            .dispatch(0, &ReaderHandoff::NextPage, || {
                entries.set(entries.get() + 1);
                Ok(())
            })
            .unwrap();
        let snapshot = store
            .selected_snapshot(admission.scope(), MAX_ITEMS)
            .unwrap()
            .unwrap();
        let mut selected = snapshot.transaction.selected.clone();
        selected.transaction_id = Uuid::new_v4();
        let replacement = SelectedAdmission::new(store.clone(), admission.scope().clone()).unwrap();
        replacement
            .activate(
                &snapshot.token,
                crate::storage::selection::SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: snapshot.token.accepted_base_sha256().into(),
                    selected,
                    retained: snapshot.transaction.retained,
                },
                BTreeMap::new(),
            )
            .unwrap();
        assert!(context
            .dispatch(1, &ReaderHandoff::PreviousPage, || {
                entries.set(entries.get() + 1);
                Ok(())
            })
            .is_err());
        assert_eq!(entries.get(), 1);
    }
    #[test]
    fn actual_selected_workflow_has_no_legacy_or_missing_context_backend_route() {
        use crate::workflow::{selected_backend::SelectedBackendFacade, Workflow};
        let fixture = super::super::tests::Fixture::new();
        let (_store, handle, token, request) = fixture.setup();
        let admission = Arc::new(handle);
        let trace = Rc::new(std::cell::RefCell::new(Vec::new()));
        let facade = SelectedBackendFacade::new(
            admission.clone(),
            Box::new(BackendRecorder {
                trace: trace.clone(),
                panic_navigation: false,
            }),
        );
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: facade.binding(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let steps = vec![
            ReaderHandoff::Text("one".into()),
            ReaderHandoff::BodyMode,
            ReaderHandoff::Symbol {
                x: 20,
                y: 20,
                text: "1".into(),
            },
            ReaderHandoff::Progress(Some("progress".into())),
            ReaderHandoff::Erase {
                bounds: [4, 1, 4, 5],
            },
            ReaderHandoff::NextPage,
            ReaderHandoff::PreviousPage,
            ReaderHandoff::Progress(None),
        ];
        let ReaderPreparation::Fresh(context) = admission
            .prepare_reader(
                Some(&token),
                request,
                source,
                ReaderPlan::new(steps.clone()).unwrap(),
            )
            .unwrap()
        else {
            panic!("fresh expected")
        };
        let mut workflow = Workflow::with_selected(facade, false);
        assert!(workflow.render_text("bypass").is_err());
        assert!(workflow.render_qa("bypass").is_err());
        assert!(workflow.set_body_text_mode().is_err());
        assert!(workflow.draw_symbol(20, 20, "1").is_err());
        assert!(workflow.show_progress("bypass").is_err());
        assert!(workflow.clear_progress().is_err());
        assert!(workflow
            .erase_region(&crate::analysis::BoundingBox {
                x: 4,
                y: 1,
                width: 4,
                height: 5
            })
            .is_err());
        assert!(workflow.navigate_to_next_page().is_err());
        assert!(workflow.wait_for_trigger().is_err());
        assert!(workflow.acquire_reader_evidence().is_err());
        assert!(workflow.dispatch_reader(None, 0, &steps[0]).is_err());
        assert!(trace.borrow().is_empty());
        for (ordinal, handoff) in steps.iter().enumerate() {
            workflow
                .dispatch_reader(Some(&context), ordinal, handoff)
                .unwrap();
        }
        assert_eq!(
            *trace.borrow(),
            vec![
                "text:one",
                "body",
                "bitmap:1024:768",
                "progress:Some(\"progress\")",
                "erase:(4, 1):(8, 6)",
                "navigate:Next",
                "navigate:Previous",
                "progress:None"
            ]
        );
        assert!(workflow
            .dispatch_reader(Some(&context), 0, &steps[0])
            .is_err());
        assert_eq!(trace.borrow().len(), 8);
    }
    #[test]
    fn actual_foreign_facade_context_refuses_and_stops_without_backend_entry() {
        use crate::workflow::{selected_backend::SelectedBackendFacade, Workflow};
        let fixture = super::super::tests::Fixture::new();
        let (_store, handle, token, request) = fixture.setup();
        let admission = Arc::new(handle);
        let trace = Rc::new(std::cell::RefCell::new(Vec::new()));
        let original = SelectedBackendFacade::new(
            admission.clone(),
            Box::new(BackendRecorder {
                trace: trace.clone(),
                panic_navigation: false,
            }),
        );
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: original.binding(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let ReaderPreparation::Fresh(context) = admission
            .prepare_reader(Some(&token), request, source, plan())
            .unwrap()
        else {
            panic!("fresh expected")
        };
        let foreign = SelectedBackendFacade::new(
            admission,
            Box::new(BackendRecorder {
                trace: trace.clone(),
                panic_navigation: false,
            }),
        );
        assert!(Workflow::with_selected(foreign, false)
            .dispatch_reader(Some(&context), 0, &ReaderHandoff::NextPage)
            .is_err());
        assert!(Workflow::with_selected(original, false)
            .dispatch_reader(Some(&context), 0, &ReaderHandoff::NextPage)
            .is_err());
        assert!(trace.borrow().is_empty());
    }

    #[test]
    fn selected_facade_native_panic_stops_retry_and_later_step() {
        use crate::workflow::{selected_backend::SelectedBackendFacade, Workflow};
        let fixture = super::super::tests::Fixture::new();
        let (_store, handle, token, request) = fixture.setup();
        let admission = Arc::new(handle);
        let trace = Rc::new(std::cell::RefCell::new(Vec::new()));
        let facade = SelectedBackendFacade::new(
            admission.clone(),
            Box::new(BackendRecorder {
                trace: trace.clone(),
                panic_navigation: true,
            }),
        );
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: facade.binding(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let ReaderPreparation::Fresh(context) = admission
            .prepare_reader(Some(&token), request, source, plan())
            .unwrap()
        else {
            panic!("new publication must mint")
        };
        let mut workflow = Workflow::with_selected(facade, false);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            workflow.dispatch_reader(Some(&context), 0, &ReaderHandoff::NextPage)
        }))
        .is_err());
        assert!(workflow
            .dispatch_reader(Some(&context), 0, &ReaderHandoff::NextPage)
            .is_err());
        assert!(workflow
            .dispatch_reader(Some(&context), 1, &ReaderHandoff::PreviousPage)
            .is_err());
        assert_eq!(trace.borrow().len(), 1);
    }

    #[test]
    fn actual_attempt_retains_fresh_context_and_historical_attempt_never_dispatches() {
        use crate::workflow::{selected_backend::SelectedBackendFacade, Workflow};
        let fixture = super::super::tests::Fixture::new();
        let (store, handle, token, request) = fixture.setup();
        let ledger = Ledger::new(store);
        let draft = ledger
            .inspect(request.conversation, false)
            .unwrap()
            .into_iter()
            .find_map(|record| match record {
                Record::Turn(turn) if turn.id == request.turn => Some(turn),
                _ => None,
            })
            .unwrap();
        assert_eq!(draft.outcome, Outcome::Generated);
        let admission = Arc::new(handle);
        let trace = Rc::new(std::cell::RefCell::new(Vec::new()));
        let facade = SelectedBackendFacade::new(
            admission.clone(),
            Box::new(BackendRecorder {
                trace: trace.clone(),
                panic_navigation: false,
            }),
        );
        let source = Rc::new(RecordingSource {
            source: request.source.clone(),
            binding: facade.binding(),
            revoked: Cell::new(false),
            plan_checks: Cell::new(0),
            lower_checks: Cell::new(0),
            refuse_lower: None,
        });
        let evidence = request.evidence[0].record_id;
        let fresh = admission
            .prepare_reader(Some(&token), request.clone(), source.clone(), plan())
            .unwrap();
        let mut workflow = Workflow::with_selected(facade, false);
        crate::workflow::assert_attachment_behavior(
            fresh,
            draft.clone(),
            evidence,
            &ledger,
            &mut workflow,
            true,
        );
        assert_eq!(trace.borrow().len(), 1);
        source.revoked.set(true);
        let historical = admission
            .prepare_reader(None, request, source, plan())
            .unwrap();
        crate::workflow::assert_attachment_behavior(
            historical,
            draft,
            evidence,
            &ledger,
            &mut workflow,
            false,
        );
        assert_eq!(trace.borrow().len(), 1);
    }
}

pub(crate) enum ReaderPreparation {
    Fresh(ReaderContext),
    Historical(HistoricalIntent),
}

pub(crate) struct ReaderContext {
    admission: Arc<SelectedAdmission>,
    token: SelectionToken,
    request: PendingIntentRequest,
    original: HistoricalIntent,
    source: Rc<dyn ReaderSourceAdmission>,
    plan: ReaderPlan,
    dispatch: DispatchState,
}

impl ReaderContext {
    pub(crate) fn matches_attempt(&self, conversation: Uuid, turn: Uuid, evidence: Uuid) -> bool {
        self.request.conversation == conversation
            && self.request.turn == turn
            && self.request.evidence.iter().any(|reference| {
                reference.namespace == Namespace::Conversation && reference.record_id == evidence
            })
    }

    #[cfg(test)]
    fn dispatch<T>(
        &self,
        ordinal: usize,
        handoff: &ReaderHandoff,
        submit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.dispatch_composite(ordinal, handoff, |permit| permit.step(0, handoff, submit))
    }
    // Only the parent pending module can call this constructor.
    pub(super) fn mint(
        admission: Arc<SelectedAdmission>,
        token: SelectionToken,
        request: PendingIntentRequest,
        original: HistoricalIntent,
        source: Rc<dyn ReaderSourceAdmission>,
        plan: ReaderPlan,
    ) -> Self {
        Self {
            admission,
            token,
            request,
            original,
            source,
            plan,
            dispatch: DispatchState::default(),
        }
    }

    /// Private composite seam; the future selected facade owns the callback.
    /// No public backend or effect permission is exposed by this context.
    pub(crate) fn dispatch_backend<T>(
        &self,
        admission: &Arc<SelectedAdmission>,
        binding: &crate::workflow::selected_backend::ReaderBackendBinding,
        ordinal: usize,
        handoff: &ReaderHandoff,
        submit: impl FnOnce(&mut ReaderPermit<'_>) -> Result<T>,
    ) -> Result<T> {
        if !Arc::ptr_eq(&self.admission, admission)
            || !self.source.backend_binding().matches(binding)
        {
            self.dispatch.stop();
            bail!("Reader context belongs to a foreign backend or admission");
        }
        self.dispatch_composite(ordinal, handoff, submit)
    }
    fn dispatch_composite<T>(
        &self,
        ordinal: usize,
        handoff: &ReaderHandoff,
        submit: impl FnOnce(&mut ReaderPermit<'_>) -> Result<T>,
    ) -> Result<T> {
        let reservation = self.dispatch.reserve(&self.plan, ordinal, handoff)?;
        self.admission
            .with_current_store(&self.token, |store, snapshot| {
                crate::conversation::reader_uncertainty::validate(
                    store,
                    snapshot,
                    &self.request,
                    Some(&self.original),
                )?;
                self.source.verify_current(&self.request)?;
                self.source.verify_handoff(&self.request, handoff)?;
                let entered = reservation.enter()?;
                let expected = match handoff {
                    ReaderHandoff::SmartErase(input) => input
                        .rectangles()
                        .iter()
                        .map(|&(from, to)| ReaderHandoff::Erase {
                            bounds: [from.0, from.1, to.0 - from.0, to.1 - from.1],
                        })
                        .collect(),
                    _ => vec![handoff.clone()],
                };
                let mut permit = ReaderPermit {
                    context: self,
                    store,
                    outer: handoff,
                    entered: &entered,
                    expected,
                    next: 0,
                    failed: false,
                };
                let value = submit(&mut permit)?;
                ensure!(
                    !permit.failed && permit.next == permit.expected.len(),
                    "Reader composite incomplete or refused"
                );
                drop(permit);
                entered.submitted()?;
                Ok(value)
            })
    }
}

/// Exists only during the held-gate outer composite; contains no backend handle.
pub(crate) struct ReaderPermit<'a> {
    context: &'a ReaderContext,
    store: &'a Store,
    outer: &'a ReaderHandoff,
    entered: &'a Entered<'a>,
    expected: Vec<ReaderHandoff>,
    next: usize,
    failed: bool,
}
impl ReaderPermit<'_> {
    pub(crate) fn step<T>(
        &mut self,
        ordinal: usize,
        lower: &ReaderHandoff,
        submit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let valid =
            !self.failed && ordinal == self.next && self.expected.get(ordinal) == Some(lower);
        self.failed = true;
        ensure!(valid, "Reader lower call changed, duplicated or stopped");
        self.entered.ensure_current()?;
        let current = self
            .store
            .selected_snapshot(self.context.admission.scope(), MAX_ITEMS)?
            .context("Reader lower selection absent")?;
        ensure!(
            current.token == self.context.token,
            "Reader lower selection replaced or stale"
        );
        crate::conversation::reader_uncertainty::validate(
            self.store,
            &current,
            &self.context.request,
            Some(&self.context.original),
        )?;
        self.context.source.verify_current(&self.context.request)?;
        self.context
            .source
            .verify_lower(&self.context.request, self.outer, ordinal, lower)?;
        self.entered.ensure_current()?;
        self.next += 1;
        let value = submit()?;
        self.entered.ensure_current()?;
        self.failed = false;
        Ok(value)
    }
}
