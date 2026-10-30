//! Synthetic acquisition decisions with read-only historical association input.
//! Neither modeled nor stored inputs confer native, Store or render authority.
//! Supplied projections are model inputs, not current-device qualification.
use remarkable_open_sdk::{
    navigation::{LogicalDirection, NavigationReceipt},
    EvidenceOrigin, PageKey, PageObservation, Uuid,
};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssociationState {
    RelevantModelAssociation,
    Missing,
    Stale,
    Imported,
    Historical,
    Conflicted,
    Tombstoned,
    OtherConversation,
    /// Stored inspection failed or lacks a complete historical association.
    UnresolvedStored,
}

/// Observation-bound model input without current-state or effect authority.
#[derive(Clone, Debug)]
pub struct AssociationWitness {
    observation: PageObservation,
    page: PageKey,
    conversation: Uuid,
    state: AssociationState,
}
impl AssociationWitness {
    pub fn modeled(
        observation: PageObservation,
        page: PageKey,
        conversation: Uuid,
        state: AssociationState,
    ) -> Self {
        Self {
            observation,
            page,
            conversation,
            state,
        }
    }
}

/// Implementers must provide read-only projections. This contractual boundary
/// does not sandbox arbitrary trait implementations or qualify a Store adapter.
pub trait ReadOnlyBindingLookup {
    fn lookup(&self, observation: &PageObservation, conversation: Uuid) -> AssociationWitness;
}

