//! Capacities REST create, managed-group updates and exact-ID readback.
mod content;
use super::{
    run::{dispatch, event, pending, report, selection_current, store_state, Owner},
    *,
};
use crate::{
    config::capacities::configured::Access,
    conversation::{
        CapacitiesAttempt, CapacitiesBaseline, CapacitiesManaged, ExportAction, Ledger, Outcome,
        CAPACITIES_BACKEND, CAPACITIES_PAGE,
    },
    storage::{digest, Namespace},
};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, Method, StatusCode, Url,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tokio::sync::watch;

const API_ORIGIN: &str = "https://api.capacities.io/";
pub(crate) const REST_VERSION_HEADER: &str = "x-capacities-api-version";
pub(crate) const REST_VERSION: &str = "1.0.0";
const MAX_RESPONSE: usize = 1024 * 1024;

pub struct CapacitiesDestination {
    client: Option<Client>,
    origin: Url,
    space: Uuid,
    tags: BTreeSet<Uuid>,
    grant_id: Option<Uuid>,
    deadline: Option<std::time::Instant>,
}

#[derive(Deserialize)]
struct Space {
    id: Uuid,
}

#[derive(Deserialize)]
struct Structures {
    structures: Vec<Structure>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Structure {
    id: String,
    property_definitions: Vec<Property>,
}

#[derive(Deserialize)]
struct Property {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    writable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectIdentity {
    id: Uuid,
    structure_id: String,
}

#[derive(Deserialize)]
struct Object {
    #[serde(flatten)]
    identity: ObjectIdentity,
    properties: BTreeMap<String, Value>,
    #[serde(default)]
    blocks: BTreeMap<String, Vec<Value>>,
}

struct VerifiedBody {
    id: String,
    identity_sha256: Option<String>,
}

impl CapacitiesDestination {
    /// The caller supplies an authorized REST access token for its selected space.
    pub fn new(token: &str, space: Uuid, tags: BTreeSet<Uuid>) -> Result<Self> {
        Self::build(API_ORIGIN, token, space, tags, false)
    }

    pub(super) fn configured(space: Uuid, grant_id: Uuid, tags: BTreeSet<Uuid>) -> Result<Self> {
        if grant_id.is_nil() {
            return Err(Failure::MissingCapacitiesCredentials);
        }
        Ok(Self {
            client: None,
            origin: Url::parse(API_ORIGIN).map_err(|_| Failure::InvalidInput)?,
            space,
            tags,
            grant_id: Some(grant_id),
            deadline: None,
        })
    }

    pub(super) fn enrolled(
        token: &str,
        space: Uuid,
        grant_id: Uuid,
        tags: BTreeSet<Uuid>,
        deadline: std::time::Instant,
    ) -> Result<Self> {
        if grant_id.is_nil() {
            return Err(Failure::MissingCapacitiesCredentials);
        }
        let mut destination = Self::new(token, space, tags)?;
        destination.grant_id = Some(grant_id);
        destination.deadline = Some(deadline);
        Ok(destination)
    }

    /// Existing HTTP fixture suites can exercise the same adapter on loopback.
    #[doc(hidden)]
    pub fn loopback_fixture(
        origin: &str,
        token: &str,
        space: Uuid,
        tags: BTreeSet<Uuid>,
    ) -> Result<Self> {
        Self::build(origin, token, space, tags, true)
    }

