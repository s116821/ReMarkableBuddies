//! Private Writer dispatch: legacy conditional-version exports or Capacities.
//! Neither variant acquires a second owner or manufactures provider revisions.
use super::*;
use crate::conversation::{ExportAssociation, Ledger, Outcome};
use tokio::sync::watch;

pub(crate) fn unresolved(association: &ExportAssociation) -> bool {
    association.outcome != Outcome::Completed
        && !(matches!(association.outcome, Outcome::Failed | Outcome::Canceled)
            && (association.capacities.is_some()
                || association.attempt.as_ref().is_some_and(|a| a.no_send)))
}

pub(crate) trait OwnedDestination: Sync {
    fn backend(&self) -> &str;
    fn association_id(&self, conversation: Uuid, correlation: Uuid) -> Uuid;
    /// Legacy bytes stay unchanged; Capacities binds space and organization too.
    fn intent_binding(&self) -> String;
    async fn export_owned(
        &self,
        ledger: &Ledger,
        note: &PreparedNote,
        canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        control: Option<&mut ExportControl<'_>>,
    ) -> ExportReport;
    async fn reconcile_owned(
        &self,
        ledger: &Ledger,
        conversation: Uuid,
        correlation: Uuid,
        canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        observation: Option<&mut ExportObservation>,
    ) -> ExportReport;
}

pub(crate) struct LegacyDestination<'a, D>(pub &'a D);
impl<D: NoteDestination> OwnedDestination for LegacyDestination<'_, D> {
    fn backend(&self) -> &str {
        self.0.backend()
    }
    fn association_id(&self, _: Uuid, correlation: Uuid) -> Uuid {
        correlation
    }
    fn intent_binding(&self) -> String {
        self.0.backend().to_owned()
    }
    async fn export_owned(
        &self,
        ledger: &Ledger,
        note: &PreparedNote,
        canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        control: Option<&mut ExportControl<'_>>,
    ) -> ExportReport {
        export_with_owner(ledger, self.0, note, canceled, owner, control).await
    }
    async fn reconcile_owned(
        &self,
        ledger: &Ledger,
        conversation: Uuid,
        correlation: Uuid,
        canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        observation: Option<&mut ExportObservation>,
    ) -> ExportReport {
        reconcile_with_owner(
            ledger,
            self.0,
            conversation,
            correlation,
            canceled,
            owner,
            observation,
        )
        .await
    }
}
