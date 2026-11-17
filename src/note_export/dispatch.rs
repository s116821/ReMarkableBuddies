//! Private Writer dispatch: legacy conditional-version exports or Capacities.
//! Neither variant acquires a second owner or manufactures provider revisions.
use super::*;
use crate::{
    config::{self, product::ExportDestination, Config},
    conversation::{ExportAssociation, Ledger, Outcome},
    storage::Store,
};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::watch;

#[cfg(all(test, target_os = "linux"))]
#[path = "configured_tests.rs"]
mod configured_tests;

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

/// Only existing command composition instantiates credentials from saved policy.
pub(crate) enum ConfiguredDestination {
    Mem(mem::MemDestination),
    Capacities {
        destination: capacities::CapacitiesDestination,
        _lease: Box<config::capacities::AccessTokenLease>,
    },
}

impl ConfiguredDestination {
    fn mem() -> Result<Self> {
        let token = std::env::var("MEM_API_KEY").map_err(|_| Failure::MissingCredentials)?;
        mem::MemDestination::new(&token).map(Self::Mem)
    }

    fn capacities(
        config: &Config,
        space: Uuid,
        grant: Uuid,
        tags: BTreeSet<Uuid>,
        may_write: bool,
    ) -> Result<Self> {
        let lease =
            config::capacities::access_token(&config.paths.credentials, space, grant, may_write)?;
        let destination =
            capacities::CapacitiesDestination::enrolled(lease.expose(), space, grant, tags)?;
        Ok(Self::Capacities {
            destination,
            _lease: Box::new(lease),
        })
    }

    pub(crate) fn selected(config: &Config) -> Result<Self> {
        match config.export_destination() {
            ExportDestination::Mem => Self::mem(),
            ExportDestination::Disabled => Err(Failure::Unsupported),
            ExportDestination::Capacities => {
                let connection = config
                    .capacities_export
                    .as_ref()
                    .ok_or(Failure::MissingCapacitiesCredentials)?;
                Self::capacities(
                    config,
                    connection.space_id,
                    connection.grant_id,
                    connection.tags.iter().copied().collect(),
                    true,
                )
            }
        }
    }

    pub(crate) fn original(
        config: &Config,
        conversation: Uuid,
        correlation: Uuid,
    ) -> Result<(Ledger, Self)> {
        let store = Store::open_existing_for_export_reconciliation(config.paths.clone())
            .map_err(|_| Failure::Storage)?;
        let ledger = Ledger::new(Arc::new(store));
        let exports = ledger
            .original_exports(conversation, correlation)
            .map_err(|_| Failure::Storage)?;
        let destination = match exports.as_slice() {
            [] => return Err(Failure::Storage),
            [association] => match (association.backend.as_str(), &association.capacities) {
                ("mem", None) => Self::mem()?,
                ("capacities", Some(attempt)) => {
                    let grant = attempt
                        .grant_id
                        .ok_or(Failure::MissingCapacitiesCredentials)?;
                    Self::capacities(config, attempt.space, grant, attempt.tags.clone(), false)?
                }
                _ => return Err(Failure::Unsupported),
            },
            _ => return Err(Failure::Conflict),
        };
        Ok((ledger, destination))
    }
}

impl OwnedDestination for ConfiguredDestination {
    fn backend(&self) -> &str {
        match self {
            Self::Mem(destination) => destination.backend(),
            Self::Capacities { destination, .. } => destination.backend(),
        }
    }

    fn association_id(&self, conversation: Uuid, correlation: Uuid) -> Uuid {
        match self {
            Self::Mem(_) => correlation,
            Self::Capacities { destination, .. } => {
                destination.association_id(conversation, correlation)
            }
        }
    }

    fn intent_binding(&self) -> String {
        match self {
            Self::Mem(destination) => destination.backend().to_owned(),
            Self::Capacities { destination, .. } => destination.intent_binding(),
        }
    }

    async fn export_owned(
        &self,
        ledger: &Ledger,
        note: &PreparedNote,
        canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        control: Option<&mut ExportControl<'_>>,
    ) -> ExportReport {
        match self {
            Self::Mem(destination) => {
                LegacyDestination(destination)
                    .export_owned(ledger, note, canceled, owner, control)
                    .await
            }
            Self::Capacities { destination, .. } => {
                destination
                    .export_owned(ledger, note, canceled, owner, control)
                    .await
            }
        }
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
        match self {
            Self::Mem(destination) => {
                LegacyDestination(destination)
                    .reconcile_owned(
                        ledger,
                        conversation,
                        correlation,
                        canceled,
                        owner,
                        observation,
                    )
                    .await
            }
            Self::Capacities { destination, .. } => {
                destination
                    .reconcile_owned(
                        ledger,
                        conversation,
                        correlation,
                        canceled,
                        owner,
                        observation,
                    )
                    .await
            }
        }
    }
}
