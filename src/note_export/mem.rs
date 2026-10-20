//! Mem's documented v2 notes REST API, never Mem It or semantic-search absence.
use super::*;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, Method, Url,
};
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;

const MAX_RESPONSE: usize = 1024 * 1024;
pub struct MemDestination {
    client: Client,
    origin: Url,
}

impl MemDestination {
    pub fn new(token: &str) -> Result<Self> {
        Self::build("https://api.mem.ai/", token, false)
    }

    /// Explicit local real-HTTP fixture surface; production never accepts an origin override.
    pub fn loopback_fixture(origin: &str, token: &str) -> Result<Self> {
        Self::build(origin, token, true)
    }

    fn build(origin: &str, token: &str, fixture: bool) -> Result<Self> {
        let url = Url::parse(origin).map_err(|_| Failure::InvalidInput)?;
        if fixture
            && (url.scheme() != "http"
                || url.host_str() != Some("127.0.0.1")
                || url.port().is_none()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
                || !url.username().is_empty()
                || url.password().is_some())
        {
            return Err(Failure::InvalidInput);
        }
        if token.is_empty() || token.len() > 4096 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Failure::MissingCredentials);
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Failure::MissingCredentials)?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth);
        let mut builder = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10));
        if fixture {
            builder = builder.no_proxy();
        }
        Ok(Self {
            client: builder.build().map_err(|_| Failure::Transport)?,
            origin: url,
        })
    }

    fn note_path(id: &str) -> Result<String> {
        let uuid = Uuid::parse_str(id).map_err(|_| Failure::InvalidInput)?;
        if uuid.is_nil() || uuid.to_string() != id {
            return Err(Failure::InvalidInput);
        }
        Ok(format!("v2/notes/{id}"))
    }
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        payload: Option<serde_json::Value>,
        missing: bool,
        budget: Budget,
    ) -> Result<Option<T>> {
        let mut request = self
            .client
            .request(method, url)
            .timeout(budget.request_timeout()?);
        if let Some(payload) = payload {
            request = request.json(&payload);
        }
        let mut response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                Failure::Deadline
            } else {
                Failure::Transport
            }
        })?;
        if missing && response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 | 403 => Failure::Authentication,
                429 => Failure::RateLimited,
                409 | 412 => Failure::Conflict,
                400 | 422 => Failure::InvalidInput,
                _ => Failure::Transport,
            });
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err(Failure::InvalidResponse);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Transport)? {
            budget.remaining()?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err(Failure::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        budget.remaining()?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| Failure::InvalidResponse)
    }
}

#[derive(Deserialize)]
struct WireNote {
    id: String,
    title: String,
    content: String,
    version: u64,
    #[serde(default)]
    trashed_at: Option<String>,
}
impl WireNote {
    fn checked(self) -> Result<RemoteNote> {
        MemDestination::note_path(&self.id).map_err(|_| Failure::InvalidResponse)?;
        if self.version == 0 || self.content.len() > MAX_CONTENT {
            return Err(Failure::InvalidResponse);
        }
        Ok(RemoteNote {
            id: self.id,
            title: self.title,
            content: self.content,
            version: self.version,
            trashed: self.trashed_at.is_some(),
            metadata: BTreeMap::new(),
        })
    }
}
#[derive(Deserialize)]
struct Row {
    id: String,
    title: String,
}
#[derive(Deserialize)]
struct Page {
    results: Vec<Row>,
    total: usize,
    next_page: Option<String>,
}

impl NoteDestination for MemDestination {
    fn backend(&self) -> &str {
        "mem"
    }
    fn correlation_location(&self) -> CorrelationLocation {
        CorrelationLocation::Title
    }