    fn build(
        origin: &str,
        token: &str,
        space: Uuid,
        tags: BTreeSet<Uuid>,
        fixture: bool,
    ) -> Result<Self> {
        let origin = Url::parse(origin).map_err(|_| Failure::InvalidInput)?;
        if fixture
            && (origin.scheme() != "http"
                || origin.host_str() != Some("127.0.0.1")
                || origin.port().is_none()
                || origin.path() != "/"
                || origin.query().is_some()
                || origin.fragment().is_some()
                || !origin.username().is_empty()
                || origin.password().is_some())
        {
            return Err(Failure::InvalidInput);
        }
        if space.is_nil() || tags.len() > 16 || tags.iter().any(Uuid::is_nil) {
            return Err(Failure::InvalidInput);
        }
        if token.is_empty() || token.len() > 4096 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Failure::MissingCapacitiesCredentials);
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Failure::MissingCapacitiesCredentials)?;
        auth.set_sensitive(true);
        let mut default_headers = HeaderMap::new();
        default_headers.insert(AUTHORIZATION, auth);
        default_headers.insert(REST_VERSION_HEADER, HeaderValue::from_static(REST_VERSION));
        let client = Client::builder()
            .default_headers(default_headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Failure::Transport)?;
        Ok(Self {
            client: Some(client),
            origin,
            space,
            tags,
            grant_id: None,
            deadline: None,
        })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        id: Option<Uuid>,
        payload: Option<Value>,
        budget: Budget,
    ) -> Result<T> {
        let mut url = self.origin.join(path).map_err(|_| Failure::InvalidInput)?;
        if let Some(id) = id {
            url.query_pairs_mut().append_pair("id", &id.to_string());
        }
        let mut request = self
            .client
            .as_ref()
            .ok_or(Failure::Authentication)?
            .request(method, url)
            .timeout(budget.request_timeout()?);
        if let Some(payload) = payload {
            request = request.json(&payload);
        }
        let mut response = request.send().await.map_err(transport)?;
        if !response.status().is_success() {
            return Err(match response.status() {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Failure::Authentication,
                StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited,
                StatusCode::NOT_FOUND | StatusCode::CONFLICT => Failure::Conflict,
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => Failure::InvalidInput,
                _ => Failure::Transport,
            });
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE as u64)
        {
            return Err(Failure::InvalidResponse);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if chunk.len() > MAX_RESPONSE - bytes.len() {
                return Err(Failure::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)
    }

    async fn verify_space(&self, budget: Budget) -> Result<()> {
        let space: Space = self
            .request(Method::GET, "space", None, None, budget)
            .await?;
        if space.id != self.space {
            return Err(Failure::Conflict);
        }
        Ok(())
    }

    async fn preflight(&self, budget: Budget) -> Result<()> {
        self.verify_space(budget).await?;
        self.writable_page(true, budget).await?;
        for tag in &self.tags {
            let found: ObjectIdentity = self
                .request(Method::GET, "object", Some(*tag), None, budget)
                .await?;
            if found.id.is_nil() || found.id != *tag || found.structure_id != "RootTag" {
                return Err(Failure::Conflict);
            }
        }
        Ok(())
    }

    async fn writable_page(&self, creating: bool, budget: Budget) -> Result<()> {
        let shapes: Structures = self
            .request(Method::GET, "space/structures", None, None, budget)
            .await?;
        let mut pages = shapes
            .structures
            .iter()
            .filter(|shape| shape.id == CAPACITIES_PAGE);
        let page = pages.next().ok_or(Failure::Unsupported)?;
        let mut content = page
            .property_definitions
            .iter()
            .filter(|p| p.id == "content");
        if pages.next().is_some()
            || !content.next().is_some_and(|p| p.writable)
            || content.next().is_some()
            || (creating
                && (!writable(page, "title", "title")
                    || (!self.tags.is_empty() && !writable(page, "tags", "entity"))))
        {
            return Err(Failure::Unsupported);
        }
        Ok(())
    }

    async fn create(&self, payload: Value, budget: Budget) -> Result<Uuid> {
        let found: ObjectIdentity = self
            .request(Method::POST, "object", None, Some(payload), budget)
            .await?;
        if found.id.is_nil() || found.structure_id != CAPACITIES_PAGE {
            return Err(Failure::InvalidResponse);
        }
        Ok(found.id)
    }

    async fn write(
        &self,
        payload: Value,
        attempt: &CapacitiesAttempt,
        budget: Budget,
    ) -> Result<Uuid> {
        if let Some(baseline) = attempt.managed.as_ref().and_then(|m| m.baseline.as_ref()) {
            let _: Value = self
                .request(Method::PATCH, "blocks/block", None, Some(payload), budget)
                .await?;
            // The acknowledgement cannot authorize rebinding to a different object.
            Ok(baseline.object)
        } else {
            self.create(payload, budget).await
        }
    }

    async fn preflight_plan(&self, attempt: &CapacitiesAttempt, budget: Budget) -> Result<()> {
        if let Some(baseline) = attempt.managed.as_ref().and_then(|m| m.baseline.as_ref()) {
            self.verify_space(budget).await?;
            self.writable_page(false, budget).await?;
            let object: Object = self
                .request(Method::GET, "object", Some(baseline.object), None, budget)
                .await?;
            let blocks = object_content(&object, attempt, baseline.object)?;
            let group = blocks
                .iter()
                .find(|b| b["id"].as_str() == Some(&baseline.block))
                .ok_or(Failure::Conflict)?;
            let verified = content::verify_group(group)?;
            if verified.identity_sha256 != baseline.identity_sha256 {
                return Err(Failure::Conflict);
            }
            Ok(())
        } else {
            self.preflight(budget).await
        }
    }

    async fn read_candidate(
        &self,
        attempt: &CapacitiesAttempt,
        id: Uuid,
        budget: Budget,
    ) -> Result<VerifiedBody> {
        self.verify_space(budget).await?;
        let found: Object = self
            .request(Method::GET, "object", Some(id), None, budget)
            .await?;
        verify_object(&found, attempt, id)
    }

    /// Create a new Page once. A returned ID must survive Ledger commit before readback.
    pub async fn export(
        &self,
        ledger: &Ledger,
        note: &PreparedNote,
        canceled: watch::Receiver<bool>,
    ) -> ExportReport {
        let Some(owner) = Owner::acquire(ledger) else {
            return ExportReport::new(ExportStatus::Busy, note.correlation, note.effect_operation);
        };
        self.owned_export(ledger, note, canceled, &owner, None, None)
            .await
    }

    pub(super) async fn owned_export(
        &self,
        ledger: &Ledger,
        note: &PreparedNote,
        mut canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        mut control: Option<&mut ExportControl<'_>>,
        mut access: Option<&mut Access>,
    ) -> ExportReport {
        let fallback = |status, failure| {
            let mut report = ExportReport::new(status, note.correlation, note.effect_operation);
            report.failure = failure;
            report
        };
        if !owner.belongs_to(ledger)
            || control.as_deref().is_some_and(|c| {
                !c.input
                    .as_ref()
                    .is_some_and(|input| input.matches(note).unwrap_or(false))
            })
        {
            return fallback(ExportStatus::Refused, Some(Failure::Storage));
        }
        if note.render(CorrelationLocation::Metadata).is_err() {
            return fallback(ExportStatus::Refused, Some(Failure::InvalidInput));
        }
        let id = CapacitiesAttempt::association_id(note.conversation, self.space, note.correlation);
        let initial = match ledger.export_association(note.conversation, id) {
            Ok(current) => current,
            Err(_) => return fallback(ExportStatus::Refused, Some(Failure::Storage)),
        };
        let original_prepared = initial.as_ref().is_some_and(|a| {
            a.outcome == Outcome::Prepared
                && a.capacities
                    .as_ref()
                    .is_some_and(|p| p.id == note.effect_operation)
        });
        let historical = match if original_prepared {
            Ok(None)
        } else {
            ledger.historical_export_attempt(note.conversation, id, note.effect_operation)
        } {
            Ok(found) => found,
            Err(_) => return fallback(ExportStatus::Refused, Some(Failure::Storage)),
        };
        if historical.is_none() && !original_prepared {
            if let Some(current) = initial.as_ref().filter(|a| {
                !matches!(
                    a.outcome,
                    Outcome::Completed | Outcome::Failed | Outcome::Canceled
                )
            }) {
                return report(current, ExportStatus::ReconcileRequired);
            }
        }
        let (payload, attempt) = match prepare(
            note,
            self.space,
            &self.tags,
            self.grant_id,
            historical.as_ref().or(initial.as_ref()),
        ) {
            Ok(plan) => plan,
            Err(failure) => return fallback(ExportStatus::Refused, Some(failure)),
        };
        let budget = self.budget();
        let budget = Budget {
            end: control
                .as_deref()
                .and_then(|c| c.deadline)
                .map_or(budget.end, |end| end.min(budget.end)),
        };

        // Guarded Writer preparation has its own receipt fingerprint. A current
        // exact Prepared plan is unresolved custody, not a generic historical
        // receipt or permission to resend after an interrupted publication.
        match ledger.export_association(note.conversation, id) {
            Ok(Some(current))
                if current.outcome == Outcome::Prepared
                    && current
                        .capacities
                        .as_ref()
                        .is_some_and(|old| old.id == attempt.id) =>
            {
                return if current.capacities.as_ref() == Some(&attempt)
                    && matches_plan(&current, note, &attempt)
                {
                    report(&current, ExportStatus::ReconcileRequired)
                } else {
                    fallback(ExportStatus::Conflict, Some(Failure::Conflict))
                };
            }
            Err(_) => return fallback(ExportStatus::Refused, Some(Failure::Storage)),
            _ => {}
        }
        if let Some(found) = historical {
            if !matches_plan(&found, note, &attempt) {
                return fallback(ExportStatus::Conflict, Some(Failure::Conflict));
            }
            if control.is_some() || access.is_some() {
                return if matches!(
                    found.outcome,
                    Outcome::Completed | Outcome::Failed | Outcome::Canceled
                ) {
                    terminal(&found)
                } else {
                    report(&found, ExportStatus::ReconcileRequired)
                };
            }
            return self
                .readback(ledger, found, &mut canceled, budget, None)
                .await;
        }
        let expected = (|| {
            let root = ledger.expected(Namespace::Conversation, note.conversation)?;
            let heads = ledger.expected(Namespace::ExportAssociation, id)?;
            let current = ledger.export_association(note.conversation, id)?;
            Ok::<_, anyhow::Error>((root, heads, current))
        })();
        let Ok((root, heads, current)) = expected else {
            return fallback(ExportStatus::Refused, Some(Failure::Storage));
        };
        if current != initial {
            return fallback(ExportStatus::Conflict, Some(Failure::Conflict));
        }
        if let Some(old) = current {
            if old.source_scope != note.source_scope || old.capacities.is_none() {
                return fallback(ExportStatus::Conflict, Some(Failure::Conflict));
            }
            match old.outcome {
                Outcome::Completed | Outcome::Canceled | Outcome::Failed => {}
                _ => return report(&old, ExportStatus::ReconcileRequired),
            }
        }

        if control.is_none() && access.is_none() {
            match pending(self.preflight_plan(&attempt, budget), &mut canceled, budget).await {
                Ok(Some(())) => {}
                Ok(None) => return fallback(ExportStatus::CanceledNoEffect, None),
                Err(failure) => return fallback(ExportStatus::Refused, Some(failure)),
            }
        }
        let mut association = ExportAssociation {
            id,
            conversation: note.conversation,
            backend: CAPACITIES_BACKEND.to_owned(),
            native_note: None,
            source_scope: note.source_scope.clone(),
            source_revision: note.source_revision.clone(),
            operation: CapacitiesAttempt::stage_id(attempt.id, "prepared"),
            outcome: Outcome::Prepared,
            attempt: None,
            capacities: Some(attempt),
        };
        event(&mut control, ExportPhase::BeforeAdmission, ledger);
        if *canceled.borrow() || canceled.has_changed().is_err() {
            return fallback(ExportStatus::CanceledNoEffect, None);
        }
        let admission = if let Some(c) = control.as_deref_mut() {
            let Some(input) = c.input.take() else {
                return fallback(ExportStatus::Refused, Some(Failure::Storage));
            };
            ledger
                .associate_writer_export_observed(
                    association.operation,
                    heads,
                    association.clone(),
                    input,
                )
                .map(|(result, fresh, guard)| {
                    c.guard = guard;
                    (result, fresh)
                })
        } else {
            ledger.associate_export_observed(
                association.operation,
                root,
                heads,
                association.clone(),
            )
        };
        match admission {
            Ok((_, true)) => {}
            Ok((_, false)) => return report(&association, ExportStatus::ReconcileRequired),
            Err(_) => return fallback(ExportStatus::Refused, Some(Failure::Storage)),
        }

        event(&mut control, ExportPhase::AfterAdmission, ledger);
        let authorized;
        let destination = if let Some(access) = access.as_deref_mut() {
            if *canceled.borrow() || canceled.has_changed().is_err() {
                return no_send(ledger, association, None, &mut control);
            }
            if !selection_current(&mut control, ledger) {
                return no_send(ledger, association, Some(Failure::Conflict), &mut control);
            }
            let selection = || {
                if control.as_deref().is_some_and(|c| {
                    !c.guard
                        .as_ref()
                        .is_some_and(|guard| guard.current(ledger).unwrap_or(false))
                }) {
                    Err(Failure::Conflict)
                } else {
                    Ok(())
                }
            };
            let token = match access
                .current(
                    ledger.store().actor_id,
                    budget.end,
                    canceled.clone(),
                    selection,
                )
                .await
            {
                Ok(token) => token,
                Err(failure) => {
                    let failure =
                        (!*canceled.borrow() && canceled.has_changed().is_ok()).then_some(failure);
                    return no_send(ledger, association, failure, &mut control);
                }
            };
            authorized = match Self::enrolled(
                token.expose(),
                self.space,
                self.grant_id.expect("configured grant"),
                self.tags.clone(),
                budget.end,
            ) {
                Ok(destination) => destination,
                Err(failure) => return no_send(ledger, association, Some(failure), &mut control),
            };
            &authorized
        } else {
            self
        };
        if control.is_some() || access.is_some() {
            event(&mut control, ExportPhase::BeforeDestinationPoll, ledger);
            if !selection_current(&mut control, ledger) {
                return no_send(ledger, association, Some(Failure::Conflict), &mut control);
            }
            if let Some(access) = access.as_deref() {
                if let Err(failure) = access.config_current() {
                    return no_send(ledger, association, Some(failure), &mut control);
                }
            }
            let preflight = async {
                if let Some(c) = control.as_deref_mut() {
                    c.observation.destination_polled = true;
                }
                destination
                    .preflight_plan(
                        association.capacities.as_ref().expect("Capacities plan"),
                        budget,
                    )
                    .await
            };
            match pending(preflight, &mut canceled, budget).await {
                Ok(Some(())) => {}
                Ok(None) => return no_send(ledger, association, None, &mut control),
                Err(failure) => return no_send(ledger, association, Some(failure), &mut control),
            }
        }

        let (result, polled) = dispatch(
            destination.write(
                payload,
                association.capacities.as_ref().expect("Capacities plan"),
                budget,
            ),
            &mut canceled,
            budget,
            &mut || {
                event(&mut control, ExportPhase::BeforeWritePoll, ledger);
                if selection_current(&mut control, ledger) {
                    access.as_deref().map_or(Ok(()), Access::config_current)
                } else {
                    Err(Failure::Conflict)
                }
            },
        )
        .await;
        if let Some(c) = control.as_deref_mut() {
            c.observation.write_polled |= polled;
        }
        match result {
            Ok(Some(candidate)) => {
                association
                    .capacities
                    .as_mut()
                    .expect("Capacities plan")
                    .candidate = Some(candidate);
                transition(&mut association, Outcome::ReconcileRequired, "candidate");
                if store_state(ledger, association.clone()).is_err() {
                    return with_failure(
                        &association,
                        ExportStatus::ReconcileRequired,
                        Failure::Storage,
                    );
                }
                destination
                    .readback(
                        ledger,
                        association,
                        &mut canceled,
                        budget,
                        control.as_deref_mut().map(|c| &mut *c.observation),
                    )
                    .await
            }
            result if polled => {
                let stage = if association
                    .capacities
                    .as_ref()
                    .is_some_and(|a| a.candidate.is_some())
                {
                    "candidate"
                } else {
                    "uncertain"
                };
                transition(&mut association, Outcome::ReconcileRequired, stage);
                if store_state(ledger, association.clone()).is_err() {
                    return with_failure(
                        &association,
                        ExportStatus::ReconcileRequired,
                        Failure::Storage,
                    );
                }
                let mut result_report = report(&association, ExportStatus::ReconcileRequired);
                result_report.failure = result.err();
                result_report
            }
            result => no_send(ledger, association, result.err(), &mut control),
        }
    }

    /// Reopen the scoped association without inferring a native ID from search.
    pub async fn reconcile(
        &self,
        ledger: &Ledger,
        conversation: Uuid,
        correlation: Uuid,
        canceled: watch::Receiver<bool>,
    ) -> ExportReport {
        let Some(owner) = Owner::acquire(ledger) else {
            return ExportReport::new(ExportStatus::Busy, correlation, Uuid::nil());
        };
        self.owned_reconcile(ledger, conversation, correlation, canceled, &owner, None)
            .await
    }
    async fn owned_reconcile(
        &self,
        ledger: &Ledger,
        conversation: Uuid,
        correlation: Uuid,
        mut canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        observation: Option<&mut ExportObservation>,
    ) -> ExportReport {
        if !owner.belongs_to(ledger) {
            let mut result = ExportReport::new(ExportStatus::Refused, correlation, Uuid::nil());
            result.failure = Some(Failure::Storage);
            return result;
        }
        let id = CapacitiesAttempt::association_id(conversation, self.space, correlation);
        let Ok(Some(association)) = ledger.export_association(conversation, id) else {
            let mut result = ExportReport::new(ExportStatus::Refused, correlation, Uuid::nil());
            result.failure = Some(Failure::Storage);
            return result;
        };
        self.readback(
            ledger,
            association,
            &mut canceled,
            self.budget(),
            observation,
        )
        .await
    }

    fn budget(&self) -> Budget {
        let default = Budget::default();
        Budget {
            end: self
                .deadline
                .map_or(default.end, |end| end.min(default.end)),
        }
    }

    async fn readback(
        &self,
        ledger: &Ledger,
        mut association: ExportAssociation,
        canceled: &mut watch::Receiver<bool>,
        budget: Budget,
        observation: Option<&mut ExportObservation>,
    ) -> ExportReport {
        let Some(attempt) = association.capacities.as_ref() else {
            return with_failure(&association, ExportStatus::Conflict, Failure::Conflict);
        };
        if attempt.space != self.space || attempt.grant_id != self.grant_id {
            return with_failure(&association, ExportStatus::Conflict, Failure::Conflict);
        }
        if let Some(local) = local_reconciliation(&association) {
            return local;
        }
        let Some(id) = attempt.candidate else {
            return report(&association, ExportStatus::ReconcileRequired);
        };
        let read = async {
            if let Some(observation) = observation {
                observation.destination_polled = true;
            }
            self.read_candidate(attempt, id, budget).await
        };
        match pending(read, canceled, budget).await {
            Ok(Some(block)) => {
                // A crash after an update was sent can leave Prepared with its
                // already-known original target. Persist the missing candidate
                // transition before completing from read-only evidence.
                if association.outcome == Outcome::Prepared {
                    transition(&mut association, Outcome::ReconcileRequired, "candidate");
                    if store_state(ledger, association.clone()).is_err() {
                        return with_failure(
                            &association,
                            ExportStatus::ReconcileRequired,
                            Failure::Storage,
                        );
                    }
                }
                let attempt = association.capacities.as_mut().expect("Capacities plan");
                attempt.verified_body_block = Some(block.id);
                if let Some(managed) = &mut attempt.managed {
                    managed.confirmed_sha256 = block.identity_sha256;
                }
                association.native_note = Some(id.to_string());
                transition(&mut association, Outcome::Completed, "completed");
                if store_state(ledger, association.clone()).is_err() {
                    let mut result = with_failure(
                        &association,
                        ExportStatus::ReconcileRequired,
                        Failure::Storage,
                    );
                    result.native_note = None;
                    return result;
                }
                report(&association, ExportStatus::VerifiedCurrent)
            }
            Ok(None) => report(&association, ExportStatus::ReconcileRequired),
            Err(failure) => with_failure(&association, ExportStatus::ReconcileRequired, failure),
        }
    }
}

pub(super) fn local_reconciliation(association: &ExportAssociation) -> Option<ExportReport> {
    if matches!(
        association.outcome,
        Outcome::Completed | Outcome::Failed | Outcome::Canceled
    ) {
        return Some(terminal(association));
    }
    association
        .capacities
        .as_ref()
        .filter(|attempt| attempt.candidate.is_none())
        .map(|_| report(association, ExportStatus::ReconcileRequired))
}

impl super::dispatch::OwnedDestination for CapacitiesDestination {
    fn backend(&self) -> &str {
        CAPACITIES_BACKEND
    }
    fn association_id(&self, conversation: Uuid, correlation: Uuid) -> Uuid {
        CapacitiesAttempt::association_id(conversation, self.space, correlation)
    }
    fn intent_binding(&self) -> String {
        intent_binding(self.space, &self.tags, self.grant_id)
    }
    async fn export_owned(
        &self,
        ledger: &Ledger,
        note: &PreparedNote,
        canceled: watch::Receiver<bool>,
        owner: &ExportOwner<'_>,
        control: Option<&mut ExportControl<'_>>,
    ) -> ExportReport {
        self.owned_export(ledger, note, canceled, owner, control, None)
            .await
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
        self.owned_reconcile(
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

pub(super) fn intent_binding(space: Uuid, tags: &BTreeSet<Uuid>, grant: Option<Uuid>) -> String {
    // Fixed UUID/ordered UUID set serialization cannot fail; no token enters the intent.
    if let Some(grant) = grant {
        serde_json::to_string(&(CAPACITIES_BACKEND, space, CAPACITIES_PAGE, tags, grant))
    } else {
        serde_json::to_string(&(CAPACITIES_BACKEND, space, CAPACITIES_PAGE, tags))
    }
    .expect("Capacities intent primitives")
}

fn no_send(
    ledger: &Ledger,
    mut association: ExportAssociation,
    failure: Option<Failure>,
    control: &mut Option<&mut ExportControl<'_>>,
) -> ExportReport {
    if failure.is_some() {
        transition(&mut association, Outcome::Failed, "failed-no-send");
    } else {
        transition(&mut association, Outcome::Canceled, "canceled-no-send");
    }
    let saved = store_state(ledger, association.clone()).is_ok();
    if let Some(c) = control.as_deref_mut() {
        c.observation.no_send_recorded = Some(saved);
    }
    if !saved {
        return with_failure(
            &association,
            ExportStatus::ReconcileRequired,
            Failure::Storage,
        );
    }
    let mut result = terminal(&association);
    result.failure = failure;
    result
}

fn transport(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Deadline
    } else {
        Failure::Transport
    }
}

fn writable(shape: &Structure, id: &str, kind: &str) -> bool {
    let mut definitions = shape
        .property_definitions
        .iter()
        .filter(|property| property.id == id);
    definitions
        .next()
        .is_some_and(|property| property.writable && property.kind == kind)
        && definitions.next().is_none()
}

fn marker(correlation: Uuid, effect: Uuid) -> String {
    format!("[RMB:{correlation}] [RMB-OP:{effect}]")
}

fn prepare(
    note: &PreparedNote,
    space: Uuid,
    tags: &BTreeSet<Uuid>,
    grant_id: Option<Uuid>,
    prior: Option<&ExportAssociation>,
) -> Result<(Value, CapacitiesAttempt)> {
    let old = prior.and_then(|a| a.capacities.as_ref());
    if old.is_some_and(|a| a.id == note.effect_operation && a.evidence_version < 3) {
        return prepare_legacy(note, space, tags, grant_id);
    }
    let group = content::group(&note.markdown)?;
    let desired_sha256 = content::semantic_digest(&group)?;
    let original = old.filter(|a| a.id == note.effect_operation);
    let (creation_effect, baseline) = if let Some(original) = original {
        let managed = original.managed.as_ref().ok_or(Failure::Conflict)?;
        (managed.creation_effect, managed.baseline.clone())
    } else if let Some(prior) = prior {
        let old = old.ok_or(Failure::Conflict)?;
        match prior.outcome {
            Outcome::Completed => {
                let managed = old.managed.as_ref().ok_or(Failure::Unsupported)?;
                if old.grant_id != grant_id {
                    return Err(Failure::Conflict);
                }
                (
                    managed.creation_effect,
                    Some(CapacitiesBaseline {
                        object: old.candidate.ok_or(Failure::Conflict)?,
                        block: old.verified_body_block.clone().ok_or(Failure::Conflict)?,
                        identity_sha256: managed
                            .confirmed_sha256
                            .clone()
                            .ok_or(Failure::Conflict)?,
                    }),
                )
            }
            Outcome::Failed | Outcome::Canceled => {
                if let Some(managed) = old
                    .managed
                    .as_ref()
                    .filter(|m| m.action == ExportAction::Update)
                {
                    if old.grant_id != grant_id {
                        return Err(Failure::Conflict);
                    }
                    (managed.creation_effect, managed.baseline.clone())
                } else {
                    (note.effect_operation, None)
                }
            }
            _ => return Err(Failure::Conflict),
        }
    } else {
        (note.effect_operation, None)
    };
    let payload = if let Some(baseline) = &baseline {
        json!({"id":baseline.object, "propertyId":"content", "blockId":baseline.block, "block":group})
    } else {
        let mut properties = json!({"title":{"type":"title","title":{"value":note.title}}});
        if !tags.is_empty() {
            properties["tags"] = json!({"type":"entity","entity":tags.iter().map(|id|json!({"id":id})).collect::<Vec<_>>()});
        }
        json!({"structureId": CAPACITIES_PAGE, "properties":properties,"blocks":{"content":[
            {"type":"TextBlock","tokens":[{"type":"TextToken","text":marker(note.correlation,creation_effect),"style":{}}],
             "blocks":[],"hierarchy":{"key":"Base","val":0}}, group
        ]}})
    };
    let encoded = serde_json::to_vec(&payload).map_err(|_| Failure::InvalidInput)?;
    if encoded.len() > MAX_INPUT {
        return Err(Failure::InvalidInput);
    }
    let mut attempt = CapacitiesAttempt {
        evidence_version: 3,
        id: note.effect_operation,
        space,
        structure: CAPACITIES_PAGE.to_owned(),
        correlation: note.correlation,
        payload_sha256: digest(&encoded),
        body_sha256: digest(note.markdown.as_bytes()),
        tags: tags.clone(),
        grant_id,
        candidate: baseline.as_ref().map(|b| b.object),
        verified_body_block: None,
        managed: Some(CapacitiesManaged {
            mapping_revision: 1,
            action: if baseline.is_some() {
                ExportAction::Update
            } else {
                ExportAction::Create
            },
            creation_effect,
            desired_sha256,
            baseline,
            confirmed_sha256: None,
        }),
    };
    // Terminal history keeps its exact verified facts; no new plan is inferred
    // from a newer association when replaying an original effect identity.
    if let Some(original) = original {
        attempt.candidate = original.candidate;
        attempt.verified_body_block = original.verified_body_block.clone();
        attempt
            .managed
            .as_mut()
            .expect("managed plan")
            .confirmed_sha256 = original
            .managed
            .as_ref()
            .and_then(|m| m.confirmed_sha256.clone());
    }
    Ok((payload, attempt))
}

fn prepare_legacy(
    note: &PreparedNote,
    space: Uuid,
    tags: &BTreeSet<Uuid>,
    grant_id: Option<Uuid>,
) -> Result<(Value, CapacitiesAttempt)> {
    let mut properties = json!({"title": {"type": "title", "title": {"value": note.title}}});
    if !tags.is_empty() {
        let entries: Vec<_> = tags.iter().map(|id| json!({"id": id})).collect();
        properties["tags"] = json!({"type": "entity", "entity": entries});
    }
    let payload = json!({
        "structureId": CAPACITIES_PAGE,
        "properties": properties,
        "blocks": {"content": [
            {"type": "TextBlock", "tokens": [
                {"type": "TextToken", "text": marker(note.correlation, note.effect_operation), "style": {}}
            ], "hierarchy": {"key": "Base", "val": 0}},
            {"type": "CodeBlock", "lang": "markdown", "text": note.markdown}
        ]}
    });
    let encoded = serde_json::to_vec(&payload).map_err(|_| Failure::InvalidInput)?;
    if encoded.len() > MAX_INPUT {
        return Err(Failure::InvalidInput);
    }
    let attempt = CapacitiesAttempt {
        evidence_version: if grant_id.is_some() { 2 } else { 1 },
        id: note.effect_operation,
        space,
        structure: CAPACITIES_PAGE.to_owned(),
        correlation: note.correlation,
        payload_sha256: digest(&encoded),
        body_sha256: digest(note.markdown.as_bytes()),
        tags: tags.clone(),
        grant_id,
        candidate: None,
        verified_body_block: None,
        managed: None,
    };
    Ok((payload, attempt))
}

fn object_content<'a>(
    object: &'a Object,
    attempt: &CapacitiesAttempt,
    id: Uuid,
) -> Result<&'a Vec<Value>> {
    if object.identity.id != id || object.identity.structure_id != attempt.structure {
        return Err(Failure::Conflict);
    }
    if attempt
        .managed
        .as_ref()
        .is_none_or(|m| m.action == ExportAction::Create)
    {
        let title = object.properties.get("title").ok_or(Failure::Conflict)?;
        if title["type"] != "title"
            || !title["title"]["value"]
                .as_str()
                .is_some_and(|title| !title.trim().is_empty())
        {
            return Err(Failure::Conflict);
        }
        if !attempt.tags.is_empty() {
            let tags = object.properties.get("tags").ok_or(Failure::Conflict)?;
            if tags["type"] != "entity" {
                return Err(Failure::Conflict);
            }
            let entries = tags["entity"].as_array().ok_or(Failure::Conflict)?;
            let ids = entries
                .iter()
                .map(|entry| {
                    entry["id"]
                        .as_str()
                        .and_then(|id| Uuid::parse_str(id).ok())
                        .ok_or(Failure::Conflict)
                })
                .collect::<Result<BTreeSet<_>>>()?;
            if !attempt.tags.is_subset(&ids) {
                return Err(Failure::Conflict);
            }
        }
    }

    let content = object.blocks.get("content").ok_or(Failure::Conflict)?;
    if attempt.managed.is_some() {
        let mut ids = BTreeSet::new();
        unique_ids(content, &mut ids, 0)?;
    }
    let creation_effect = attempt
        .managed
        .as_ref()
        .map_or(attempt.id, |m| m.creation_effect);
    let expected = marker(attempt.correlation, creation_effect);
    let mut markers = content.iter().filter(|block| {
        block["type"] == "TextBlock" && block["tokens"].as_array().is_some_and(|tokens| {
            matches!(tokens.as_slice(), [token] if token["type"] == "TextToken" && token["text"] == expected)
        })
    });
    let Some(marker) = markers.next() else {
        return Err(Failure::Conflict);
    };
    let marker_id = block_id(marker)?;
    if markers.next().is_some()
        || content
            .iter()
            .filter(|b| b["id"].as_str() == Some(marker_id))
            .count()
            != 1
    {
        return Err(Failure::Conflict);
    }
    Ok(content)
}

fn unique_ids(blocks: &[Value], ids: &mut BTreeSet<String>, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(Failure::Conflict);
    }
    for block in blocks {
        if ids.len() >= 4096 || !ids.insert(block_id(block)?.to_owned()) {
            return Err(Failure::Conflict);
        }
        if let Some(children) = block.get("blocks") {
            unique_ids(
                children.as_array().ok_or(Failure::Conflict)?,
                ids,
                depth + 1,
            )?;
        }
    }
    Ok(())
}

fn verify_object(object: &Object, attempt: &CapacitiesAttempt, id: Uuid) -> Result<VerifiedBody> {
    let content = object_content(object, attempt, id)?;
    if let Some(managed) = &attempt.managed {
        let mut matched = None;
        for group in content.iter().filter(|b| b["type"] == "GroupBlock") {
            if managed
                .baseline
                .as_ref()
                .is_some_and(|b| group["id"].as_str() != Some(&b.block))
            {
                continue;
            }
            let verified = match content::verify_group(group) {
                Ok(value) => value,
                Err(failure) if managed.baseline.is_some() => return Err(failure),
                Err(_) => continue,
            };
            if verified.semantic_sha256 == managed.desired_sha256 {
                if matched.is_some() {
                    return Err(Failure::Conflict);
                }
                matched = Some(VerifiedBody {
                    id: verified.id,
                    identity_sha256: Some(verified.identity_sha256),
                });
            }
        }
        return matched.ok_or(Failure::Conflict);
    }
    let mut bodies = content.iter().filter(|block| {
        block["type"] == "CodeBlock"
            && block["lang"] == "markdown"
            && block["text"]
                .as_str()
                .is_some_and(|body| digest(body.as_bytes()) == attempt.body_sha256)
    });
    let body_id = block_id(bodies.next().ok_or(Failure::Conflict)?)?;
    if bodies.next().is_some()
        || content
            .iter()
            .filter(|b| b["id"].as_str() == Some(body_id))
            .count()
            != 1
    {
        return Err(Failure::Conflict);
    }
    Ok(VerifiedBody {
        id: body_id.to_owned(),
        identity_sha256: None,
    })
}

fn block_id(block: &Value) -> Result<&str> {
    block["id"]
        .as_str()
        .filter(|id| (1..=1024).contains(&id.len()) && !id.chars().any(char::is_control))
        .ok_or(Failure::Conflict)
}

fn matches_plan(
    association: &ExportAssociation,
    note: &PreparedNote,
    attempt: &CapacitiesAttempt,
) -> bool {
    association.source_scope == note.source_scope
        && association.source_revision == note.source_revision
        && association
            .capacities
            .as_ref()
            .is_some_and(|old| old.same_plan(attempt))
}

fn transition(association: &mut ExportAssociation, outcome: Outcome, stage: &str) {
    let effect = association.capacities.as_ref().expect("Capacities plan").id;
    association.operation = CapacitiesAttempt::stage_id(effect, stage);
    association.outcome = outcome;
}

fn terminal(association: &ExportAssociation) -> ExportReport {
    report(
        association,
        match association.outcome {
            Outcome::Completed => ExportStatus::HistoricalCompleted,
            Outcome::Canceled => ExportStatus::CanceledNoEffect,
            _ => ExportStatus::Refused,
        },
    )
}

fn with_failure(
    association: &ExportAssociation,
    status: ExportStatus,
    failure: Failure,
) -> ExportReport {
    let mut result = report(association, status);
    result.failure = Some(failure);
    result
}

#[cfg(test)]
mod legacy_tests {
    use super::*;

