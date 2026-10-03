//! Explicit experimental SDK consumer seam; never installed Reader write authority.
//! No native adapter is qualified. Model preparation cannot bind or render a page.
use remarkable_open_sdk::{
    navigation::{CreationHandoffOutcome, LogicalDirection, NavigationOutcome, NavigationRequest},
    CreationOutcome, CreationReceipt, CreationRequest, EvidenceOrigin, OperationId,
    PageObservation, Platform, UnsupportedReason,
};
use std::collections::HashSet;

/// In-memory experimental attempts, not the production durable journal. Retain
/// this across calls; replacing it cannot restore live or write authority.
#[derive(Default)]
pub struct AcquisitionSession {
    attempted: HashSet<OperationId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcquisitionOutcome {
    Unsupported(UnsupportedReason),
    CanceledBeforeCreation,
    /// May include a committed page. No redispatch or write is authorized.
    ReconcileRequired,
    /// Contract-model result only. No renderer/binding callback is accepted here.
    SyntheticPrepared {
        creation: Box<CreationReceipt>,
        target: PageObservation,
    },
}

/// One creation request, one explicit handoff, zero or one logical Next. Repeated
/// operations stop in the retained session and cannot refresh completion. This
/// in-memory seam does not substitute for production durable reconciliation.
pub fn acquire_created_target(
    session: &mut AcquisitionSession,
    platform: &mut impl Platform,
    creation: &CreationRequest,
    navigation_operation: OperationId,
    canceled: bool,
) -> AcquisitionOutcome {
    if !session.attempted.insert(creation.operation()) {
        return AcquisitionOutcome::ReconcileRequired;
    }
    let receipt = match platform.create_after(creation, canceled) {
        CreationOutcome::Unsupported(reason) => return AcquisitionOutcome::Unsupported(reason),
        CreationOutcome::CanceledBeforeDispatch => {
            return AcquisitionOutcome::CanceledBeforeCreation
        }
        CreationOutcome::Committed(receipt) => receipt,
        _ => return AcquisitionOutcome::ReconcileRequired,
    };
    // Receipt alone is historical. Explicit adapter handoff must retain observer,
    // input epoch/session and qualify the legitimate structural/visit transition.
    let observed = match platform.acquire_after_creation(&receipt, canceled) {
        CreationHandoffOutcome::Unsupported(reason) => {
            return AcquisitionOutcome::Unsupported(reason)
        }
        CreationHandoffOutcome::SyntheticAcquired(observed) => observed,
        _ => return AcquisitionOutcome::ReconcileRequired,
    };
    if receipt.request() != creation
        || observed.order() != receipt.after_order()
        || observed.origin() != EvidenceOrigin::Synthetic
        || receipt.origin() != EvidenceOrigin::Synthetic
    {
        return AcquisitionOutcome::ReconcileRequired;
    }
    if observed.page() == receipt.target() {
        return AcquisitionOutcome::SyntheticPrepared {
            creation: Box::new(receipt),
            target: observed,
        };
    }
    if observed.page() != creation.source().page() {
        return AcquisitionOutcome::ReconcileRequired;
    }
    let Ok(request) = NavigationRequest::new(
        navigation_operation,
        observed,
        LogicalDirection::Next,
        receipt.target(),
    ) else {
        return AcquisitionOutcome::ReconcileRequired;
    };
    match platform.navigate(&request, canceled) {
        NavigationOutcome::Unsupported(reason) => AcquisitionOutcome::Unsupported(reason),
        NavigationOutcome::SyntheticVerified(navigation)
            if navigation.request() == &request
                && navigation.observed().page() == receipt.target()
                && navigation.observed().order() == receipt.after_order()
                && navigation.observed().origin() == EvidenceOrigin::Synthetic =>
        {
            AcquisitionOutcome::SyntheticPrepared {
                creation: Box::new(receipt),
                target: navigation.observed().clone(),
            }
        }
        _ => AcquisitionOutcome::ReconcileRequired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remarkable_open_sdk::{
        mock::{ExecutionEvent, MockPlatform, NavigationFault},
        TargetAllocation, UnqualifiedPlatform, Uuid,
    };
    fn id(n: u8) -> Uuid {
        Uuid::parse(&format!("{n:08x}-1111-1111-1111-111111111111")).unwrap()
    }
    fn platform() -> MockPlatform {
        MockPlatform::new(id(1), id(2), vec![id(3), id(4)], id(3))
    }
    fn request(p: &MockPlatform) -> CreationRequest {
        CreationRequest::new(
            OperationId(id(9)),
            p.observe_page().unwrap(),
            TargetAllocation::ClientSelected(id(5)),
        )
    }
    struct HandoffFault {
        inner: MockPlatform,
        canceled: bool,
        order_changed: bool,
        creations: usize,
    }
    impl Platform for HandoffFault {
        fn observe_page(&self) -> Result<PageObservation, remarkable_open_sdk::ObservationFailure> {
            self.inner.observe_page()
        }
        fn create_after(&mut self, request: &CreationRequest, canceled: bool) -> CreationOutcome {
            self.creations += 1;
            let result = self.inner.create_after(request, canceled);
            if self.order_changed {
                if let CreationOutcome::Committed(receipt) = &result {
                    self.inner.remove_page(receipt.target().page);
                }
            }
            result
        }
        fn reconcile_creation(
            &self,
            request: &CreationRequest,
        ) -> remarkable_open_sdk::Reconciliation {
            self.inner.reconcile_creation(request)
        }
        fn acquire_after_creation(
            &mut self,
            receipt: &CreationReceipt,
            canceled: bool,
        ) -> CreationHandoffOutcome {
            self.inner
                .acquire_after_creation(receipt, canceled || self.canceled)
        }
        fn navigate(&mut self, request: &NavigationRequest, canceled: bool) -> NavigationOutcome {
            self.inner.navigate(request, canceled)
        }
    }
    #[test]
    fn cancellation_or_order_loss_at_handoff_preserves_no_redispatch() {
        for (canceled, order_changed) in [(true, false), (false, true)] {
            let mut p = HandoffFault {
                inner: platform(),
                canceled,
                order_changed,
                creations: 0,
            };
            let r = request(&p.inner);
            let mut session = AcquisitionSession::default();
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            assert_eq!(p.creations, 1);
            assert_eq!(p.inner.gesture_count(), 0);
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(11)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            assert_eq!(p.creations, 1);
            assert_eq!(p.inner.gesture_count(), 0);
        }
    }
    #[test]
    fn source_or_auto_selected_target_uses_one_or_zero_gestures() {
        for selected in [false, true] {
            let mut p = platform();
            let mut session = AcquisitionSession::default();
            let r = request(&p);
            p.set_creation_selects_target(selected);
            let AcquisitionOutcome::SyntheticPrepared { creation, target } =
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false)
            else {
                panic!("model preparation")
            };
            assert_eq!(target.page(), creation.target());
            assert_eq!(target.order(), creation.after_order());
            assert_eq!(target.origin(), EvidenceOrigin::Synthetic);
            assert_eq!(p.gesture_count(), usize::from(!selected));
            assert_eq!(p.pages(), &[id(3), id(5), id(4)]);
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            assert_eq!(p.gesture_count(), usize::from(!selected));
            assert_eq!(p.pages().len(), 3);
        }
    }
    #[test]
    fn creation_cancel_uncertainty_and_guard_loss_never_navigate() {
        let mut p = platform();
        let mut session = AcquisitionSession::default();
        let r = request(&p);
        assert_eq!(
            acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), true),
            AcquisitionOutcome::CanceledBeforeCreation
        );
        assert_eq!(p.pages().len(), 2);
        assert_eq!(p.gesture_count(), 0);
        for event in [
            ExecutionEvent::ExternalInput,
            ExecutionEvent::ReverseOrder,
            ExecutionEvent::CommitThenLoseReply,
        ] {
            let mut p = platform();
            let mut session = AcquisitionSession::default();
            let r = request(&p);
            p.inject_at_execution(event);
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            assert_eq!(p.gesture_count(), 0);
            let page_count = p.pages().len();
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            assert_eq!(p.gesture_count(), 0);
            assert_eq!(p.pages().len(), page_count);
        }
    }
    #[test]
    fn navigation_faults_stop_without_second_creation_or_gesture() {
        for fault in [
            NavigationFault::BeforeDispatchInput,
            NavigationFault::BeforeDispatchOrder,
            NavigationFault::BeforeDispatchSession,
            NavigationFault::BeforeDispatchVisit,
            NavigationFault::NoMovement,
            NavigationFault::WrongNeighbor,
            NavigationFault::AfterDispatchInput,
            NavigationFault::AfterDispatchSession,
            NavigationFault::AfterDispatchOrder,
            NavigationFault::AfterDispatchVisit,
            NavigationFault::CanceledAfterDispatch,
            NavigationFault::Deadline,
            NavigationFault::UnreadyPixels,
        ] {
            let mut p = platform();
            let mut session = AcquisitionSession::default();
            let r = request(&p);
            p.inject_navigation_fault(fault);
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            let count = p.gesture_count();
            assert!(count <= 1);
            assert_eq!(p.pages().len(), 3);
            assert_eq!(
                acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
                AcquisitionOutcome::ReconcileRequired
            );
            assert_eq!(p.gesture_count(), count);
            assert_eq!(p.pages().len(), 3);
        }
    }
    #[test]
    fn unsupported_runtime_and_unqualified_model_do_not_operate() {
        let mut p = platform();
        let mut session = AcquisitionSession::default();
        let r = request(&p);
        assert_eq!(
            acquire_created_target(
                &mut AcquisitionSession::default(),
                &mut UnqualifiedPlatform,
                &r,
                OperationId(id(10)),
                false
            ),
            AcquisitionOutcome::Unsupported(UnsupportedReason::NoNativeAdapter)
        );
        p.set_guard_enforcement(false);
        assert_eq!(
            acquire_created_target(&mut session, &mut p, &r, OperationId(id(10)), false),
            AcquisitionOutcome::Unsupported(UnsupportedReason::UnqualifiedMechanism)
        );
        assert_eq!(p.pages().len(), 2);
        assert_eq!(p.gesture_count(), 0);
    }
}
