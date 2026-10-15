//! Read-only Reader/SDK association inspection. No native reuse or effect permit.
use super::Workflow;
use crate::conversation::{
    SelectedBindingCandidate, SelectedBindingInspection, SelectedBindingUnresolved,
};
use crate::storage::selection::SelectionToken;
use crate::storage::{Uuid, MAX_ITEMS};
use anyhow::{ensure, Context, Result};
use remarkable_open_sdk::{EvidenceOrigin, PageObservation};
use std::collections::HashSet;

/// An exact supplied SDK observation paired with historical selected evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservedBuddyBinding {
    observation: PageObservation,
    selected: SelectedBindingCandidate,
}
impl ObservedBuddyBinding {
    pub fn observation(&self) -> &PageObservation {
        &self.observation
    }
    pub fn selected(&self) -> &SelectedBindingCandidate {
        &self.selected
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BuddyBindingInspection {
    HistoricalCandidate(Box<ObservedBuddyBinding>),
    Unresolved(SelectedBindingUnresolved),
}

impl Workflow {
    /// Inspect only the current observed page, using this Workflow's selected
    /// facade. Supplied matching observations are not a native freshness oracle.
    /// No capture, successor lookup, navigation, provider or publication occurs.
    pub fn inspect_current_buddy_binding(
        &self,
        accepted: &SelectionToken,
        conversation: Uuid,
        original: &PageObservation,
        fresh: &PageObservation,
    ) -> Result<BuddyBindingInspection> {
        ensure!(
            !self.input_failed,
            "Request input lost; binding inspection refused"
        );
        ensure!(
            original.origin() == EvidenceOrigin::Synthetic
                && fresh.origin() == EvidenceOrigin::Synthetic,
            "Unsupported SDK observation origin"
        );
        ensure!(
            original.order().len() <= MAX_ITEMS && fresh.order().len() <= MAX_ITEMS,
            "SDK binding observation exceeds domain order bound"
        );
        ensure!(original == fresh, "SDK binding observation changed");
        let order: HashSet<_> = original.order().iter().collect();
        ensure!(
            !order.is_empty()
                && order.len() == original.order().len()
                && order.contains(&original.page().page),
            "SDK binding observation order invalid"
        );
        let facade = self
            .selected
            .as_ref()
            .context("Unbound Workflow cannot inspect selected binding")?;
        let page = original.page();
        let selected = facade.inspect_buddy_binding(
            accepted,
            conversation,
            Uuid::from_bytes(*page.document.as_bytes()),
            Uuid::from_bytes(*page.page.as_bytes()),
        )?;
        Ok(match selected {
            SelectedBindingInspection::HistoricalCandidate(selected) => {
                BuddyBindingInspection::HistoricalCandidate(Box::new(ObservedBuddyBinding {
                    observation: original.clone(),
                    selected: *selected,
                }))
            }
            SelectedBindingInspection::Unresolved(reason) => {
                BuddyBindingInspection::Unresolved(reason)
            }
        })
    }
}

#[cfg(test)]
mod tests;
