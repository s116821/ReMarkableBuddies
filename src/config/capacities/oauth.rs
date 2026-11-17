//! Fixed HTTPS OAuth transport. Test origins are constructor-only loopback.
use super::{lifecycle::Exchange, private_text, Grant};
use crate::{config::inspection::strict_json, note_export::Failure, storage::Uuid};
use reqwest::{header, Client, Response, Url};
use serde::{de::DeserializeOwned, Deserialize};
use std::{
    collections::BTreeSet,
    future::Future,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

type Result<T> = std::result::Result<T, Failure>;
const RESOURCE: &str = "https://api.capacities.io";
const REDIRECT: &str = "http://127.0.0.1:48761/oauth/capacities/callback";
const VERIFIED_CLIENT_ID: Option<&str> = None;
const SCOPES: [&str; 3] = ["api:read", "api:write", "offline_access"];
const MAX_RESPONSE: usize = 32 * 1024;
const REQUEST_BOUND: Duration = Duration::from_secs(10);

pub(super) struct Registration {
    pub client_id: String,
    origin: Url,
}

impl Registration {
    pub(super) fn production() -> Option<Self> {
        let client_id = VERIFIED_CLIENT_ID?.to_owned();
        Some(Self {
            client_id,
            origin: Url::parse(RESOURCE).ok()?,
        })
    }

    #[cfg(test)]
    pub(super) fn loopback(origin: &str) -> Self {
        let origin = Url::parse(origin).unwrap();
        assert!(
            origin.scheme() == "http"
                && origin.host_str() == Some("127.0.0.1")
                && origin.port().is_some()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none()
                && origin.username().is_empty()
                && origin.password().is_none()
        );
        Self {
            client_id: "fixture-public-client".to_owned(),
            origin,
        }
    }

    fn endpoint(&self, fixed_path: &str) -> Result<Url> {
        self.origin
            .join(fixed_path)
            .map_err(|_| Failure::Unsupported)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    code: String,
    verifier: String,
    client_id: String,
    redirect_uri: String,
    resource: String,
}

impl Proof {
    pub(super) fn valid(&self, registration: &Registration) -> bool {
        self.client_id == registration.client_id
            && private_text(&self.client_id, 256)
            && private_text(&self.code, 4096)
            && (43..=128).contains(&self.verifier.len())
            && self
                .verifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
            && self.redirect_uri == REDIRECT
            && self.resource == RESOURCE
    }
}

pub(super) struct Budget {
    deadline: Instant,
    canceled: watch::Receiver<bool>,
}

impl Budget {
    pub(super) fn new(deadline: Instant, canceled: watch::Receiver<bool>) -> Self {
        Self { deadline, canceled }
    }

    pub(super) fn current(&self) -> Result<Duration> {
        if *self.canceled.borrow() {
            return Err(Failure::Deadline);
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(Failure::Deadline)
    }

    async fn request<T>(&self, request: impl Future<Output = Result<T>>) -> Result<T> {
        let remaining = self.current()?.min(REQUEST_BOUND);
        let mut canceled = self.canceled.clone();
        tokio::select! {
            biased;
            _ = canceled.wait_for(|value| *value) => Err(Failure::Deadline),
            result = tokio::time::timeout(remaining, request) =>
                result.map_err(|_| Failure::Deadline)?,
        }
    }
}

#[derive(Deserialize)]
struct Metadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    code_challenge_methods_supported: Vec<String>,
    grant_types_supported: Vec<String>,
    response_types_supported: Vec<String>,
    token_endpoint_auth_methods_supported: Vec<String>,
    scopes_supported: Vec<String>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    scope: String,
    token_type: String,
}

#[derive(Deserialize)]
struct Space {
    id: Uuid,
}

pub(super) struct OAuth<'a> {
    client: Client,
    registration: &'a Registration,
}

impl<'a> OAuth<'a> {
    pub(super) fn new(registration: &'a Registration) -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .timeout(REQUEST_BOUND)
            .build()
            .map_err(|_| Failure::Transport)?;
        Ok(Self {
            client,
            registration,
        })
    }

    pub(super) async fn verify_metadata(&self, budget: &Budget) -> Result<()> {
        let url = self
            .registration
            .endpoint("/.well-known/oauth-authorization-server")?;
        let metadata: Metadata = budget
            .request(async {
                decode(
                    self.client
                        .get(url)
                        .header(header::ACCEPT, "application/json")
                        .send()
                        .await
                        .map_err(map_transport)?,
                )
                .await
            })
            .await?;
        let advertised = |list: &[String], expected| list.iter().any(|value| value == expected);
        if Url::parse(&metadata.issuer).ok().as_ref() != Some(&self.registration.origin)
            || metadata.authorization_endpoint
                != self.registration.endpoint("/oauth/authorize")?.as_str()
            || metadata.token_endpoint != self.registration.endpoint("/oauth/token")?.as_str()
            || !advertised(&metadata.code_challenge_methods_supported, "S256")
            || !advertised(&metadata.grant_types_supported, "authorization_code")
            || !advertised(&metadata.response_types_supported, "code")
            || !advertised(&metadata.token_endpoint_auth_methods_supported, "none")
            || !SCOPES
                .into_iter()
                .all(|scope| advertised(&metadata.scopes_supported, scope))
        {
            return Err(Failure::Unsupported);
        }
        Ok(())
    }

    pub(super) async fn redeem(
        &self,
        proof: &Proof,
        permit: &mut Exchange,
        id: Uuid,
        budget: &Budget,
        current_config: impl FnOnce() -> Result<()>,
    ) -> Result<Grant> {
        let form = form_body([
            ("grant_type", "authorization_code"),
            ("code", proof.code.as_str()),
            ("redirect_uri", proof.redirect_uri.as_str()),
            ("client_id", proof.client_id.as_str()),
            ("code_verifier", proof.verifier.as_str()),
        ]);
        let request = self
            .client
            .post(self.registration.endpoint("/oauth/token")?)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form)
            .build()
            .map_err(|_| Failure::InvalidInput)?;
        let tokens: Tokens = budget
            .request(async {
                budget.current()?;
                permit.authorize_dispatch()?;
                current_config()?;
                budget.current()?;
                decode(self.client.execute(request).await.map_err(map_transport)?).await
            })
            .await?;

        let scopes: Vec<_> = tokens.scope.split(' ').map(str::to_owned).collect();
        let actual: BTreeSet<_> = scopes.iter().map(String::as_str).collect();
        if tokens.token_type != "Bearer"
            || actual != SCOPES.into_iter().collect::<BTreeSet<_>>()
            || actual.len() != scopes.len()
            || !(1..=3600).contains(&tokens.expires_in)
            || !private_text(&tokens.access_token, 4096)
            || tokens.access_token.starts_with("cap-api-")
            || !private_text(&tokens.refresh_token, 4096)
        {
            return Err(Failure::Authentication);
        }
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Failure::Authentication)?
            .as_secs()
            .checked_add(tokens.expires_in)
            .ok_or(Failure::Authentication)?;
        let mut auth = header::HeaderValue::from_str(&format!("Bearer {}", tokens.access_token))
            .map_err(|_| Failure::Authentication)?;
        auth.set_sensitive(true);
        let url = self.registration.endpoint("/space")?;
        let space: Space = budget
            .request(async {
                decode(
                    self.client
                        .get(url)
                        .header(header::AUTHORIZATION, auth)
                        .send()
                        .await
                        .map_err(map_transport)?,
                )
                .await
            })
            .await?;
        let grant = Grant {
            schema_version: 1,
            grant_id: id,
            space_id: space.id,
            client_id: self.registration.client_id.clone(),
            resource: RESOURCE.to_owned(),
            scopes,
            expires_at,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
        };
        grant.validate(id)?;
        Ok(grant)
    }
}

fn form_body<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    // Reuse the URL crate's standard form encoding without adding a dependency.
    // Only the encoded bytes become the POST body, never the HTTP request URL.
    let mut encoder = Url::parse(RESOURCE).expect("fixed Capacities resource");
    encoder.query_pairs_mut().extend_pairs(pairs);
    encoder.query().unwrap_or_default().to_owned()
}

fn map_transport(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Deadline
    } else {
        Failure::Transport
    }
}

async fn decode<T: DeserializeOwned>(mut response: Response) -> Result<T> {
    if response.status() != reqwest::StatusCode::OK {
        return Err(Failure::Authentication);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err(Failure::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_transport)? {
        if chunk.len() > MAX_RESPONSE - bytes.len() {
            return Err(Failure::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = strict_json(&bytes).map_err(|_| Failure::InvalidResponse)?;
    if !value.is_object() {
        return Err(Failure::InvalidResponse);
    }
    serde_json::from_value(value).map_err(|_| Failure::InvalidResponse)
}