    #[test]
    fn legacy_versions_keep_literal_readback_and_refuse_implicit_group_migration() {
        for grant in [None, Some(Uuid::new_v4())] {
            let note = PreparedNote {
                conversation: Uuid::new_v4(),
                correlation: Uuid::new_v4(),
                effect_operation: Uuid::new_v4(),
                source_scope: "fixture".into(),
                source_revision: "1".into(),
                title: "Title".into(),
                markdown: "# Legacy **source**".into(),
            };
            let (mut payload, mut attempt) =
                prepare_legacy(&note, Uuid::new_v4(), &BTreeSet::new(), grant).unwrap();
            assert_eq!(
                attempt.evidence_version,
                if grant.is_some() { 2 } else { 1 }
            );
            assert!(attempt.managed.is_none());
            let object_id = Uuid::new_v4();
            payload["id"] = json!(object_id);
            payload["blocks"]["content"][0]["id"] = json!("legacy-marker");
            payload["blocks"]["content"][1]["id"] = json!("legacy-body");
            // An unrelated legacy sibling must not acquire the new managed-tree
            // shape requirements during an original readback.
            payload["blocks"]["content"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "id":"old-unrelated", "type":"TextBlock", "blocks":"legacy-extension"
                }));
            let object: Object = serde_json::from_value(payload).unwrap();
            let verified = verify_object(&object, &attempt, object_id).unwrap();
            assert_eq!(verified.id, "legacy-body");
            assert!(verified.identity_sha256.is_none());
            attempt.candidate = Some(object_id);
            attempt.verified_body_block = Some(verified.id);
            let association = ExportAssociation {
                id: CapacitiesAttempt::association_id(
                    note.conversation,
                    attempt.space,
                    note.correlation,
                ),
                conversation: note.conversation,
                backend: CAPACITIES_BACKEND.into(),
                native_note: Some(object_id.to_string()),
                source_scope: note.source_scope.clone(),
                source_revision: note.source_revision.clone(),
                operation: CapacitiesAttempt::stage_id(attempt.id, "completed"),
                outcome: Outcome::Completed,
                attempt: None,
                capacities: Some(attempt.clone()),
            };
            let (_, historical) = prepare(
                &note,
                attempt.space,
                &BTreeSet::new(),
                grant,
                Some(&association),
            )
            .unwrap();
            assert!(historical.same_plan(&attempt));
            let mut new = note.clone();
            new.effect_operation = Uuid::new_v4();
            assert!(matches!(
                prepare(
                    &new,
                    attempt.space,
                    &BTreeSet::new(),
                    grant,
                    Some(&association)
                ),
                Err(Failure::Unsupported)
            ));
        }
    }
}
