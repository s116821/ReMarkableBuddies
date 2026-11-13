//! Ordinary operation adapter; never reopens storage or admits native input.
use super::{ConfirmedContext, ContextFailure, ExampleSelection, LocalCorrections};
use crate::{config::Config, mode_maintenance::LocalDispatchLifetime, storage::Store};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) const POLICY: &str = "confirmed-recency-v1";
#[derive(serde::Serialize)]
pub(crate) struct Selection {
    pub selected: Vec<ExampleSelection>,
    pub scanned: u64,
    pub eligible: usize,
    pub checked: usize,
    pub skipped_media: usize,
    pub conflicted: u64,
    pub unsupported: u64,
    pub unexamined: usize,
    pub tombstones: u64,
    pub excluded_targets: usize,
}
pub(crate) struct Runtime {
    config: Config,
    store: Arc<Store>,
    local: Arc<LocalDispatchLifetime>,
}
pub(crate) struct Capture {
    pub context: Option<Arc<ConfirmedContext>>,
    pub selection: Option<Selection>,
    pub failure: Option<ContextFailure>,
    _refused_or_empty: Option<LocalCorrections>,
    _local: Arc<LocalDispatchLifetime>,
}
impl Runtime {
    pub(crate) fn new(
        config: &Config,
        store: Arc<Store>,
        local: Arc<LocalDispatchLifetime>,
    ) -> anyhow::Result<Option<Self>> {
        if !config.handwriting.enabled {
            return Ok(None);
        }
        config.validate_values()?;
        anyhow::ensure!(
            local.admits_store(&store),
            "handwriting receiving store changed"
        );
        Ok(Some(Self {
            config: config.clone(),
            store,
            local,
        }))
    }
    pub(crate) fn capture(
        &self,
        store: &Arc<Store>,
        targets: &BTreeSet<String>,
        progress: &mut dyn FnMut() -> Result<(), ContextFailure>,
    ) -> Capture {
        let mut result = Capture {
            context: None,
            selection: None,
            failure: None,
            _refused_or_empty: None,
            _local: self.local.clone(),
        };
        if !Arc::ptr_eq(store, &self.store) || !self.local.admits_store(store) {
            result.failure = Some(ContextFailure::Storage);
            return result;
        }
        if progress().is_err() {
            result.failure = Some(ContextFailure::Canceled);
            return result;
        }
        let owner = match LocalCorrections::new(store.clone(), &self.config) {
            Ok(owner) => owner,
            Err(_) => {
                result.failure = Some(ContextFailure::Storage);
                return result;
            }
        };
        let profile = self
            .config
            .handwriting
            .profile
            .expect("validated active profile");
        let selection = match owner.runtime_selection(profile, targets, progress) {
            Ok(selection) => selection,
            Err(failure) => {
                result.failure = Some(failure);
                result._refused_or_empty = Some(owner);
                return result;
            }
        };
        if selection.selected.is_empty() {
            result._refused_or_empty = Some(owner);
            result.selection = Some(selection);
            return result;
        }
        match owner.capture_context(profile, &selection.selected, progress) {
            Ok(context) => result.context = Some(Arc::new(context)),
            Err((failure, owner)) => {
                result.failure = Some(failure);
                result._refused_or_empty = Some(owner);
            }
        }
        result.selection = Some(selection);
        result
    }
}

/// Entire ordered content is bounded before the engine sees any example bytes.
pub(crate) fn reader_payload(
    prompt: &str,
    verification: bool,
    targets: &[String],
    context: &ConfirmedContext,
) -> anyhow::Result<Vec<serde_json::Value>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde_json::json;
    let addendum = if verification {
        include_str!("runtime-verification-instructions.txt")
    } else {
        include_str!("runtime-analysis-instructions.txt")
    };
    let mut parts = vec![
        json!({"type":"text","text":prompt}),
        json!({"type":"text","text":addendum}),
    ];
    let absent_original = context.has_absent_original();
    if absent_original {
        parts.push(json!({"type":"text","text":super::context::ABSENT_ORIGINAL_INSTRUCTION}));
    }
    for (i, e) in context.examples.iter().enumerate() {
        #[derive(serde::Serialize)]
        struct ExampleData<'a> {
            kind: &'static str,
            index: usize,
            original_text: Option<&'a str>,
            corrected_text: &'a str,
        }
        let descriptor = serde_json::to_string(&ExampleData {
            kind: "confirmed-example",
            index: i + 1,
            original_text: e.original_text.as_deref(),
            corrected_text: &e.corrected_text,
        })?;

        parts.push(json!({"type":"text","text":descriptor}));
        parts.push(json!({"type":"image_url","image_url":STANDARD.encode(&e.png)}));
    }
    parts.push(json!({"type":"text","text":"Target page images follow."}));
    for image in targets {
        parts.push(json!({"type":"image_url","image_url":image}));
    }
    let text = parts
        .iter()
        .filter(|p| p["type"] == "text")
        .collect::<Vec<_>>();
    anyhow::ensure!(
        serde_json::to_vec(&text)?.len() <= 32768
            && serde_json::to_vec(&parts)?.len() <= 24 * 1024 * 1024,
        "handwriting Reader payload exceeds bound"
    );
    let task = if absent_original && verification {
        "reader-verification-confirmed-v1-nullable-original-v1"
    } else if absent_original {
        "reader-analysis-confirmed-v1-nullable-original-v1"
    } else if verification {
        "reader-verification-confirmed-v1"
    } else {
        "reader-analysis-confirmed-v1"
    };
    log::debug!(
        "Handwriting payload task={} sha256={}",
        task,
        crate::storage::digest(&serde_json::to_vec(&parts)?)
    );
    Ok(parts)
}
pub(crate) fn send_parts(engine: &mut dyn crate::LLMEngine, parts: &[serde_json::Value]) {
    for part in parts {
        if let Some(text) = part["text"].as_str() {
            engine.add_text_content(text);
        } else {
            engine.add_image_content(part["image_url"].as_str().expect("typed image part"));
        }
    }
}

impl Capture {
    pub(crate) fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({"policy":POLICY,"selection":self.selection,"selection_status":self.selection.as_ref().map(|s|s.status()),"context":self.context.as_ref().map(|c|c.identity())})
    }
}

impl Selection {
    pub(crate) fn status(&self) -> &'static str {
        let partial = self.unexamined > 0
            || self.skipped_media > 0
            || self.conflicted > 0
            || self.unsupported > 0;
        match (partial, self.selected.is_empty()) {
            (false, true) => "complete-empty",
            (true, true) => "partial-empty",
            (false, false) => "complete",
            (true, false) => "partial",
        }
    }
}
