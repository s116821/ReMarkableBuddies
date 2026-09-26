//! Drive v3 wire adapter. No authorization UI, account discovery plugin or service API.
use super::{files, types::*};
use crate::config::read_secret_file;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const APP_SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
const CHUNK: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub credential_version: u32,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub scopes: Vec<String>,
    pub account_permission_id: String,
    pub collection: Uuid,
}
impl Credentials {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.credential_version == 1
                && !self.access_token.trim().is_empty()
                && !self.client_id.trim().is_empty(),
            "invalid Drive credentials"
        );
        ensure!(
            self.scopes.iter().any(|s| s == APP_SCOPE)
                && !self.account_permission_id.is_empty()
                && !self.collection.is_nil(),
            "Drive grant binding missing"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tag {
    pub collection: Uuid,
    pub kind: String,
    pub sha256: String,
}
impl Tag {
    fn properties(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("buddy_protocol".into(), "1".into()),
            ("collection".into(), self.collection.to_string()),
            ("kind".into(), self.kind.clone()),
            ("sha256".into(), self.sha256.clone()),
        ])
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.collection.is_nil()
                && ["object", "manifest"].contains(&self.kind.as_str())
                && valid_digest(&self.sha256),
            "invalid remote identity"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFile {
    pub id: String,
    #[serde(default)]
    pub app_properties: BTreeMap<String, String>,
    #[serde(default)]
    pub trashed: bool,
}
impl RemoteFile {
    pub fn tag(&self) -> Result<Tag> {
        ensure!(
            self.app_properties
                .get("buddy_protocol")
                .map(String::as_str)
                == Some("1"),
            "unsupported remote protocol"
        );
        let get = |key: &str| {
            self.app_properties
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing remote identity"))
        };
        let tag = Tag {
            collection: get("collection")?
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid remote collection"))?,
            kind: get("kind")?,
            sha256: get("sha256")?,
        };
        tag.validate()?;
        Ok(tag)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub file_id: String,
    #[serde(default)]
    pub removed: bool,
    pub file: Option<RemoteFile>,
}
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
    pub checkpoint: Option<String>,
}

// Deliberately no Debug: an upload location is a bearer-like private URL.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct UploadSession {
    pub uri: Option<String>,
    pub offset: u64,
}
pub enum UploadProgress {
    Complete,
    Pending,
    Expired,
}

#[derive(Debug)]
pub struct TransportFailure {
    pub status: u16,
    pub retry_after_seconds: Option<u64>,
}
impl std::fmt::Display for TransportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Drive transport status {}", self.status)
    }
}
impl std::error::Error for TransportFailure {}

pub trait DriveTransport: Send {
    fn authorize(&mut self, collection: Uuid) -> Result<()>;
    fn start_token(&mut self) -> Result<String>;
    fn list(&mut self, page: Option<&str>) -> Result<Page<RemoteFile>>;
    fn changes(&mut self, page: &str) -> Result<Page<Change>>;
    fn download(&mut self, id: &str, limit: u64) -> Result<Vec<u8>>;
    fn allocate(&mut self) -> Result<String>;
    fn upload(
        &mut self,
        id: &str,
        tag: &Tag,
        bytes: &[u8],
        session: &mut UploadSession,
    ) -> Result<UploadProgress>;
}