/// Reads the existing historical ledger, never opening a Store or minting
/// relevant/current associations from saved data. Installed Reader is unchanged.
pub struct HistoricalLedgerLookup<'a> {
    ledger: &'a crate::conversation::Ledger,
}
impl<'a> HistoricalLedgerLookup<'a> {
    pub fn new(ledger: &'a crate::conversation::Ledger) -> Self {
        Self { ledger }
    }
}
impl ReadOnlyBindingLookup for HistoricalLedgerLookup<'_> {
    fn lookup(&self, observation: &PageObservation, conversation: Uuid) -> AssociationWitness {
        use crate::conversation::{
            HistoricalPageAssociationInspection as Inspection,
            HistoricalPageAssociationUnresolved as Unresolved,
        };
        let page = observation.page();
        let state = match self.ledger.inspect_page_association(
            crate::storage::Uuid::from_bytes(*conversation.as_bytes()),
            crate::storage::Uuid::from_bytes(*page.document.as_bytes()),
            crate::storage::Uuid::from_bytes(*page.page.as_bytes()),
        ) {
            Ok(Inspection::HistoricalCandidate(_)) => AssociationState::Historical,
            Ok(Inspection::Unresolved(
                Unresolved::RootConflicted | Unresolved::BindingConflicted,
            )) => AssociationState::Conflicted,
            Ok(Inspection::Unresolved(
                Unresolved::RootTombstoned | Unresolved::BindingTombstoned,
            )) => AssociationState::Tombstoned,
            Ok(Inspection::Unresolved(Unresolved::OtherConversation)) => {
                AssociationState::OtherConversation
            }
            Ok(Inspection::Unresolved(_)) | Err(_) => AssociationState::UnresolvedStored,
        };
        AssociationWitness::modeled(observation.clone(), page, conversation, state)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recognition {
    OrdinarySource,
    ConfirmedNonBuddy,
    RelevantBuddy,
    LegacyBuddy,
    OtherBuddy,
    Blank,
    MissingCache,
    Unknown,
    Failed,
}

#[derive(Clone, Debug)]
pub struct RecognitionWitness {
    observation: PageObservation,
    page: PageKey,
    conversation: Uuid,
    recognition: Recognition,
}
impl RecognitionWitness {
    pub fn modeled(
        observation: PageObservation,
        page: PageKey,
        conversation: Uuid,
        recognition: Recognition,
    ) -> Self {
        Self {
            observation,
            page,
            conversation,
            recognition,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnresolvedReason {
    ObservationGuard,
    WitnessMismatch,
    LogicalClaim,
    RecognitionUncertain,
    NavigationGuard,
    ContinuityGuard,
}

#[derive(Clone, Debug)]
pub struct InspectionPlan {
    original: PageObservation,
    conversation: Uuid,
    neighbor: PageKey,
}
impl InspectionPlan {
    pub fn original_source(&self) -> PageKey {
        self.original.page()
    }
    pub fn neighbor(&self) -> PageKey {
        self.neighbor
    }
}

/// Historical placement only. No target allocation or creation-request conversion.
#[derive(Clone, Debug)]
enum PlacementContinuity {
    UninspectedEnd,
    InspectedSuccessor(PageObservation),
}

#[derive(Clone, Debug)]
pub struct PlacementPlan {
    original: PageObservation,
    conversation: Uuid,
    neighbor: Option<PageKey>,
    continuity: PlacementContinuity,
}
impl PlacementPlan {
    pub fn original_source(&self) -> PageKey {
        self.original.page()
    }
    pub fn expected_order(&self) -> &[Uuid] {
        self.original.order()
    }
    pub fn conversation(&self) -> Uuid {
        self.conversation
    }
    pub fn before_neighbor(&self) -> Option<PageKey> {
        self.neighbor
    }
}

#[derive(Clone, Debug)]
pub struct SyntheticPageCandidate {
    observation: PageObservation,
    conversation: Uuid,
}
impl SyntheticPageCandidate {
    pub fn observation(&self) -> &PageObservation {
        &self.observation
    }
    pub fn conversation(&self) -> Uuid {
        self.conversation
    }
}

#[derive(Clone, Debug)]
pub enum SyntheticDecision {
    ReuseCandidate(SyntheticPageCandidate),
    RecognitionNeeded(SyntheticPageCandidate),
    AdoptionRequired(SyntheticPageCandidate),
    Unresolved(UnresolvedReason),
    InspectImmediateSuccessor(InspectionPlan),
    InsertionPlan(PlacementPlan),
}

fn synthetic(observation: &PageObservation) -> bool {
    observation.origin() == EvidenceOrigin::Synthetic
}

fn valid_order(observation: &PageObservation) -> bool {
    !observation.order().is_empty()
        && observation.order().iter().collect::<HashSet<_>>().len() == observation.order().len()
        && observation.order().contains(&observation.page().page)
}

/// Contextualize a supplied legacy enum as model evidence only. The enum has
/// no capture provenance, conversation relevance or current-device qualification.
/// This neither calls nor changes the legacy classifier or installed workflow.
pub fn model_legacy_answer_classification(
    observation: &PageObservation,
    conversation: Uuid,
    classification: super::AnswerPageType,
) -> Result<RecognitionWitness, UnresolvedReason> {
    if !synthetic(observation) || !valid_order(observation) {
        return Err(UnresolvedReason::ObservationGuard);
    }
    let recognition = match classification {
        super::AnswerPageType::ExistingQA => Recognition::LegacyBuddy,
        super::AnswerPageType::Blank => Recognition::Blank,
        super::AnswerPageType::Invalid => Recognition::Unknown,
    };
    Ok(RecognitionWitness::modeled(
        observation.clone(),
        observation.page(),
        conversation,
        recognition,
    ))
}

fn witness_matches(
    observed: &PageObservation,
    page: PageKey,
    conversation: Uuid,
    expected: &PageObservation,
    desired: Uuid,
) -> bool {
    synthetic(observed)
        && observed == expected
        && page == expected.page()
        && conversation == desired
}

// None means that the required positive classification permits the next step.
fn assess(
    observation: &PageObservation,
    conversation: Uuid,
    lookup: &impl ReadOnlyBindingLookup,
    recognition: Option<&RecognitionWitness>,
    positive: Recognition,
) -> Option<SyntheticDecision> {
    if recognition.is_some_and(|w| {
        !witness_matches(
            &w.observation,
            w.page,
            w.conversation,
            observation,
            conversation,
        )
    }) {
        return Some(SyntheticDecision::Unresolved(
            UnresolvedReason::WitnessMismatch,
        ));
    }
    let association = lookup.lookup(observation, conversation);
    if !witness_matches(
        &association.observation,
        association.page,
        association.conversation,
        observation,
        conversation,
    ) {
        return Some(SyntheticDecision::Unresolved(
            UnresolvedReason::WitnessMismatch,
        ));
    }
    let candidate = || SyntheticPageCandidate {
        observation: observation.clone(),
        conversation,
    };
    match association.state {
        AssociationState::RelevantModelAssociation => {
            if recognition.is_some_and(|w| {
                matches!(
                    w.recognition,
                    Recognition::OrdinarySource
                        | Recognition::ConfirmedNonBuddy
                        | Recognition::OtherBuddy
                )
            }) {
                return Some(SyntheticDecision::Unresolved(
                    UnresolvedReason::LogicalClaim,
                ));
            }
            Some(SyntheticDecision::ReuseCandidate(candidate()))
        }
        AssociationState::Missing => match recognition.map(|w| w.recognition) {
            None => Some(SyntheticDecision::RecognitionNeeded(candidate())),
            Some(Recognition::RelevantBuddy | Recognition::LegacyBuddy) => {
                Some(SyntheticDecision::AdoptionRequired(candidate()))
            }
            Some(value) if value == positive => None,
            _ => Some(SyntheticDecision::Unresolved(
                UnresolvedReason::RecognitionUncertain,
            )),
        },
        _ => Some(SyntheticDecision::Unresolved(
            UnresolvedReason::LogicalClaim,
        )),
    }
}

/// Current first, with no Platform/effect callback and no successor lookup on reuse.
pub fn begin_current(
    original: &PageObservation,
    fresh: &PageObservation,
    conversation: Uuid,
    lookup: &impl ReadOnlyBindingLookup,
    recognition: Option<&RecognitionWitness>,
) -> SyntheticDecision {
    if !synthetic(original) || !synthetic(fresh) || original != fresh || !valid_order(original) {
        return SyntheticDecision::Unresolved(UnresolvedReason::ObservationGuard);
    }
    if let Some(decision) = assess(
        original,
        conversation,
        lookup,
        recognition,
        Recognition::OrdinarySource,
    ) {
        return decision;
    }
    let index = original
        .order()
        .iter()
        .position(|id| *id == original.page().page)
        .unwrap();
    match original.order().get(index + 1) {
        Some(page) => SyntheticDecision::InspectImmediateSuccessor(InspectionPlan {
            original: original.clone(),
            conversation,
            neighbor: PageKey {
                document: original.page().document,
                page: *page,
            },
        }),
        None => SyntheticDecision::InsertionPlan(PlacementPlan {
            original: original.clone(),
            conversation,
            neighbor: None,
            continuity: PlacementContinuity::UninspectedEnd,
        }),
    }
}

/// SDK receipt proves a legitimate visit transition; fresh equality closes it.
/// This produces a placement plan, never dispatches the stale original guard.
pub fn complete_successor(
    anchor: &InspectionPlan,
    receipt: &NavigationReceipt,
    fresh: &PageObservation,
    lookup: &impl ReadOnlyBindingLookup,
    recognition: Option<&RecognitionWitness>,
) -> SyntheticDecision {
    let request = receipt.request();
    let observed = receipt.observed();
    if !synthetic(request.source())
        || !synthetic(observed)
        || !synthetic(fresh)
        || request.source() != &anchor.original
        || request.direction() != LogicalDirection::Next
        || request.target() != anchor.neighbor
        || observed.page() != anchor.neighbor
        || observed.order() != anchor.original.order()
        || observed != fresh
        || !valid_order(observed)
    {
        return SyntheticDecision::Unresolved(UnresolvedReason::NavigationGuard);
    }
    if let Some(decision) = assess(
        observed,
        anchor.conversation,
        lookup,
        recognition,
        Recognition::ConfirmedNonBuddy,
    ) {
        return decision;
    }
    SyntheticDecision::InsertionPlan(PlacementPlan {
        original: anchor.original.clone(),
        conversation: anchor.conversation,
        neighbor: Some(anchor.neighbor),
        continuity: PlacementContinuity::InspectedSuccessor(observed.clone()),
    })
}

/// Point-in-time SDK model continuity only: no logical/recognition refresh or
/// collection-freshness oracle, native qualification, or effect permission.
#[derive(Clone, Debug)]
pub struct SyntheticSourceContinuity {
    placement: PlacementPlan,
    source: PageObservation,
}
impl SyntheticSourceContinuity {
    pub fn historical_placement(&self) -> &PlacementPlan {
        &self.placement
    }
    pub fn matched_source(&self) -> &PageObservation {
        &self.source
    }
    pub fn conversation(&self) -> Uuid {
        self.placement.conversation
    }
}

/// Match supplied SDK evidence without dispatching a return or refreshing any
/// association/recognition predicate. An older clone may also compare equal;
/// future effectful consumers still need separate current logical/native guards.
pub fn match_source_continuity(
    plan: &PlacementPlan,
    supplied_source: &PageObservation,
    return_receipt: Option<&NavigationReceipt>,
) -> Result<SyntheticSourceContinuity, UnresolvedReason> {
    if !synthetic(&plan.original)
        || !synthetic(supplied_source)
        || !valid_order(&plan.original)
        || !valid_order(supplied_source)
    {
        return Err(UnresolvedReason::ContinuityGuard);
    }
    let matches = match (&plan.continuity, plan.neighbor, return_receipt) {
        (PlacementContinuity::UninspectedEnd, None, None) => {
            supplied_source == &plan.original
                && plan.original.order().last() == Some(&plan.original.page().page)
        }
        (PlacementContinuity::InspectedSuccessor(successor), Some(neighbor), Some(receipt)) => {
            let request = receipt.request();
            let observed = receipt.observed();
            let adjacent = plan
                .original
                .order()
                .iter()
                .position(|page| *page == plan.original.page().page)
                .and_then(|index| plan.original.order().get(index + 1))
                .copied()
                == Some(neighbor.page);
            synthetic(successor)
                && synthetic(request.source())
                && synthetic(observed)
                && successor.page() == neighbor
                && neighbor.document == plan.original.page().document
                && successor.order() == plan.original.order()
                && adjacent
                && request.source() == successor
                && request.direction() == LogicalDirection::Previous
                && request.target() == plan.original.page()
                && observed.page() == plan.original.page()
                && observed.order() == plan.original.order()
                && supplied_source == observed
        }
        _ => false,
    };
    if !matches {
        return Err(UnresolvedReason::ContinuityGuard);
    }
    Ok(SyntheticSourceContinuity {
        placement: plan.clone(),
        source: supplied_source.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use remarkable_open_sdk::{
        mock::{MockPlatform, NavigationFault},
        navigation::{NavigationOutcome, NavigationRequest},
        CreationOutcome, CreationRequest, OperationId, Platform, TargetAllocation,
    };
    use std::cell::RefCell;

    fn id(n: u8) -> Uuid {
        Uuid::parse(&format!("{n:08x}-1111-1111-1111-111111111111")).unwrap()
    }
    fn platform() -> MockPlatform {
        MockPlatform::new(id(1), id(2), vec![id(3), id(4)], id(3))
    }
    struct Lookup {
        state: AssociationState,
        calls: RefCell<Vec<PageKey>>,
        supplied: Option<AssociationWitness>,
    }
    impl Lookup {
        fn new(state: AssociationState) -> Self {
            Self {
                state,
                calls: RefCell::new(vec![]),
                supplied: None,
            }
        }
    }
    impl ReadOnlyBindingLookup for Lookup {
        fn lookup(&self, observation: &PageObservation, conversation: Uuid) -> AssociationWitness {
            self.calls.borrow_mut().push(observation.page());
            self.supplied.clone().unwrap_or_else(|| {
                AssociationWitness::modeled(
                    observation.clone(),
                    observation.page(),
                    conversation,
                    self.state,
                )
            })
        }
    }
    fn recognition(observation: &PageObservation, value: Recognition) -> RecognitionWitness {
        RecognitionWitness::modeled(observation.clone(), observation.page(), id(8), value)
    }
    fn anchor(observation: &PageObservation) -> InspectionPlan {
        let SyntheticDecision::InspectImmediateSuccessor(plan) = begin_current(
            observation,
            observation,
            id(8),
            &Lookup::new(AssociationState::Missing),
            Some(&recognition(observation, Recognition::OrdinarySource)),
        ) else {
            panic!("synthetic inspection plan")
        };
        plan
    }
    fn navigate(
        p: &mut MockPlatform,
        source: &PageObservation,
        direction: LogicalDirection,
        target: PageKey,
        op: u8,
    ) -> NavigationReceipt {
        let request =
            NavigationRequest::new(OperationId(id(op)), source.clone(), direction, target).unwrap();
        let NavigationOutcome::SyntheticVerified(receipt) = p.navigate(&request, false) else {
            panic!("SDK fixture navigation")
        };
        receipt
    }

    #[test]
    fn current_reuse_queries_only_current_and_changes_nothing() {
        let p = platform();
        let original = p.observe_page().unwrap();
        let lookup = Lookup::new(AssociationState::RelevantModelAssociation);
        assert!(
            matches!(begin_current(&original, &original, id(8), &lookup, None), SyntheticDecision::ReuseCandidate(ref observed) if observed.observation() == &original && observed.conversation() == id(8))
        );
        assert_eq!(*lookup.calls.borrow(), vec![original.page()]);
        assert_eq!(p.observe_page().unwrap(), original);
        assert_eq!(p.pages(), &[id(3), id(4)]);
        assert_eq!(p.gesture_count(), 0);
    }

    #[test]
    fn positive_recognition_contradicting_association_refuses_in_both_phases() {
        let mut p = platform();
        let original = p.observe_page().unwrap();
        let plan = anchor(&original);
        let receipt = navigate(
            &mut p,
            &original,
            LogicalDirection::Next,
            plan.neighbor(),
            10,
        );
        let fresh = p.observe_page().unwrap();
        let lookup = Lookup::new(AssociationState::RelevantModelAssociation);
        for value in [
            Recognition::OrdinarySource,
            Recognition::ConfirmedNonBuddy,
            Recognition::OtherBuddy,
        ] {
            assert!(matches!(
                begin_current(
                    &original,
                    &original,
                    id(8),
                    &lookup,
                    Some(&recognition(&original, value))
                ),
                SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
            ));
            assert!(matches!(
                complete_successor(
                    &plan,
                    &receipt,
                    &fresh,
                    &lookup,
                    Some(&recognition(&fresh, value))
                ),
                SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
            ));
        }
        assert_eq!(p.observe_page().unwrap(), fresh);
        assert_eq!(p.gesture_count(), 1);
        assert_eq!(p.pages(), &[id(3), id(4)]);
    }

    #[test]
    fn sdk_rejects_nonadjacent_receipt_request_without_dispatch() {
        let p = MockPlatform::new(id(1), id(2), vec![id(3), id(4), id(5)], id(3));
        let original = p.observe_page().unwrap();
        assert!(NavigationRequest::new(
            OperationId(id(10)),
            original.clone(),
            LogicalDirection::Next,
            PageKey {
                document: id(2),
                page: id(5)
            }
        )
        .is_err());
        assert_eq!(p.observe_page().unwrap(), original);
        assert_eq!(p.gesture_count(), 0);
    }

    #[test]
    fn missing_needs_recognition_and_unbound_buddy_needs_adoption() {
        let original = platform().observe_page().unwrap();
        let lookup = Lookup::new(AssociationState::Missing);
        assert!(matches!(
            begin_current(&original, &original, id(8), &lookup, None),
            SyntheticDecision::RecognitionNeeded(_)
        ));
        for value in [Recognition::RelevantBuddy, Recognition::LegacyBuddy] {
            assert!(matches!(
                begin_current(
                    &original,
                    &original,
                    id(8),
                    &lookup,
                    Some(&recognition(&original, value))
                ),
                SyntheticDecision::AdoptionRequired(_)
            ));
        }
        for value in [
            Recognition::Blank,
            Recognition::MissingCache,
            Recognition::Unknown,
            Recognition::Failed,
            Recognition::OtherBuddy,
            Recognition::ConfirmedNonBuddy,
        ] {
            assert!(matches!(
                begin_current(
                    &original,
                    &original,
                    id(8),
                    &lookup,
                    Some(&recognition(&original, value))
                ),
                SyntheticDecision::Unresolved(_)
            ));
        }
    }

    #[test]
    fn ordinary_recognition_cannot_erase_any_logical_claim() {
        let original = platform().observe_page().unwrap();
        for state in [
            AssociationState::Stale,
            AssociationState::Imported,
            AssociationState::Historical,
            AssociationState::Conflicted,
            AssociationState::Tombstoned,
            AssociationState::OtherConversation,
            AssociationState::UnresolvedStored,
        ] {
            assert!(matches!(
                begin_current(
                    &original,
                    &original,
                    id(8),
                    &Lookup::new(state),
                    Some(&recognition(&original, Recognition::OrdinarySource))
                ),
                SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
            ));
        }
    }

    #[test]
    fn exact_end_is_historical_plan_with_no_allocated_target() {
        let p = MockPlatform::new(id(1), id(2), vec![id(3)], id(3));
        let original = p.observe_page().unwrap();
        let SyntheticDecision::InsertionPlan(plan) = begin_current(
            &original,
            &original,
            id(8),
            &Lookup::new(AssociationState::Missing),
            Some(&recognition(&original, Recognition::OrdinarySource)),
        ) else {
            panic!("end plan")
        };
        assert_eq!(plan.original_source(), original.page());
        assert_eq!(plan.before_neighbor(), None);
        assert_eq!(plan.expected_order(), &[id(3)]);
        assert_eq!(plan.conversation(), id(8));
        assert_eq!(p.observe_page().unwrap(), original);
        assert_eq!(p.gesture_count(), 0);
    }

    #[test]
    fn successor_reuse_or_plan_preserves_original_and_has_zero_policy_effects() {
        let mut p = platform();
        let original = p.observe_page().unwrap();
        let plan = anchor(&original);
        assert_eq!(plan.original_source(), original.page());
        let receipt = navigate(
            &mut p,
            &original,
            LogicalDirection::Next,
            plan.neighbor(),
            10,
        );
        let fresh = p.observe_page().unwrap();
        let count = p.gesture_count();
        let relevant = Lookup::new(AssociationState::RelevantModelAssociation);
        assert!(
            matches!(complete_successor(&plan, &receipt, &fresh, &relevant, None), SyntheticDecision::ReuseCandidate(ref observed) if observed.observation() == &fresh && observed.conversation() == id(8))
        );
        assert_eq!(*relevant.calls.borrow(), vec![plan.neighbor()]);
        let missing = Lookup::new(AssociationState::Missing);
        let SyntheticDecision::InsertionPlan(placement) = complete_successor(
            &plan,
            &receipt,
            &fresh,
            &missing,
            Some(&recognition(&fresh, Recognition::ConfirmedNonBuddy)),
        ) else {
            panic!("original placement")
        };
        assert_eq!(placement.original_source(), original.page());
        assert_ne!(placement.original_source(), fresh.page());
        assert_eq!(placement.before_neighbor(), Some(fresh.page()));
        assert_eq!(placement.expected_order(), original.order());
        assert_eq!(p.observe_page().unwrap(), fresh);
        assert_eq!(p.gesture_count(), count);
        assert_eq!(p.pages(), &[id(3), id(4)]);
        // SDK API-limit fixture only: product policy never constructs/dispatches it.
        let stale = CreationRequest::new(
            OperationId(id(11)),
            original,
            TargetAllocation::ClientSelected(id(9)),
        );
        assert!(matches!(
            p.create_after(&stale, false),
            CreationOutcome::RejectedWithoutMutation { .. }
        ));
        assert_eq!(p.pages(), &[id(3), id(4)]);
    }

    #[test]
    fn successor_unknown_adoption_and_claims_never_become_insertion() {
        let mut p = platform();
        let original = p.observe_page().unwrap();
        let plan = anchor(&original);
        let receipt = navigate(
            &mut p,
            &original,
            LogicalDirection::Next,
            plan.neighbor(),
            10,
        );
        let fresh = p.observe_page().unwrap();
        let missing = Lookup::new(AssociationState::Missing);
        assert!(matches!(
            complete_successor(&plan, &receipt, &fresh, &missing, None),
            SyntheticDecision::RecognitionNeeded(_)
        ));
        for value in [Recognition::RelevantBuddy, Recognition::LegacyBuddy] {
            assert!(matches!(
                complete_successor(
                    &plan,
                    &receipt,
                    &fresh,
                    &missing,
                    Some(&recognition(&fresh, value))
                ),
                SyntheticDecision::AdoptionRequired(_)
            ));
        }
        for value in [
            Recognition::Blank,
            Recognition::MissingCache,
            Recognition::Unknown,
            Recognition::Failed,
            Recognition::OtherBuddy,
            Recognition::OrdinarySource,
        ] {
            assert!(matches!(
                complete_successor(
                    &plan,
                    &receipt,
                    &fresh,
                    &missing,
                    Some(&recognition(&fresh, value))
                ),
                SyntheticDecision::Unresolved(_)
            ));
        }
        for state in [
            AssociationState::Stale,
            AssociationState::Imported,
            AssociationState::Historical,
            AssociationState::Conflicted,
            AssociationState::Tombstoned,
            AssociationState::OtherConversation,
        ] {
            assert!(matches!(
                complete_successor(
                    &plan,
                    &receipt,
                    &fresh,
                    &Lookup::new(state),
                    Some(&recognition(&fresh, Recognition::ConfirmedNonBuddy))
                ),
                SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
            ));
        }
    }

    #[test]
    fn witness_scope_and_conversation_mismatch_never_fall_through() {
        let original = platform().observe_page().unwrap();
        let foreign = platform().observe_page().unwrap();
        let wrong = PageKey {
            document: id(2),
            page: id(4),
        };
        for (observed, page, conversation) in [
            (foreign, original.page(), id(8)),
            (original.clone(), wrong, id(8)),
            (original.clone(), original.page(), id(9)),
        ] {
            let lookup = Lookup::new(AssociationState::Missing);
            let witness = RecognitionWitness::modeled(
                observed.clone(),
                page,
                conversation,
                Recognition::OrdinarySource,
            );
            assert!(matches!(
                begin_current(&original, &original, id(8), &lookup, Some(&witness)),
                SyntheticDecision::Unresolved(UnresolvedReason::WitnessMismatch)
            ));
            assert!(lookup.calls.borrow().is_empty());
            let lookup = Lookup {
                supplied: Some(AssociationWitness::modeled(
                    observed,
                    page,
                    conversation,
                    AssociationState::RelevantModelAssociation,
                )),
                ..Lookup::new(AssociationState::Missing)
            };
            assert!(matches!(
                begin_current(&original, &original, id(8), &lookup, None),
                SyntheticDecision::Unresolved(UnresolvedReason::WitnessMismatch)
            ));
        }
    }

    #[test]
    fn opaque_input_session_visit_and_order_drift_refuse_before_lookup() {
        for fault in [
            NavigationFault::BeforeDispatchInput,
            NavigationFault::BeforeDispatchSession,
            NavigationFault::BeforeDispatchVisit,
            NavigationFault::BeforeDispatchOrder,
        ] {
            let mut p = platform();
            let original = p.observe_page().unwrap();
            let plan = anchor(&original);
            p.inject_navigation_fault(fault);
            let request = NavigationRequest::new(
                OperationId(id(10)),
                original.clone(),
                LogicalDirection::Next,
                plan.neighbor(),
            )
            .unwrap();
            let _ = p.navigate(&request, false);
            let fresh = p.observe_page().unwrap();
            let lookup = Lookup::new(AssociationState::RelevantModelAssociation);
            assert!(matches!(
                begin_current(&original, &fresh, id(8), &lookup, None),
                SyntheticDecision::Unresolved(UnresolvedReason::ObservationGuard)
            ));
            assert!(lookup.calls.borrow().is_empty());
        }
    }

    #[test]
    fn foreign_receipt_wrong_direction_and_stale_completion_refuse() {
        let mut p = platform();
        let original = p.observe_page().unwrap();
        let plan = anchor(&original);
        let next = navigate(
            &mut p,
            &original,
            LogicalDirection::Next,
            plan.neighbor(),
            10,
        );
        let fresh = p.observe_page().unwrap();
        let previous = navigate(
            &mut p,
            &fresh,
            LogicalDirection::Previous,
            original.page(),
            11,
        );
        let back = p.observe_page().unwrap();
        assert_eq!(back.page(), original.page());
        assert_eq!(back.order(), original.order());
        assert_ne!(back, original);
        let mut foreign = platform();
        let other = foreign.observe_page().unwrap();
        let foreign_receipt = navigate(
            &mut foreign,
            &other,
            LogicalDirection::Next,
            plan.neighbor(),
            12,
        );
        let lookup = Lookup::new(AssociationState::RelevantModelAssociation);
        for (receipt, current) in [
            (&next, &back),
            (&previous, &back),
            (&foreign_receipt, &fresh),
        ] {
            assert!(matches!(
                complete_successor(&plan, receipt, current, &lookup, None),
                SyntheticDecision::Unresolved(UnresolvedReason::NavigationGuard)
            ));
        }
        assert!(lookup.calls.borrow().is_empty());
        assert!(matches!(
            begin_current(&original, &back, id(8), &lookup, None),
            SyntheticDecision::Unresolved(UnresolvedReason::ObservationGuard)
        ));
    }

    fn placement_after_inspection(p: &mut MockPlatform) -> (PlacementPlan, PageObservation) {
        let original = p.observe_page().unwrap();
        let plan = anchor(&original);
        let next = navigate(p, &original, LogicalDirection::Next, plan.neighbor(), 20);
        let successor = p.observe_page().unwrap();
        let SyntheticDecision::InsertionPlan(placement) = complete_successor(
            &plan,
            &next,
            &successor,
            &Lookup::new(AssociationState::Missing),
            Some(&recognition(&successor, Recognition::ConfirmedNonBuddy)),
        ) else {
            panic!("historical placement")
        };
        (placement, successor)
    }

    #[test]
    fn supported_return_matches_changed_visit_without_policy_effects() {
        let mut p = platform();
        let (plan, successor) = placement_after_inspection(&mut p);
        let previous = navigate(
            &mut p,
            &successor,
            LogicalDirection::Previous,
            plan.original_source(),
            21,
        );
        let supplied = p.observe_page().unwrap();
        assert_ne!(supplied, plan.original);
        let gestures = p.gesture_count();
        let result = match_source_continuity(&plan, &supplied, Some(&previous)).unwrap();
        assert_eq!(result.matched_source(), &supplied);
        assert_eq!(result.conversation(), id(8));
        assert_eq!(
            result.historical_placement().original_source(),
            plan.original_source()
        );
        assert_eq!(
            result.historical_placement().before_neighbor(),
            Some(successor.page())
        );
        assert_eq!(
            result.historical_placement().expected_order(),
            plan.expected_order()
        );
        assert_eq!(p.observe_page().unwrap(), supplied);
        assert_eq!(p.gesture_count(), gestures);
        assert_eq!(p.pages(), &[id(3), id(4)]);
        assert!(match_source_continuity(&plan, &plan.original, Some(&previous)).is_err());
        assert!(match_source_continuity(&plan, &supplied, None).is_err());
    }

    #[test]
    fn end_requires_no_receipt_and_exact_unchanged_original() {
        let p = MockPlatform::new(id(1), id(2), vec![id(3)], id(3));
        let original = p.observe_page().unwrap();
        let SyntheticDecision::InsertionPlan(plan) = begin_current(
            &original,
            &original,
            id(8),
            &Lookup::new(AssociationState::Missing),
            Some(&recognition(&original, Recognition::OrdinarySource)),
        ) else {
            panic!("end plan")
        };
        let supplied = p.observe_page().unwrap();
        assert_eq!(
            match_source_continuity(&plan, &supplied, None)
                .unwrap()
                .matched_source(),
            &supplied
        );
        let mut other = platform();
        let start = other.observe_page().unwrap();
        let receipt = navigate(
            &mut other,
            &start,
            LogicalDirection::Next,
            anchor(&start).neighbor(),
            20,
        );
        assert!(match_source_continuity(&plan, &supplied, Some(&receipt)).is_err());
        assert!(match_source_continuity(&plan, &start, None).is_err());
        assert_eq!(p.gesture_count(), 0);
    }

    #[test]
    fn foreign_wrong_direction_target_and_replaced_plan_context_refuse_return() {
        let mut p = MockPlatform::new(id(1), id(2), vec![id(3), id(4), id(5)], id(3));
        let (plan, successor) = placement_after_inspection(&mut p);
        let wrong = navigate(
            &mut p,
            &successor,
            LogicalDirection::Next,
            PageKey {
                document: id(2),
                page: id(5),
            },
            21,
        );
        let supplied = p.observe_page().unwrap();
        assert!(match_source_continuity(&plan, &supplied, Some(&wrong)).is_err());
        let mut foreign = platform();
        let (other_plan, other_successor) = placement_after_inspection(&mut foreign);
        let previous = navigate(
            &mut foreign,
            &other_successor,
            LogicalDirection::Previous,
            other_plan.original_source(),
            21,
        );
        let fresh = foreign.observe_page().unwrap();
        assert!(match_source_continuity(&plan, &fresh, Some(&previous)).is_err());
        // Private invariant corruption controls only; public callers have no setters.
        let mut changed = other_plan.clone();
        changed.neighbor = Some(PageKey {
            document: id(2),
            page: id(5),
        });
        assert!(match_source_continuity(&changed, &fresh, Some(&previous)).is_err());
        changed = other_plan.clone();
        changed.continuity = PlacementContinuity::UninspectedEnd;
        assert!(match_source_continuity(&changed, &fresh, Some(&previous)).is_err());
    }

    #[test]
    fn no_movement_and_uncertain_return_do_not_match() {
        let mut p = platform();
        let (plan, successor) = placement_after_inspection(&mut p);
        p.inject_navigation_fault(NavigationFault::NoMovement);
        let request = NavigationRequest::new(
            OperationId(id(21)),
            successor,
            LogicalDirection::Previous,
            plan.original_source(),
        )
        .unwrap();
        let NavigationOutcome::SyntheticUnchanged(receipt) = p.navigate(&request, false) else {
            panic!("SDK no movement")
        };
        let supplied = p.observe_page().unwrap();
        assert!(match_source_continuity(&plan, &supplied, Some(&receipt)).is_err());
        for fault in [
            NavigationFault::Deadline,
            NavigationFault::UnreadyPixels,
            NavigationFault::BeforeDispatchInput,
            NavigationFault::BeforeDispatchSession,
            NavigationFault::BeforeDispatchVisit,
            NavigationFault::BeforeDispatchOrder,
        ] {
            let mut p = platform();
            let (plan, successor) = placement_after_inspection(&mut p);
            p.inject_navigation_fault(fault);
            let request = NavigationRequest::new(
                OperationId(id(21)),
                successor,
                LogicalDirection::Previous,
                plan.original_source(),
            )
            .unwrap();
            assert!(!matches!(
                p.navigate(&request, false),
                NavigationOutcome::SyntheticVerified(_)
            ));
            assert!(match_source_continuity(&plan, &p.observe_page().unwrap(), None).is_err());
        }
    }

    #[test]
    fn after_return_opaque_drift_refuses_even_when_page_ids_match() {
        for fault in [
            NavigationFault::BeforeDispatchInput,
            NavigationFault::BeforeDispatchSession,
            NavigationFault::BeforeDispatchVisit,
            NavigationFault::BeforeDispatchOrder,
        ] {
            let mut p = platform();
            let (plan, successor) = placement_after_inspection(&mut p);
            let previous = navigate(
                &mut p,
                &successor,
                LogicalDirection::Previous,
                plan.original_source(),
                21,
            );
            let returned = p.observe_page().unwrap();
            p.inject_navigation_fault(fault);
            let request = NavigationRequest::new(
                OperationId(id(22)),
                returned,
                LogicalDirection::Next,
                successor.page(),
            )
            .unwrap();
            let _ = p.navigate(&request, false);
            let changed = p.observe_page().unwrap();
            assert_eq!(changed.page(), plan.original_source());
            assert!(match_source_continuity(&plan, &changed, Some(&previous)).is_err());
        }
    }

    #[test]
    fn matching_sdk_continuity_cannot_refresh_changed_logical_projection() {
        let mut p = platform();
        let (plan, successor) = placement_after_inspection(&mut p);
        let previous = navigate(
            &mut p,
            &successor,
            LogicalDirection::Previous,
            plan.original_source(),
            21,
        );
        let supplied = p.observe_page().unwrap();
        let changed = Lookup::new(AssociationState::Conflicted);
        let gestures = p.gesture_count();
        let continuity = match_source_continuity(&plan, &supplied, Some(&previous)).unwrap();
        assert!(changed.calls.borrow().is_empty());
        assert!(matches!(
            begin_current(
                continuity.matched_source(),
                continuity.matched_source(),
                continuity.conversation(),
                &changed,
                Some(&recognition(
                    continuity.matched_source(),
                    Recognition::OrdinarySource
                )),
            ),
            SyntheticDecision::Unresolved(UnresolvedReason::LogicalClaim)
        ));
        assert_eq!(p.observe_page().unwrap(), supplied);
        assert_eq!(p.gesture_count(), gestures);
        assert_eq!(p.pages(), &[id(3), id(4)]);
    }

    #[test]
    fn legacy_bridge_current_missing_association_never_inspects_or_plans_insertion() {
        let p = platform();
        let original = p.observe_page().unwrap();
        for (legacy, expected) in [
            (
                super::super::AnswerPageType::ExistingQA,
                Recognition::LegacyBuddy,
            ),
            (super::super::AnswerPageType::Blank, Recognition::Blank),
            (super::super::AnswerPageType::Invalid, Recognition::Unknown),
        ] {
            let witness = model_legacy_answer_classification(&original, id(8), legacy).unwrap();
            assert_eq!(witness.recognition, expected);
            assert_eq!(witness.observation, original);
            assert_eq!(witness.page, original.page());
            assert_eq!(witness.conversation, id(8));
            let missing = Lookup::new(AssociationState::Missing);
            let decision = begin_current(&original, &original, id(8), &missing, Some(&witness));
            match legacy {
                super::super::AnswerPageType::ExistingQA => {
                    assert!(matches!(decision, SyntheticDecision::AdoptionRequired(_)))
                }
                _ => assert!(matches!(
                    decision,
                    SyntheticDecision::Unresolved(UnresolvedReason::RecognitionUncertain)
                )),
            }
            assert_eq!(*missing.calls.borrow(), vec![original.page()]);
            assert_eq!(p.observe_page().unwrap(), original);
            assert_eq!(p.gesture_count(), 0);
            assert_eq!(p.pages(), &[id(3), id(4)]);
            let known = Lookup::new(AssociationState::RelevantModelAssociation);
            assert!(
                matches!(begin_current(&original, &original, id(8), &known, Some(&witness)), SyntheticDecision::ReuseCandidate(ref candidate) if candidate.observation() == &original && candidate.conversation() == id(8))
            );
        }
        let wrong = model_legacy_answer_classification(
            &original,
            id(9),
            super::super::AnswerPageType::ExistingQA,
        )
        .unwrap();
        let missing = Lookup::new(AssociationState::Missing);
        assert!(matches!(
            begin_current(&original, &original, id(8), &missing, Some(&wrong)),
            SyntheticDecision::Unresolved(UnresolvedReason::WitnessMismatch)
        ));
        assert!(missing.calls.borrow().is_empty());
    }

    #[test]
    fn legacy_bridge_successor_requires_adoption_or_remains_unresolved() {
        let mut p = platform();
        let original = p.observe_page().unwrap();
        let plan = anchor(&original);
        let next = navigate(
            &mut p,
            &original,
            LogicalDirection::Next,
            plan.neighbor(),
            20,
        );
        let successor = p.observe_page().unwrap();
        let gestures = p.gesture_count();
        for legacy in [
            super::super::AnswerPageType::ExistingQA,
            super::super::AnswerPageType::Blank,
            super::super::AnswerPageType::Invalid,
        ] {
            let witness = model_legacy_answer_classification(&successor, id(8), legacy).unwrap();
            let missing = Lookup::new(AssociationState::Missing);
            let decision = complete_successor(&plan, &next, &successor, &missing, Some(&witness));
            match legacy {
                super::super::AnswerPageType::ExistingQA => {
                    assert!(matches!(decision, SyntheticDecision::AdoptionRequired(_)))
                }
                _ => assert!(matches!(
                    decision,
                    SyntheticDecision::Unresolved(UnresolvedReason::RecognitionUncertain)
                )),
            }
            assert_eq!(*missing.calls.borrow(), vec![successor.page()]);
            assert_eq!(p.observe_page().unwrap(), successor);
            assert_eq!(p.gesture_count(), gestures);
            assert_eq!(p.pages(), &[id(3), id(4)]);
        }
        let wrong = model_legacy_answer_classification(
            &original,
            id(8),
            super::super::AnswerPageType::ExistingQA,
        )
        .unwrap();
        let missing = Lookup::new(AssociationState::Missing);
        assert!(matches!(
            complete_successor(&plan, &next, &successor, &missing, Some(&wrong)),
            SyntheticDecision::Unresolved(UnresolvedReason::WitnessMismatch)
        ));
        assert!(missing.calls.borrow().is_empty());
    }
}
