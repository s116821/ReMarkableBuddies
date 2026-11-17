//! Capacities REST create and exact-ID readback for explicitly prepared notes.
use super::{
    run::{dispatch, pending, report, store_state, Owner},
    *,
};
use crate::{
    conversation::{CapacitiesAttempt, Ledger, Outcome, CAPACITIES_BACKEND, CAPACITIES_PAGE},
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

const MAX_RESPONSE: usize = 1024 * 1024;

pub struct CapacitiesDestination {
    client: Client,
    origin: Url,
    space: Uuid,
    tags: BTreeSet<Uuid>,
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

impl CapacitiesDestination {
    /// The caller supplies an authorized REST access token for its selected space.
    pub fn new(token: &str, space: Uuid, tags: BTreeSet<Uuid>) -> Result<Self> {
        Self::build("https://api.capacities.io/", token, space, tags, false)
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
            client,
            origin,
            space,
            tags,
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
        let shapes: Structures = self
            .request(Method::GET, "space/structures", None, None, budget)
            .await?;
        let mut pages = shapes
            .structures
            .iter()
            .filter(|shape| shape.id == CAPACITIES_PAGE);
        let page = pages.next().ok_or(Failure::Unsupported)?;
        if pages.next().is_some()
            || !writable(page, "title", "title")
            || (!self.tags.is_empty() && !writable(page, "tags", "entity"))
        {
            return Err(Failure::Unsupported);
        }
        for tag in &self.tags {
            let found: ObjectIdentity = self
                .request(Method::GET, "object", Some(*tag), None, budget)
                .await?;
            if found.id != *tag || found.structure_id != "RootTag" {
                return Err(Failure::Conflict);
            }
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

    async fn read_candidate(
        &self,
        attempt: &CapacitiesAttempt,
        id: Uuid,
        budget: Budget,
    ) -> Result<String> {
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
        mut canceled: watch::Receiver<bool>,
    ) -> ExportReport {
        let fallback = |status, failure| {
            let mut report = ExportReport::new(status, note.correlation, note.effect_operation);
            report.failure = failure;
            report
        };
        let Some(_owner) = Owner::acquire(ledger) else {
            return fallback(ExportStatus::Busy, None);
        };
        if note.render(CorrelationLocation::Metadata).is_err() {
            return fallback(ExportStatus::Refused, Some(Failure::InvalidInput));
        }
        let id = CapacitiesAttempt::association_id(note.conversation, self.space, note.correlation);
        let (payload, attempt) = match prepare(note, self.space, &self.tags) {
            Ok(plan) => plan,
            Err(failure) => return fallback(ExportStatus::Refused, Some(failure)),
        };
        let budget = Budget::default();

        match ledger.historical_export_attempt(note.conversation, id, attempt.id) {
            Ok(Some(found)) => {
                if !matches_plan(&found, note, &attempt) {
                    return fallback(ExportStatus::Conflict, Some(Failure::Conflict));
                }
                return self.readback(ledger, found, &mut canceled, budget).await;
            }
            Ok(None) => {}
            Err(_) => return fallback(ExportStatus::Refused, Some(Failure::Storage)),
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
        if let Some(old) = current {
            if old.source_scope != note.source_scope || old.capacities.is_none() {
                return fallback(ExportStatus::Conflict, Some(Failure::Conflict));
            }
            match old.outcome {
                Outcome::Completed => {
                    return fallback(ExportStatus::Refused, Some(Failure::Unsupported))
                }
                Outcome::Canceled | Outcome::Failed => {}
                _ => return report(&old, ExportStatus::ReconcileRequired),
            }
        }

        match pending(self.preflight(budget), &mut canceled, budget).await {
            Ok(Some(())) => {}
            Ok(None) => return fallback(ExportStatus::CanceledNoEffect, None),
            Err(failure) => return fallback(ExportStatus::Refused, Some(failure)),
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
        match ledger.associate_export_observed(
            association.operation,
            root,
            heads,
            association.clone(),
        ) {
            Ok((_, true)) => {}
            Ok((_, false)) => return report(&association, ExportStatus::ReconcileRequired),
            Err(_) => return fallback(ExportStatus::Refused, Some(Failure::Storage)),
        }

        let (result, polled) = dispatch(
            self.create(payload, budget),
            &mut canceled,
            budget,
            &mut || Ok(()),
        )
        .await;
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
                self.readback(ledger, association, &mut canceled, budget)
                    .await
            }
            result if polled => {
                transition(&mut association, Outcome::ReconcileRequired, "uncertain");
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
            result => {
                let failure = result.err();
                if failure.is_some() {
                    transition(&mut association, Outcome::Failed, "failed-no-send");
                } else {
                    transition(&mut association, Outcome::Canceled, "canceled-no-send");
                }
                if store_state(ledger, association.clone()).is_err() {
                    return with_failure(
                        &association,
                        ExportStatus::ReconcileRequired,
                        Failure::Storage,
                    );
                }
                let mut result_report = terminal(&association);
                result_report.failure = failure;
                result_report
            }
        }
    }

    /// Reopen the scoped association without inferring a native ID from search.
    pub async fn reconcile(
        &self,
        ledger: &Ledger,
        conversation: Uuid,
        correlation: Uuid,
        mut canceled: watch::Receiver<bool>,
    ) -> ExportReport {
        let Some(_owner) = Owner::acquire(ledger) else {
            return ExportReport::new(ExportStatus::Busy, correlation, Uuid::nil());
        };
        let id = CapacitiesAttempt::association_id(conversation, self.space, correlation);
        let Ok(Some(association)) = ledger.export_association(conversation, id) else {
            let mut result = ExportReport::new(ExportStatus::Refused, correlation, Uuid::nil());
            result.failure = Some(Failure::Storage);
            return result;
        };
        self.readback(ledger, association, &mut canceled, Budget::default())
            .await
    }

    async fn readback(
        &self,
        ledger: &Ledger,
        mut association: ExportAssociation,
        canceled: &mut watch::Receiver<bool>,
        budget: Budget,
    ) -> ExportReport {
        let Some(attempt) = association.capacities.as_mut() else {
            return with_failure(&association, ExportStatus::Conflict, Failure::Conflict);
        };
        if attempt.space != self.space {
            return with_failure(&association, ExportStatus::Conflict, Failure::Conflict);
        }
        if matches!(
            association.outcome,
            Outcome::Completed | Outcome::Failed | Outcome::Canceled
        ) {
            return terminal(&association);
        }
        let Some(id) = attempt.candidate else {
            return report(&association, ExportStatus::ReconcileRequired);
        };
        match pending(self.read_candidate(attempt, id, budget), canceled, budget).await {
            Ok(Some(block)) => {
                attempt.verified_body_block = Some(block);
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
        evidence_version: 1,
        id: note.effect_operation,
        space,
        structure: CAPACITIES_PAGE.to_owned(),
        correlation: note.correlation,
        payload_sha256: digest(&encoded),
        body_sha256: digest(note.markdown.as_bytes()),
        tags: tags.clone(),
        candidate: None,
        verified_body_block: None,
    };
    Ok((payload, attempt))
}

fn verify_object(object: &Object, attempt: &CapacitiesAttempt, id: Uuid) -> Result<String> {
    if object.identity.id != id || object.identity.structure_id != attempt.structure {
        return Err(Failure::Conflict);
    }
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

    let content = object.blocks.get("content").ok_or(Failure::Conflict)?;
    let expected = marker(attempt.correlation, attempt.id);
    let mut markers = content.iter().filter(|block| {
        block["type"] == "TextBlock" && block["tokens"].as_array().is_some_and(|tokens| {
            matches!(tokens.as_slice(), [token] if token["type"] == "TextToken" && token["text"] == expected)
        })
    });
    let Some(marker) = markers.next() else {
        return Err(Failure::Conflict);
    };
    let marker_id = block_id(marker)?;
    if markers.next().is_some() {
        return Err(Failure::Conflict);
    }

    let mut bodies = content.iter().filter(|block| {
        block["type"] == "CodeBlock"
            && block["lang"] == "markdown"
            && block["text"]
                .as_str()
                .is_some_and(|body| digest(body.as_bytes()) == attempt.body_sha256)
    });
    let body_id = block_id(bodies.next().ok_or(Failure::Conflict)?)?;
    if body_id == marker_id
        || bodies.next().is_some()
        || content
            .iter()
            .filter(|block| block["id"].as_str() == Some(body_id))
            .count()
            != 1
        || content
            .iter()
            .filter(|block| block["id"].as_str() == Some(marker_id))
            .count()
            != 1
    {
        return Err(Failure::Conflict);
    }
    Ok(body_id.to_owned())
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
            .is_some_and(|old| old.payload_sha256 == attempt.payload_sha256)
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
