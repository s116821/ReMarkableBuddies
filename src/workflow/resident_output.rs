//! Resident target acquisition, distinct from historical source pixels.
//! Only an explicitly qualified backend can supply recognition/adoption and a
//! live output lease. Ordinary DeviceBackend implementations expose no backend.
pub use super::resident_source::DevelopmentTargetLease;
use crate::{
    conversation::{HistoricalPageAssociation, Ledger},
    device::request_input::RequestInput,
    storage::Uuid,
};
use anyhow::{ensure, Result};
use remarkable_open_sdk::{
    native_adapter::{HistoricalCapture, ReaderFacts, RuntimeIdentity},
    navigation::LogicalDirection,
};
use std::time::Instant;
#[cfg(all(
    feature = "development-resident-bootstrap",
    feature = "development-input-diagnostics"
))]
pub mod development;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageUse {
    /// Qualified current-device association, reconciled through existing Ledger.
    RelevantBuddy,
    /// Recognition and existing association inspection positively exclude Buddy.
    ConfirmedNonBuddy,
    /// Legacy Buddy requires explicit qualified adoption in this same backend.
    LegacyBuddy,
    /// Includes blank, missing cache/binding, stale/conflicting claims and failure.
    Unresolved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentOutputOutcome {
    Unavailable,
    Unresolved,
    ReusedWithoutWrite,
    /// Rendering was submitted under the backend lease; persistence not implied.
    SubmittedUnverified,
}

pub enum SelectedConversationRequest {
    BlankStart(Option<Uuid>),
    /// Ordinary installed receivers cannot consume fresh regional generation.
    UnsupportedRegionalWriter,
    WriterRefinement {
        request: crate::writer::Request,
        require_current_draft: bool,
    },
}

/// Trusted native adapter seam, analogous to DeviceBackend, not an authority
/// minted by ReaderFacts. No production implementation is supplied here.
/// Implementations must use existing Ledger bindings/adoption, not a second map.
pub trait ResidentOutputBackend {
    fn selected_conversation_request(&self) -> Result<Option<SelectedConversationRequest>> {
        Ok(None)
    }

    fn validate_new_page_text(&self, _text: &str) -> Result<()> {
        Ok(())
    }

    /// Optional diagnostics from fresh native pixels after rendering. This is
    /// neither a target permission nor a persistence receipt.
    fn record_rendered(
        &mut self,
        _runtime: &RuntimeIdentity,
        _facts: &ReaderFacts,
        _pixels: &HistoricalCapture,
    ) -> Result<()> {
        Ok(())
    }
    /// Pin exact runtime/profile/backend qualification BEFORE any navigation or
    /// insertion. Reader-source approval and historical captures are insufficient.
    fn admit(
        &mut self,
        runtime: &RuntimeIdentity,
        association: Option<&HistoricalPageAssociation>,
        deadline: Instant,
    ) -> Result<()>;
    fn owned_keyboard(&mut self) -> Result<Option<std::path::PathBuf>> {
        Ok(None)
    }
    /// Read-only, complete recognition; missing binding/cache/blank is unresolved.
    #[allow(clippy::too_many_arguments)]
    fn classify(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        runtime: &RuntimeIdentity,
        facts: &ReaderFacts,
        pixels: &HistoricalCapture,
        blank_start_source: bool,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<PageUse>;
    /// Explicit adoption uses existing REM37 binding/CAS contracts. Failure or
    /// ambiguity remains unresolved; never silently treats a legacy header as bound.
    fn adopt(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        runtime: &RuntimeIdentity,
        facts: &ReaderFacts,
        deadline: Instant,
    ) -> Result<bool>;
    /// Exactly one supported relative gesture. Consume the old input guard and
    /// return a fresh guard only after release; pulse checks remain active during
    /// the explicitly owned physical window. No direct page-open/Select fallback.
    fn navigate(
        &mut self,
        input: RequestInput,
        direction: LogicalDirection,
        runtime: &RuntimeIdentity,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<RequestInput>;
    /// For a new target, perform existing Ledger association publication/CAS
    /// here and qualify that claim; never create a parallel target registry.
    /// Returns a non-cloneable live renderer lease after fresh current target /
    /// ownership/input/content qualification. Facts alone cannot satisfy this.
    #[allow(clippy::too_many_arguments)]
    fn renderer(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        runtime: &RuntimeIdentity,
        target: &ReaderFacts,
        newly_created: bool,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Box<dyn ResidentRenderer + '_>>;
}
/// Adapter owns guard enforcement at each lower-level effect. It must route the
/// supplied text through the existing output renderer, not add a new layout or
/// history system. No native lease constructor is exposed by this coordinator.
pub trait ResidentRenderer {
    fn render(&mut self, text: &str, lease: &mut DevelopmentTargetLease<'_>) -> Result<()>;
}
pub(super) fn exact(actual: &ReaderFacts, expected: &ReaderFacts) -> Result<()> {
    ensure!(actual == expected, "resident output source/order changed");
    Ok(())
}
