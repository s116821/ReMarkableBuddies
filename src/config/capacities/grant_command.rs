//! Original-grant maintenance under the same bounded tablet actor and custody.
use super::{
    super::lifecycle::{Revocation, Rotation},
    super::oauth::GrantOperation,
    config_matches, durable_config_matches, Binding, Budget, Custody, Document, Failure,
    GrantOwner, Instant, OAuth, PathBuf, Phase, Registration, Status, Store, Uuid, MAX_BODY,
    MAX_MILLIS,
};
use crate::{config::inspection::strict_json, storage::valid_digest};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Selector {
    GrantStatus,
    Refresh,
    Disconnect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    contract_version: u32,
    selector: Selector,
    budget_ms: u64,
    binding: Binding,
    space_id: Uuid,
    expected_grant_sha256: Option<String>,
}

impl Request {
    pub(super) fn parse(frame: &[u8]) -> Option<Self> {
        if frame.len() > MAX_BODY {
            return None;
        }
        let raw = strict_json(frame).ok()?;
        let binding = raw["binding"].as_object()?;
        for field in ["boot_id", "operation_id", "grant_id"] {
            let uid = binding.get(field)?.as_str()?;
            if uid != Uuid::parse_str(uid).ok()?.to_string() {
                return None;
            }
        }
        let space = raw["space_id"].as_str()?;
        if space != Uuid::parse_str(space).ok()?.to_string() {
            return None;
        }
        let has_revision = raw.get("expected_grant_sha256").is_some();
        let request: Self = serde_json::from_value(raw).ok()?;
        (request.contract_version == 1
            && (1..=MAX_MILLIS).contains(&request.budget_ms)
            && request.binding.valid()
            && !request.space_id.is_nil()
            && if request.selector == Selector::GrantStatus {
                !has_revision
            } else {
                request
                    .expected_grant_sha256
                    .as_deref()
                    .is_some_and(valid_digest)
            })
        .then_some(request)
    }

    pub(super) fn is_disconnect(&self) -> bool {
        self.selector == Selector::Disconnect
    }

