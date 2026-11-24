//! Private Writer dispatch: legacy conditional-version exports or Capacities.
//! Neither variant acquires a second owner or manufactures provider revisions.
use super::*;
use crate::{
    config::{capacities::configured::Access, product::ExportDestination, Config, SavedRevision},
    conversation::{CapacitiesAttempt, ExportAssociation, Ledger, Outcome, CAPACITIES_BACKEND},
    storage::Store,
};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::{watch, Mutex};

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
    Capacities(Box<ConfiguredCapacities>),
}

pub(crate) struct ConfiguredCapacities {
    space: Uuid,
    grant: Uuid,
    tags: BTreeSet<Uuid>,
    access: Mutex<Access>,
}

impl ConfiguredDestination {
    fn mem() -> Result<Self> {
        let token = std::env::var("MEM_API_KEY").map_err(|_| Failure::MissingCredentials)?;
        mem::MemDestination::new(&token).map(Self::Mem)
    }

    fn capacities(
        config: &Config,
        saved: Option<&SavedRevision>,
        space: Uuid,
        grant: Uuid,
        tags: BTreeSet<Uuid>,
        may_write: bool,
        existing: Option<&Store>,
    ) -> Result<Self> {
        let access = Access::open(&config.paths, saved.cloned(), space, grant, may_write)?;
        if may_write {
            access.admit(existing.map(|store| store.actor_id), true)?;
        }
        Ok(Self::Capacities(Box::new(ConfiguredCapacities {
            space,
            grant,
            tags,
            access: Mutex::new(access),
        })))
    }

    pub(crate) fn selected(
        config: &Config,
        saved: Option<&SavedRevision>,
        existing: Option<&Store>,
    ) -> Result<Self> {
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
                    saved,
                    connection.space_id,
                    connection.grant_id,
                    connection.tags.iter().copied().collect(),
                    true,
                    existing,
                )
            }
        }
    }

    pub(crate) fn admit_actor(&mut self, store: &Store) -> Result<()> {
        match self {
            Self::Mem(_) => Ok(()),
            Self::Capacities(binding) => binding.access.get_mut().admit_actor(store.actor_id),
        }
    }

    pub(crate) fn original(
        config: &Config,
        saved: Option<&SavedRevision>,
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
                    let mut destination = Self::capacities(
                        config,
                        saved,
                        attempt.space,
                        grant,
                        attempt.tags.clone(),
                        false,
                        Some(ledger.store()),
                    )?;
                    if capacities::local_reconciliation(association).is_some() {
                        let Self::Capacities(binding) = &mut destination else {
                            unreachable!()
                        };
                        binding
                            .access
                            .get_mut()
                            .admit(Some(ledger.store().actor_id), false)?;
                    }
                    destination
                }
                _ => return Err(Failure::Unsupported),
            },
            _ => return Err(Failure::Conflict),
        };
        Ok((ledger, destination))
    }
}

fn credential_report(correlation: Uuid, effect: Uuid, failure: Failure) -> ExportReport {
    let status = if failure == Failure::Conflict {
        ExportStatus::Conflict
    } else {
        ExportStatus::Refused
    };
    let mut report = ExportReport::new(status, correlation, effect);
    report.failure = Some(failure);
    report
}

impl OwnedDestination for ConfiguredDestination {
    fn backend(&self) -> &str {
        match self {
            Self::Mem(destination) => destination.backend(),
            Self::Capacities(_) => CAPACITIES_BACKEND,
        }
    }

    fn association_id(&self, conversation: Uuid, correlation: Uuid) -> Uuid {
        match self {
            Self::Mem(_) => correlation,
            Self::Capacities(destination) => {
                CapacitiesAttempt::association_id(conversation, destination.space, correlation)
            }
        }
    }

    fn intent_binding(&self) -> String {
        match self {
            Self::Mem(destination) => destination.backend().to_owned(),
            Self::Capacities(destination) => capacities::intent_binding(
                destination.space,
                &destination.tags,
                Some(destination.grant),
            ),
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
            Self::Capacities(binding) => {
                let Ok(mut access) = binding.access.try_lock() else {
                    return ExportReport::new(
                        ExportStatus::Busy,
                        note.correlation,
                        note.effect_operation,
                    );
                };
                let destination = match capacities::CapacitiesDestination::configured(
                    binding.space,
                    binding.grant,
                    binding.tags.clone(),
                ) {
                    Ok(destination) => destination,
                    Err(failure) => {
                        return credential_report(note.correlation, note.effect_operation, failure)
                    }
                };
                destination
                    .owned_export(ledger, note, canceled, owner, control, Some(&mut access))
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
            Self::Capacities(binding) => {
                if !owner.belongs_to(ledger) {
                    return credential_report(correlation, Uuid::nil(), Failure::Storage);
                }
                let id =
                    CapacitiesAttempt::association_id(conversation, binding.space, correlation);
                let Ok(Some(association)) = ledger.export_association(conversation, id) else {
                    return credential_report(correlation, Uuid::nil(), Failure::Storage);
                };
                if !association.capacities.as_ref().is_some_and(|attempt| {
                    attempt.space == binding.space && attempt.grant_id == Some(binding.grant)
                }) {
                    return credential_report(correlation, Uuid::nil(), Failure::Conflict);
                }
                if let Some(local) = capacities::local_reconciliation(&association) {
                    return local;
                }
                let refuse = |failure| {
                    let mut report =
                        super::run::report(&association, ExportStatus::ReconcileRequired);
                    report.failure = Some(failure);
                    report
                };
                let Ok(mut access) = binding.access.try_lock() else {
                    return ExportReport::new(ExportStatus::Busy, correlation, Uuid::nil());
                };
                let deadline = Budget::default().end;
                let token = match access
                    .current(ledger.store().actor_id, deadline, canceled.clone(), || {
                        Ok(())
                    })
                    .await
                {
                    Ok(token) => token,
                    Err(failure) => return refuse(failure),
                };
                let destination = match capacities::CapacitiesDestination::enrolled(
                    token.expose(),
                    binding.space,
                    binding.grant,
                    binding.tags.clone(),
                    deadline,
                ) {
                    Ok(destination) => destination,
                    Err(failure) => return refuse(failure),
                };
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