    async fn discover(&self, correlation: Uuid, budget: Budget) -> Result<Discovery> {
        let mut cursor: Option<String> = None;
        let mut cursors = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut expected_total = None;
        let mut matches = Vec::new();
        for _ in 0..20 {
            let mut url = self
                .origin
                .join("v2/notes")
                .map_err(|_| Failure::InvalidInput)?;
            {
                let mut query = url.query_pairs_mut();
                query
                    .append_pair("limit", "100")
                    .append_pair("order_by", "created_at");
                if let Some(cursor) = &cursor {
                    query.append_pair("page", cursor);
                }
            }
            let page: Page = self
                .request(Method::GET, url, None, false, budget)
                .await?
                .ok_or(Failure::InvalidResponse)?;
            if page.results.len() > 100
                || page.total > 2000
                || expected_total.is_some_and(|n| n != page.total)
            {
                return Ok(Discovery::Incomplete);
            }
            expected_total = Some(page.total);
            for row in page.results {
                Self::note_path(&row.id).map_err(|_| Failure::InvalidResponse)?;
                if !ids.insert(row.id.clone()) {
                    return Ok(Discovery::Incomplete);
                }
                if title_match(&row.title, correlation)? {
                    matches.push(row.id);
                    if matches.len() > 1 {
                        return Ok(Discovery::Duplicate);
                    }
                }
            }
            match page.next_page {
                None => {
                    if ids.len() != page.total {
                        return Ok(Discovery::Incomplete);
                    }
                    return match matches.pop() {
                        None => Ok(Discovery::Missing),
                        Some(id) => match self.read(&id, budget).await? {
                            Some(note)
                                if title_match(&note.title, correlation)? && !note.trashed =>
                            {
                                Ok(Discovery::Unique(note))
                            }
                            _ => Err(Failure::Conflict),
                        },
                    };
                }
                Some(next) => {
                    if next.is_empty() || next.len() > 4096 || !cursors.insert(next.clone()) {
                        return Ok(Discovery::Incomplete);
                    }
                    cursor = Some(next);
                }
            }
        }
        Ok(Discovery::Incomplete)
    }

    async fn read(&self, id: &str, budget: Budget) -> Result<Option<RemoteNote>> {
        let path = Self::note_path(id)?;
        let url = self.origin.join(&path).map_err(|_| Failure::InvalidInput)?;
        let note: Option<WireNote> = self.request(Method::GET, url, None, true, budget).await?;
        let note = note.map(WireNote::checked).transpose()?;
        if note.as_ref().is_some_and(|note| note.id != id) {
            return Err(Failure::InvalidResponse);
        }
        Ok(note)
    }
    async fn create(
        &self,
        id: &str,
        content: &str,
        _correlation: Uuid,
        budget: Budget,
    ) -> Result<RemoteNote> {
        Self::note_path(id)?;
        if content.len() > MAX_CONTENT {
            return Err(Failure::InvalidInput);
        }
        let url = self
            .origin
            .join("v2/notes")
            .map_err(|_| Failure::InvalidInput)?;
        let wire: WireNote = self
            .request(
                Method::POST,
                url,
                Some(serde_json::json!({"id":id,"content":content})),
                false,
                budget,
            )
            .await?
            .ok_or(Failure::InvalidResponse)?;
        let note = wire.checked()?;
        if note.id != id {
            return Err(Failure::InvalidResponse);
        }
        Ok(note)
    }
    async fn update(
        &self,
        id: &str,
        content: &str,
        version: u64,
        _correlation: Uuid,
        budget: Budget,
    ) -> Result<RemoteNote> {
        if content.len() > MAX_CONTENT || version == 0 {
            return Err(Failure::InvalidInput);
        }
        let url = self
            .origin
            .join(&Self::note_path(id)?)
            .map_err(|_| Failure::InvalidInput)?;
        let wire: WireNote = self
            .request(
                Method::PATCH,
                url,
                Some(serde_json::json!({"content":content,"version":version})),
                false,
                budget,
            )
            .await?
            .ok_or(Failure::InvalidResponse)?;
        let note = wire.checked()?;
        if note.id != id {
            return Err(Failure::InvalidResponse);
        }
        Ok(note)
    }
}
