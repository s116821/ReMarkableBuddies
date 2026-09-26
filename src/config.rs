//! Nonsecret file settings. Credentials are selected through protected references.
use crate::{
    llm::openai::DEFAULT_MODEL,
    storage::{files, Namespace, StorePaths, Uuid, MAX_MEDIA, MAX_RECORD},
    TriggerCorner,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncPolicy {
    pub enabled: bool,
    pub namespaces: BTreeSet<Namespace>,
    pub media: bool,
    pub max_media_bytes: u64,
    pub poll_seconds: u64,
    pub batch_items: usize,
    pub collection: Option<Uuid>,
    pub create_new: bool,
}
impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            namespaces: BTreeSet::new(),
            media: false,
            max_media_bytes: 32 * 1024 * 1024,
            poll_seconds: 60,
            batch_items: 32,
            collection: None,
            create_new: false,
        }
    }
}
impl SyncPolicy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (5..=3600).contains(&self.poll_seconds) && (1..=128).contains(&self.batch_items),
            "invalid sync work limits"
        );
        ensure!(self.max_media_bytes <= MAX_MEDIA, "invalid media limit");
        ensure!(
            self.collection.is_none_or(|c| !c.is_nil()),
            "invalid collection binding"
        );
        ensure!(
            !self.create_new || self.collection.is_some(),
            "new collection needs explicit identity"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub config_schema_version: u32,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub trigger_corner: Option<String>,
    pub log_level: Option<String>,
    pub debug_dump: Option<bool>,
    pub paths: StorePaths,
    pub model_credential: Option<String>,
    pub sync: SyncPolicy,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            config_schema_version: 1,
            model: None,
            base_url: None,
            trigger_corner: None,
            log_level: None,
            debug_dump: None,
            paths: StorePaths::default(),
            model_credential: None,
            sync: SyncPolicy::default(),
        }
    }
}
impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let explicit = path.is_some();
        let path =
            path.unwrap_or_else(|| "/home/root/.config/remarkable-buddies/config.json".into());
        let config = if path.exists() {
            files::json(&path, MAX_RECORD as u64)?
        } else {
            ensure!(!explicit, "selected configuration is missing");
            Self::default()
        };
        Self::validate(&config)?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.config_schema_version == 1,
            "unsupported configuration version"
        );
        self.paths.validate()?;
        self.sync.validate()?;
        if let Some(model) = &self.model {
            ensure!(
                !model.trim().is_empty() && model.len() <= 256,
                "invalid model setting"
            );
        }
        if let Some(corner) = &self.trigger_corner {
            ensure!(
                ["UR", "UL", "LR", "LL"].contains(&corner.as_str()),
                "invalid trigger corner"
            );
        }
        if let Some(level) = &self.log_level {
            level
                .parse::<log::LevelFilter>()
                .map_err(|_| anyhow::anyhow!("invalid log level"))?;
        }
        if let Some(endpoint) = &self.base_url {
            ensure!(
                !endpoint.trim().is_empty() && endpoint.len() <= 2048,
                "invalid model endpoint"
            );
        }
        if let Some(name) = &self.model_credential {
            credential_name(name)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Overrides {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub trigger_corner: Option<String>,
    pub log_level: Option<log::LevelFilter>,
    pub debug_dump: bool,
}
#[derive(Default)]
pub struct Environment {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub log_filter: Option<String>,
    pub debug_dump: Option<String>,
}
impl Environment {
    pub fn current() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            base_url: std::env::var("OPENAI_BASE_URL").ok(),
            log_filter: std::env::var("RUST_LOG").ok(),
            debug_dump: std::env::var("READER_BUDDY_DEBUG_DUMP").ok(),
        }
    }
}
pub struct Effective {
    pub key: Secret,
    pub model: String,
    pub base_url: Option<String>,
    pub trigger_corner: TriggerCorner,
    pub corner_name: String,
    pub log_level: Option<log::LevelFilter>,
    pub log_filter: Option<String>,
    pub debug_dump: bool,
    pub sources: BTreeMap<String, String>,
}
pub struct Secret(String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}