    fn approved(&self, status: &Status, registration: &Registration) -> bool {
        status.enrollment.as_ref().is_some_and(|original| {
            original.tablet_id == self.binding.tablet_id
                && registration.client_id == original.client_id
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Observed,
    Refused,
    Conflict,
    Refreshed,
    LocalDisabled,
    Revoked,
    Unresolved,
}

#[derive(Serialize)]
pub(super) struct Report {
    contract_version: u32,
    #[serde(flatten)]
    binding: Binding,
    space_id: Uuid,
    status: Outcome,
    phase: Option<&'static str>,
    grant_sha256: Option<String>,
    observed_operation_id: Option<Uuid>,
    #[serde(skip)]
    _store: Option<Custody>,
    #[serde(skip)]
    _owner: Option<GrantOwner>,
    #[serde(skip)]
    _rotation: Option<Rotation>,
    #[serde(skip)]
    _revocation: Option<Revocation>,
}

impl Report {
    fn new(request: &Request) -> Self {
        Self {
            contract_version: 1,
            binding: request.binding.clone(),
            space_id: request.space_id,
            status: Outcome::Unresolved,
            phase: None,
            grant_sha256: None,
            observed_operation_id: None,
            _store: None,
            _owner: None,
            _rotation: None,
            _revocation: None,
        }
    }

    fn observe(&mut self, status: Status, outcome: Outcome) {
        self.status = outcome;
        self.phase = Some(match status.phase {
            Phase::Absent => "absent",
            Phase::PendingExchange => "pending-exchange",
            Phase::Staged => "staged",
            Phase::Active => "active",
            Phase::PendingRotation => "pending-rotation",
            Phase::LocalDisabled => "local-disabled",
            Phase::RevocationPending => "revocation-pending",
            Phase::Revoked => "revoked",
            Phase::RetiredUnconfirmed => "retired-unconfirmed",
        });
        self.grant_sha256 = status.revision;
        self.observed_operation_id = status.operation_id;
    }
}

pub(super) async fn handle(
    path: Option<PathBuf>,
    request: Request,
    started: Instant,
    canceled: watch::Receiver<bool>,
    registration: Option<&Registration>,
    boot: Option<Uuid>,
) -> Report {
    let mut report = Report::new(&request);
    let binding = &request.binding;
    let budget = Budget::new(started + Duration::from_millis(request.budget_ms), canceled);
    if boot != Some(binding.boot_id) || budget.current().is_err() {
        return report;
    }
    let Some(doc) = Document::load(path) else {
        return report;
    };
    if doc.sha256 != binding.config_sha256 {
        report.status = Outcome::Conflict;
        return report;
    }
    let store = if request.selector == Selector::GrantStatus {
        Store::open_existing_for_capacities_capture(doc.config.paths.clone())
    } else {
        Store::open_existing_for_export_reconciliation(doc.config.paths.clone())
    };
    let Ok(store) = store else {
        return report;
    };
    if store.actor_id.to_string() != binding.tablet_id {
        report.status = Outcome::Conflict;
        return report;
    }
    let Ok(custody) = Custody::acquire(Arc::new(store)) else {
        return report;
    };
    report._store = Some(custody);
    let Ok(Some(owner)) =
        GrantOwner::open_existing(&doc.config.paths.credentials, binding.grant_id)
    else {
        return report;
    };
    let Ok(current) = owner.status() else {
        report._owner = Some(owner);
        return report;
    };
    if current.space_id != Some(request.space_id)
        || current
            .enrollment
            .as_ref()
            .is_some_and(|original| original.tablet_id != binding.tablet_id)
        || !durable_config_matches(&doc.path, &binding.config_sha256)
        || budget.current().is_err()
    {
        report._owner = Some(owner);
        return report;
    }
    if request.selector == Selector::GrantStatus {
        report.observe(current, Outcome::Observed);
        report._owner = Some(owner);
        return report;
    }
    let selected = doc
        .config
        .capacities_export
        .as_ref()
        .is_some_and(|selection| {
            selection.grant_id == binding.grant_id && selection.space_id == request.space_id
        });
    if !selected || request.expected_grant_sha256 != current.revision {
        report.status = Outcome::Conflict;
        report._owner = Some(owner);
        return report;
    }
    let approval = registration.filter(|approval| request.approved(&current, approval));
    match request.selector {
        Selector::Refresh => {
            refresh(report, &request, &doc, &budget, owner, current, approval).await
        }
        Selector::Disconnect => {
            disconnect(report, &request, &doc, &budget, owner, current, approval).await
        }
        Selector::GrantStatus => report,
    }
}

fn current_config(doc: &Document, budget: &Budget) -> Result<(), Failure> {
    if !config_matches(&doc.path, &doc.sha256) {
        return Err(Failure::Conflict);
    }
    budget.current().map(|_| ())
}

pub(super) fn disabled_current(
    owner: &GrantOwner,
    doc: &Document,
    budget: &Budget,
) -> Option<Status> {
    let status = owner.status().ok()?;
    (status.phase == Phase::LocalDisabled && current_config(doc, budget).is_ok()).then_some(status)
}

async fn refresh(
    mut report: Report,
    request: &Request,
    doc: &Document,
    budget: &Budget,
    owner: GrantOwner,
    snapshot: Status,
    approval: Option<&Registration>,
) -> Report {
    let Some(approval) = approval.filter(|_| snapshot.phase == Phase::Active) else {
        report.observe(snapshot, Outcome::Refused);
        report._owner = Some(owner);
        return report;
    };
    if snapshot.operation_id == Some(request.binding.operation_id) {
        report.observe(snapshot, Outcome::Conflict);
        report._owner = Some(owner);
        return report;
    }
    let Ok(oauth) = OAuth::new(approval) else {
        report._owner = Some(owner);
        return report;
    };
    if oauth
        .verify_metadata_for(budget, Some(GrantOperation::Refresh))
        .await
        .is_err()
        || current_config(doc, budget).is_err()
    {
        report.observe(snapshot, Outcome::Refused);
        report._owner = Some(owner);
        return report;
    }
    let mut permit = match owner.begin_rotation_retained(
        request.space_id,
        request.binding.operation_id,
        snapshot.revision.as_deref(),
    ) {
        Ok(permit) => permit,
        Err((owner, _)) => {
            report._owner = Some(*owner);
            return report;
        }
    };
    if let Ok(grant) = oauth
        .refresh(
            &mut permit,
            request.binding.grant_id,
            request.space_id,
            budget,
            || current_config(doc, budget),
        )
        .await
    {
        if let Ok(bytes) = serde_json::to_vec(&grant) {
            if current_config(doc, budget).is_ok() && permit.install_in_place(&bytes).is_ok() {
                if let Ok(snapshot) = permit.owner().status() {
                    if snapshot.phase == Phase::Active
                        && snapshot.operation_id == Some(request.binding.operation_id)
                        && current_config(doc, budget).is_ok()
                    {
                        report.observe(snapshot, Outcome::Refreshed);
                    }
                }
            }
        }
    }
    report._rotation = Some(permit);
    report
}

async fn disconnect(
    mut report: Report,
    request: &Request,
    doc: &Document,
    budget: &Budget,
    owner: GrantOwner,
    snapshot: Status,
    approval: Option<&Registration>,
) -> Report {
    if !matches!(
        snapshot.phase,
        Phase::Active | Phase::PendingRotation | Phase::LocalDisabled
    ) {
        report.observe(snapshot, Outcome::Refused);
        report._owner = Some(owner);
        return report;
    }
    if current_config(doc, budget).is_err()
        || (snapshot.phase != Phase::LocalDisabled
            && owner
                .disable_local_current(request.space_id, snapshot.revision.as_deref())
                .is_err())
    {
        report._owner = Some(owner);
        return report;
    }
    let Some(disabled) = disabled_current(&owner, doc, budget) else {
        report._owner = Some(owner);
        return report;
    };
    // Publish local disablement independently. An unknown rotating token has
    // no revocable material, and a blocked preflight leaves local use disabled.
    let revision = disabled.revision.clone();
    report.observe(disabled, Outcome::LocalDisabled);
    let Some(approval) = approval else {
        report._owner = Some(owner);
        return report;
    };
    let Ok(oauth) = OAuth::new(approval) else {
        report._owner = Some(owner);
        return report;
    };
    if !owner.can_revoke()
        || oauth
            .verify_metadata_for(budget, Some(GrantOperation::Revoke))
            .await
            .is_err()
        || current_config(doc, budget).is_err()
    {
        report._owner = Some(owner);
        return report;
    }
    let mut permit = match owner.begin_revocation_retained(
        request.space_id,
        request.binding.operation_id,
        revision.as_deref(),
    ) {
        Ok(permit) => permit,
        Err((owner, _)) => {
            report.status = Outcome::Unresolved;
            report.grant_sha256 = None;
            report._owner = Some(*owner);
            return report;
        }
    };
    if oauth
        .revoke(&mut permit, budget, || current_config(doc, budget))
        .await
        .is_ok()
        && current_config(doc, budget).is_ok()
        && permit.confirm_in_place().is_ok()
    {
        if let Ok(snapshot) = permit.owner().status() {
            if snapshot.phase == Phase::Revoked
                && snapshot.operation_id == Some(request.binding.operation_id)
                && current_config(doc, budget).is_ok()
            {
                report.observe(snapshot, Outcome::Revoked);
            }
        }
    }
    if report.status != Outcome::Revoked {
        // Only describe durable newly observed progress. The original
        // local-disable confirmation survives an unknown provider outcome.
        report.status = Outcome::Unresolved;
        report.grant_sha256 = None;
        if let Ok(snapshot) = permit.owner().status() {
            if snapshot.phase == Phase::RevocationPending {
                report.observe(snapshot, Outcome::Unresolved);
            }
        }
    }
    report._revocation = Some(permit);
    report
}