pub struct GoogleDrive {
    agent: ureq::Agent,
    credential_path: PathBuf,
    credentials: Credentials,
    api: String,
    upload_api: String,
    token_url: String,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid Drive response"))
}
fn id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
        "invalid Drive file ID"
    );
    Ok(())
}
fn json_response(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    check(&response)?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_METADATA as u64)
        .read_to_vec()
        .map_err(|_| anyhow::anyhow!("Drive response read failed"))?;
    parse(&bytes)
}
fn check(response: &ureq::http::Response<ureq::Body>) -> Result<()> {
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(TransportFailure {
            status,
            retry_after_seconds: response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .map(|s: u64| s.min(3600)),
        }
        .into());
    }
    Ok(())
}
impl GoogleDrive {
    pub fn open(credential_path: PathBuf) -> Result<Self> {
        let credentials: Credentials = parse(&read_secret_file(&credential_path)?)?;
        credentials.validate()?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .new_agent();
        Ok(Self {
            agent,
            credential_path,
            credentials,
            api: "https://www.googleapis.com/drive/v3".into(),
            upload_api: "https://www.googleapis.com/upload/drive/v3".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
        })
    }
    fn token(&mut self) -> Result<String> {
        if self.credentials.expires_at <= now() + 60 {
            ensure!(
                !self.credentials.refresh_token.is_empty(),
                "Drive authorization refresh required"
            );
            let mut fields = vec![
                ("client_id", self.credentials.client_id.as_str()),
                ("refresh_token", self.credentials.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ];
            if let Some(secret) = &self.credentials.client_secret {
                fields.push(("client_secret", secret));
            }
            let response = self
                .agent
                .post(&self.token_url)
                .send_form(fields)
                .map_err(|_| anyhow::anyhow!("Drive token transport failed"))?;
            let value = json_response(response)?;
            let token = value
                .get("access_token")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Drive token response missing credential"))?;
            if let Some(scopes) = value.get("scope").and_then(Value::as_str) {
                ensure!(
                    scopes.split_whitespace().any(|s| s == APP_SCOPE),
                    "Drive grant lacks required scope"
                );
            }
            self.credentials.access_token = token.into();
            self.credentials.expires_at = now()
                + value
                    .get("expires_in")
                    .and_then(Value::as_u64)
                    .filter(|v| *v <= 86400)
                    .ok_or_else(|| anyhow::anyhow!("invalid Drive token expiry"))?;
            if let Some(refresh) = value.get("refresh_token").and_then(Value::as_str) {
                self.credentials.refresh_token = refresh.into();
            }
            files::atomic_json(&self.credential_path, &self.credentials)?;
        }
        Ok(format!("Bearer {}", self.credentials.access_token))
    }
    fn metadata(&mut self, id_value: &str) -> Result<RemoteFile> {
        id(id_value)?;
        let token = self.token()?;
        let value = json_response(
            self.agent
                .get(format!("{}/files/{}", self.api, id_value))
                .query("fields", "id,appProperties,trashed")
                .header("Authorization", token)
                .call()
                .map_err(|_| anyhow::anyhow!("Drive metadata transport failed"))?,
        )?;
        serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid Drive metadata"))
    }
    fn verify_existing(&mut self, id: &str, tag: &Tag, bytes: &[u8]) -> Result<()> {
        let existing = self.metadata(id)?;
        ensure!(
            !existing.trashed && existing.tag()? == *tag,
            "Drive allocated identity collision"
        );
        ensure!(
            self.download(id, bytes.len() as u64)? == bytes,
            "Drive existing content collision"
        );
        Ok(())
    }
    fn safe_session(&self, uri: &str) -> Result<()> {
        let parsed: ureq::http::Uri = uri
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid private upload location"))?;
        ensure!(
            parsed.scheme_str() == Some("https")
                && parsed.host() == Some("www.googleapis.com")
                && parsed.port_u16().is_none()
                && parsed.path().starts_with("/upload/drive/v3/"),
            "untrusted upload location refused"
        );
        Ok(())
    }
}
impl DriveTransport for GoogleDrive {
    fn authorize(&mut self, collection: Uuid) -> Result<()> {
        ensure!(
            collection == self.credentials.collection,
            "Drive collection binding mismatch"
        );
        let token = self.token()?;
        let value = json_response(
            self.agent
                .get(format!("{}/about", self.api))
                .query("fields", "user(permissionId)")
                .header("Authorization", token)
                .call()
                .map_err(|_| anyhow::anyhow!("Drive binding check failed"))?,
        )?;
        ensure!(
            value.pointer("/user/permissionId").and_then(Value::as_str)
                == Some(self.credentials.account_permission_id.as_str()),
            "Drive account binding mismatch"
        );
        Ok(())
    }
    fn start_token(&mut self) -> Result<String> {
        let token = self.token()?;
        let value = json_response(
            self.agent
                .get(format!("{}/changes/startPageToken", self.api))
                .header("Authorization", token)
                .call()
                .map_err(|_| anyhow::anyhow!("Drive token discovery failed"))?,
        )?;
        value["startPageToken"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("missing change token"))
    }
    fn list(&mut self, page: Option<&str>) -> Result<Page<RemoteFile>> {
        let token = self.token()?;
        let mut request = self
            .agent
            .get(format!("{}/files", self.api))
            .query("spaces", "appDataFolder")
            .query("pageSize", "100")
            .query(
                "fields",
                "files(id,appProperties,trashed),nextPageToken,incompleteSearch",
            )
            .header("Authorization", token);
        if let Some(page) = page {
            request = request.query("pageToken", page);
        }
        let value = json_response(
            request
                .call()
                .map_err(|_| anyhow::anyhow!("Drive listing failed"))?,
        )?;
        ensure!(
            value.get("incompleteSearch").and_then(Value::as_bool) != Some(true),
            "Drive listing incomplete"
        );
        let items = serde_json::from_value(value["files"].clone())
            .map_err(|_| anyhow::anyhow!("invalid Drive file page"))?;
        Ok(Page {
            items,
            next: value["nextPageToken"].as_str().map(str::to_owned),
            checkpoint: None,
        })
    }
    fn changes(&mut self, page: &str) -> Result<Page<Change>> {
        let token = self.token()?;
        let value=json_response(self.agent.get(format!("{}/changes",self.api)).query("spaces","appDataFolder").query("pageToken",page).query("includeRemoved","true").query("pageSize","100").query("fields","changes(fileId,removed,file(id,appProperties,trashed)),nextPageToken,newStartPageToken").header("Authorization",token).call().map_err(|_|anyhow::anyhow!("Drive changes failed"))?)?;
        Ok(Page {
            items: serde_json::from_value(value["changes"].clone())
                .map_err(|_| anyhow::anyhow!("invalid Drive change page"))?,
            next: value["nextPageToken"].as_str().map(str::to_owned),
            checkpoint: value["newStartPageToken"].as_str().map(str::to_owned),
        })
    }
    fn download(&mut self, id_value: &str, limit: u64) -> Result<Vec<u8>> {
        id(id_value)?;
        ensure!(limit <= MAX_MEDIA, "Drive download exceeds supported bound");
        let token = self.token()?;
        let mut response = self
            .agent
            .get(format!("{}/files/{}", self.api, id_value))
            .query("alt", "media")
            .header("Authorization", token)
            .call()
            .map_err(|_| anyhow::anyhow!("Drive download failed"))?;
        check(&response)?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|_| anyhow::anyhow!("Drive bounded download failed"))
    }
    fn allocate(&mut self) -> Result<String> {
        let token = self.token()?;
        let value = json_response(
            self.agent
                .get(format!("{}/files/generateIds", self.api))
                .query("count", "1")
                .query("space", "appDataFolder")
                .query("type", "files")
                .header("Authorization", token)
                .call()
                .map_err(|_| anyhow::anyhow!("Drive ID allocation failed"))?,
        )?;
        let value = value["ids"][0]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing allocated ID"))?
            .to_owned();
        id(&value)?;
        Ok(value)
    }
    fn upload(
        &mut self,
        id_value: &str,
        tag: &Tag,
        bytes: &[u8],
        session: &mut UploadSession,
    ) -> Result<UploadProgress> {
        id(id_value)?;
        tag.validate()?;
        ensure!(
            digest(bytes) == tag.sha256 && bytes.len() as u64 <= MAX_MEDIA,
            "upload identity mismatch"
        );
        let token = self.token()?;
        let metadata = json!({"id":id_value,"name":tag.sha256,"parents":["appDataFolder"],"appProperties":tag.properties()});
        if bytes.len() <= CHUNK {
            let boundary = format!("buddy-{}", Uuid::new_v4());
            let mut body=format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
            body.extend_from_slice(bytes);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            let response = self
                .agent
                .post(format!("{}/files", self.upload_api))
                .query("uploadType", "multipart")
                .header("Authorization", token)
                .header(
                    "Content-Type",
                    format!("multipart/related; boundary={boundary}"),
                )
                .send(body)
                .map_err(|_| anyhow::anyhow!("Drive upload outcome unknown"))?;
            if response.status().as_u16() != 409 {
                check(&response)?;
            }
            self.verify_existing(id_value, tag, bytes)?;
            return Ok(UploadProgress::Complete);
        }
        if session.uri.is_none() {
            let response = self
                .agent
                .post(format!("{}/files", self.upload_api))
                .query("uploadType", "resumable")
                .header("Authorization", token)
                .header("X-Upload-Content-Type", "application/octet-stream")
                .header("X-Upload-Content-Length", bytes.len().to_string())
                .send_json(metadata)
                .map_err(|_| anyhow::anyhow!("Drive session creation outcome unknown"))?;
            if response.status().as_u16() == 409 {
                self.verify_existing(id_value, tag, bytes)?;
                return Ok(UploadProgress::Complete);
            }
            check(&response)?;
            let uri = response
                .headers()
                .get("location")
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("missing private upload location"))?
                .to_owned();
            self.safe_session(&uri)?;
            session.uri = Some(uri);
            session.offset = 0;
            return Ok(UploadProgress::Pending);
        }
        let uri = session.uri.as_ref().unwrap();
        self.safe_session(uri)?;
        let response = self
            .agent
            .put(uri)
            .header("Authorization", &token)
            .header("Content-Range", format!("bytes */{}", bytes.len()))
            .send_empty()
            .map_err(|_| anyhow::anyhow!("Drive upload status failed"))?;
        match response.status().as_u16() {
            200 | 201 => {
                self.verify_existing(id_value, tag, bytes)?;
                return Ok(UploadProgress::Complete);
            }
            404 => {
                *session = UploadSession::default();
                return Ok(UploadProgress::Expired);
            }
            308 => (),
            _ => check(&response)?,
        }
        let offset = match response.headers().get("range") {
            None => 0,
            Some(value) => value
                .to_str()
                .ok()
                .and_then(|s| s.strip_prefix("bytes=0-"))
                .and_then(|s| s.parse::<usize>().ok())
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| anyhow::anyhow!("invalid upload offset"))?,
        };
        ensure!(offset < bytes.len(), "invalid upload progress");
        let end = (offset + CHUNK).min(bytes.len());
        let response = self
            .agent
            .put(uri)
            .header("Authorization", token)
            .header(
                "Content-Range",
                format!("bytes {}-{}/{}", offset, end - 1, bytes.len()),
            )
            .send(&bytes[offset..end])
            .map_err(|_| anyhow::anyhow!("Drive chunk outcome unknown"))?;
        match response.status().as_u16() {
            200 | 201 => {
                self.verify_existing(id_value, tag, bytes)?;
                Ok(UploadProgress::Complete)
            }
            308 => {
                session.offset = end as u64;
                Ok(UploadProgress::Pending)
            }
            404 => {
                *session = UploadSession::default();
                Ok(UploadProgress::Expired)
            }
            _ => {
                check(&response)?;
                anyhow::bail!("unexpected upload response")
            }
        }
    }
}