fn credential_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 80
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid credential reference"
    );
    Ok(())
}
pub fn read_secret_file(path: &Path) -> Result<Vec<u8>> {
    files::safe_path(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(
            metadata.mode() & 0o077 == 0,
            "insecure credential permissions"
        );
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing credential directory"))?;
        ensure!(
            std::fs::metadata(parent)?.mode() & 0o077 == 0,
            "insecure credential directory"
        );
    }
    files::read(path, MAX_RECORD as u64)
}
pub fn resolve(config: &Config, cli: Overrides, env: Environment) -> Result<Effective> {
    config.validate()?;
    let mut sources = BTreeMap::new();
    let key = if let Some(key) = cli.api_key {
        sources.insert("api_key".into(), "cli".into());
        key
    } else if let Some(key) = env.api_key {
        sources.insert("api_key".into(), "environment".into());
        key
    } else if let Some(name) = &config.model_credential {
        sources.insert("api_key".into(), "protected-reference".into());
        String::from_utf8(read_secret_file(&config.paths.credentials.join(name))?)
            .map_err(|_| anyhow::anyhow!("invalid selected credential"))?
    } else {
        anyhow::bail!("Set OPENAI_API_KEY or supply --api-key or a protected credential reference")
    };
    ensure!(!key.trim().is_empty(), "selected API credential is empty");
    fn choose(
        name: &str,
        cli: Option<String>,
        env: Option<String>,
        file: Option<String>,
        default: Option<&str>,
        sources: &mut BTreeMap<String, String>,
    ) -> Option<String> {
        for (value, label) in [
            (cli, "cli"),
            (env, "environment"),
            (file, "file"),
            (default.map(str::to_owned), "default"),
        ] {
            if let Some(value) = value {
                sources.insert(name.into(), label.into());
                return Some(value);
            }
        }
        None
    }
    let model = choose(
        "model",
        cli.model,
        None,
        config.model.clone(),
        Some(DEFAULT_MODEL),
        &mut sources,
    )
    .unwrap();
    ensure!(!model.trim().is_empty(), "selected model is empty");
    let corner_name = choose(
        "trigger_corner",
        cli.trigger_corner,
        None,
        config.trigger_corner.clone(),
        Some("LL"),
        &mut sources,
    )
    .unwrap();
    let trigger_corner = TriggerCorner::from_string(&corner_name)
        .map_err(|_| anyhow::anyhow!("invalid selected trigger corner"))?;
    let base_url = choose(
        "base_url",
        cli.base_url,
        env.base_url,
        config.base_url.clone(),
        None,
        &mut sources,
    );
    let debug_dump = if cli.debug_dump {
        sources.insert("debug_dump".into(), "cli".into());
        true
    } else if let Some(value) = env.debug_dump {
        sources.insert("debug_dump".into(), "environment".into());
        match value.as_str() {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => anyhow::bail!("READER_BUDDY_DEBUG_DUMP must be true, false, 1 or 0"),
        }
    } else {
        sources.insert(
            "debug_dump".into(),
            if config.debug_dump.is_some() {
                "file"
            } else {
                "default"
            }
            .into(),
        );
        config.debug_dump.unwrap_or(false)
    };
    let (log_level, log_filter) = if let Some(level) = cli.log_level {
        sources.insert("logging".into(), "cli".into());
        (Some(level), None)
    } else if let Some(filter) = env.log_filter {
        sources.insert("logging".into(), "environment".into());
        (None, Some(filter))
    } else {
        sources.insert(
            "logging".into(),
            if config.log_level.is_some() {
                "file"
            } else {
                "default"
            }
            .into(),
        );
        (
            config
                .log_level
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid file log level"))?,
            None,
        )
    };
    Ok(Effective {
        key: Secret(key),
        model,
        base_url,
        trigger_corner,
        corner_name,
        log_level,
        log_filter,
        debug_dump,
        sources,
    })
}
