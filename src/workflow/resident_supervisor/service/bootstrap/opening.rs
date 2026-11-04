//! Explicit disposable-document activity metadata policy. All baseline bytes
//! come from the original backup manifest; this never writes the source tree.
use super::{setup::CapturePlan, *};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

const MAX_JSON: usize = 64 * 1024;

pub(super) struct Samples(Vec<(FileKind, Pinned, Vec<u8>)>);
impl Samples {
    pub(super) fn read(
        plan: &CapturePlan,
        source: &Path,
        evidence: &Path,
        label: &str,
        inventory: &[SourceEntry],
        deadline: Instant,
    ) -> Result<Self> {
        let mut samples = Vec::new();
        for kind in [FileKind::Content, FileKind::Metadata] {
            let path = kind.path(plan);
            let (pin, bytes) = read_pin(&source.join(&path), deadline)?;
            // Keep bounded rejected samples too, without writing source files.
            write_new(
                evidence,
                &format!("{label}.opening-{}", kind.extension()),
                &bytes,
            )?;
            ensure!(
                inventory
                    .iter()
                    .any(|e| e.path == path && e.sha256.as_deref() == Some(sha(&bytes).as_str())),
                "metadata changed after inventory"
            );
            samples.push((kind, pin, bytes));
        }
        Ok(Self(samples))
    }
    pub(super) fn verify(&self, policy: &OpeningMetadata, plan: &CapturePlan) -> Result<()> {
        for (_, pin, _) in &self.0 {
            pin.current()?;
        }
        for (kind, _, bytes) in &self.0 {
            policy.verify(plan, *kind, bytes)?;
        }
        for (_, pin, _) in &self.0 {
            pin.current()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpeningMetadata {
    pub content: String,
    pub metadata: String,
}

#[derive(Clone, Copy)]
pub(super) enum FileKind {
    Content,
    Metadata,
}
impl FileKind {
    pub(super) fn extension(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Metadata => "metadata",
        }
    }
    pub(super) fn path(self, plan: &CapturePlan) -> String {
        format!("{}.{}", plan.document, self.extension())
    }
}

impl OpeningMetadata {
    fn original(&self, kind: FileKind) -> &[u8] {
        match kind {
            FileKind::Content => self.content.as_bytes(),
            FileKind::Metadata => self.metadata.as_bytes(),
        }
    }
    pub(super) fn validate(&self, plan: &CapturePlan, entries: &[SourceEntry]) -> Result<()> {
        plan.facts()?;
        for kind in [FileKind::Content, FileKind::Metadata] {
            let path = kind.path(plan);
            let found: Vec<_> = entries.iter().filter(|e| e.path == path).collect();
            ensure!(
                found.len() == 1
                    && found[0].sha256.as_deref() == Some(sha(self.original(kind)).as_str()),
                "opening baseline is not the original manifest file"
            );
            self.verify(plan, kind, self.original(kind))?;
        }
        let content = parse(self.content.as_bytes())?;
        let pages = content
            .pointer("/cPages/pages")
            .and_then(Value::as_array)
            .context("opening baseline has no page array")?;
        let order: Vec<_> = pages
            .iter()
            .map(|page| page.get("id").and_then(Value::as_str))
            .collect();
        ensure!(
            order
                == plan
                    .order
                    .iter()
                    .map(|id| Some(id.as_str()))
                    .collect::<Vec<_>>()
                && content.get("pageCount").and_then(Value::as_u64)
                    == Some(plan.order.len() as u64),
            "opening baseline differs from selected full order"
        );
        Ok(())
    }
    pub(super) fn verify(&self, plan: &CapturePlan, kind: FileKind, current: &[u8]) -> Result<()> {
        let old = parse(self.original(kind))?;
        let new = parse(current)?;
        match kind {
            FileKind::Metadata => {
                let old_time = decimal(string(&old, "/lastOpened")?)?;
                let new_time = decimal(string(&new, "/lastOpened")?)?;
                let index = |value: &Value| {
                    value
                        .get("lastOpenedPage")
                        .and_then(Value::as_u64)
                        .filter(|i| *i < plan.order.len() as u64)
                        .context("invalid remembered page index")
                };
                let old_index = index(&old)?;
                let new_index = index(&new)?;
                let selected = plan
                    .order
                    .iter()
                    .position(|p| p == &plan.page)
                    .context("missing selected page")? as u64;
                ensure!(
                    (new_index == old_index || new_index == selected)
                        && new_time >= old_time
                        && (new_index == old_index || new_time > old_time),
                    "unpermitted remembered page/timestamp change"
                );
            }
            FileKind::Content => {
                let stamp = |value: &Value| -> Result<(u64, u64)> {
                    let (client, counter) = string(value, "/cPages/lastOpened/timestamp")?
                        .split_once(':')
                        .context("invalid register timestamp")?;
                    Ok((decimal(client)?, decimal(counter)?))
                };
                let old_stamp = stamp(&old)?;
                let new_stamp = stamp(&new)?;
                let old_page = string(&old, "/cPages/lastOpened/value")?;
                let new_page = string(&new, "/cPages/lastOpened/value")?;
                ensure!(
                    plan.order.iter().any(|id| id == old_page)
                        && (new_page == old_page || new_page == plan.page)
                        && new_stamp.0 == old_stamp.0
                        && new_stamp.1 >= old_stamp.1
                        && (new_page == old_page || new_stamp.1 > old_stamp.1),
                    "unpermitted remembered register change"
                );
            }
        }
        ensure!(
            protected(self.original(kind), kind)? == protected(current, kind)?,
            "protected metadata key/value/type changed"
        );
        Ok(())
    }
}

// Explicit development insertion only. Existing activity-only policy above is
// unchanged; all old page objects and unrelated bytes remain exact.
impl Samples {
    pub(super) fn verify_insertion(
        &self,
        policy: &OpeningMetadata,
        plan: &CapturePlan,
        target: &str,
        source: &Path,
        original: &[SourceEntry],
        current: &[SourceEntry],
        deadline: Instant,
    ) -> Result<()> {
        let added = [
            format!("{}/{target}.rm", plan.document),
            format!("{}.thumbnails/{target}.png", plan.document),
        ];
        ensure!(
            !plan.order.iter().any(|id| id == target) && plan.order.len() <= 16,
            "invalid insertion preservation target"
        );
        ensure!(
            current.len() == original.len() + 2,
            "insertion must add exactly two target files"
        );
        let old: BTreeMap<_, _> = original
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let now: BTreeMap<_, _> = current
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        ensure!(
            old.len() == original.len() && now.len() == current.len(),
            "duplicate inventory path"
        );
        let mutable = [FileKind::Content.path(plan), FileKind::Metadata.path(plan)];
        for (path, entry) in &old {
            let actual = now.get(path).context("original source path removed")?;
            if mutable.iter().any(|value| value.as_str() == *path) {
                ensure!(
                    entry.sha256.is_some() && actual.sha256.is_some(),
                    "document JSON type changed"
                );
            } else {
                ensure!(
                    *entry == *actual,
                    "protected original bytes changed: {path}"
                );
            }
        }
        ensure!(
            now.keys()
                .filter(|path| !old.contains_key(**path))
                .all(|path| added.iter().any(|value| value.as_str() == *path)),
            "unexpected inserted path"
        );
        let mut added_bytes = 0i128;
        let mut added_pins = Vec::new();
        for path in &added {
            let entry = now
                .get(path.as_str())
                .context("declared inserted file absent")?;
            let (pin, bytes) = read_pin(&source.join(path), deadline)?;
            ensure!(
                entry.sha256.as_deref() == Some(sha(&bytes).as_str()),
                "inserted bytes changed after inventory"
            );
            added_bytes += bytes.len() as i128;
            added_pins.push(pin);
        }
        let content_bytes = &self
            .0
            .iter()
            .find(|(kind, _, _)| matches!(kind, FileKind::Content))
            .context("missing content sample")?
            .2;
        let metadata_bytes = &self
            .0
            .iter()
            .find(|(kind, _, _)| matches!(kind, FileKind::Metadata))
            .context("missing metadata sample")?
            .2;
        let before_content = parse(policy.content.as_bytes())?;
        let after_content = parse(content_bytes)?;
        let before_metadata = parse(policy.metadata.as_bytes())?;
        let after_metadata = parse(metadata_bytes)?;
        let position = plan
            .order
            .iter()
            .position(|page| page == &plan.page)
            .context("source page absent")?;
        let mut order = plan.order.clone();
        order.insert(position + 1, target.into());
        let pages = after_content
            .pointer("/cPages/pages")
            .and_then(Value::as_array)
            .context("missing inserted page array")?;
        ensure!(
            pages
                .iter()
                .map(|page| page.get("id").and_then(Value::as_str))
                .eq(order.iter().map(|id| Some(id.as_str())))
                && after_content.get("pageCount").and_then(Value::as_u64)
                    == Some(order.len() as u64),
            "inserted order/count differs"
        );
        let raw_old: Box<serde_json::value::RawValue> =
            serde_json::from_slice(policy.content.as_bytes())?;
        let raw_new: Box<serde_json::value::RawValue> = serde_json::from_slice(content_bytes)?;
        let exact_old = exact(&raw_old, 0)?;
        let exact_new = exact(&raw_new, 0)?;
        fn exact_pages(value: &ExactJson) -> Result<&Vec<ExactJson>> {
            let ExactJson::Object(root) = value else {
                anyhow::bail!("invalid document object")
            };
            let Some(ExactJson::Object(cpages)) = root.get("cPages") else {
                anyhow::bail!("invalid cPages")
            };
            let Some(ExactJson::Array(pages)) = cpages.get("pages") else {
                anyhow::bail!("invalid pages")
            };
            Ok(pages)
        }
        ensure!(
            exact_pages(&exact_new)?
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != position + 1)
                .map(|(_, page)| page)
                .eq(exact_pages(&exact_old)?.iter()),
            "an original page object changed"
        );
        let page = pages[position + 1]
            .as_object()
            .context("invalid inserted page")?;
        ensure!(
            page.len() == 3
                && page.contains_key("id")
                && page.contains_key("idx")
                && page.contains_key("modifed"),
            "unexpected inserted page fields"
        );
        let idx = page["idx"].as_object().context("invalid inserted index")?;
        ensure!(
            idx.len() == 2 && idx.get("timestamp").and_then(Value::as_str) == Some("1:2"),
            "unexpected inserted index timestamp/fields"
        );
        let value = idx
            .get("value")
            .and_then(Value::as_str)
            .context("invalid inserted index value")?;
        ensure!(
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| byte.is_ascii_lowercase()),
            "invalid inserted fractional index"
        );
        let old_pages = before_content
            .pointer("/cPages/pages")
            .and_then(Value::as_array)
            .context("missing original pages")?;
        let left = string(&old_pages[position], "/idx/value")?;
        ensure!(
            left < value
                && (position + 1 == old_pages.len()
                    || value < string(&old_pages[position + 1], "/idx/value")?),
            "inserted index is not between original neighbours"
        );
        let old_modified = decimal(string(&before_metadata, "/lastModified")?)?;
        let modified = decimal(
            page["modifed"]
                .as_str()
                .context("invalid inserted modified timestamp")?,
        )?;
        let new_modified = decimal(string(&after_metadata, "/lastModified")?)?;
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        ensure!(
            modified >= old_modified
                && new_modified >= old_modified
                && u128::from(modified) <= wall
                && u128::from(new_modified) <= wall,
            "insertion timestamps outside original/current bounds"
        );
        let old_size = decimal(string(&before_content, "/sizeInBytes")?)?;
        let new_size = decimal(string(&after_content, "/sizeInBytes")?)?;
        let byte_delta = added_bytes + content_bytes.len() as i128 - policy.content.len() as i128
            + metadata_bytes.len() as i128
            - policy.metadata.len() as i128;
        ensure!(
            byte_delta >= 0 && i128::from(new_size) - i128::from(old_size) == byte_delta,
            "sizeInBytes delta must equal exact added-file plus document JSON byte growth"
        );
        let old_stamp = string(&before_content, "/cPages/lastOpened/timestamp")?
            .split_once(':')
            .context("invalid old register timestamp")?;
        let new_stamp = string(&after_content, "/cPages/lastOpened/timestamp")?
            .split_once(':')
            .context("invalid new register timestamp")?;
        let old_page = string(&before_content, "/cPages/lastOpened/value")?;
        let new_page = string(&after_content, "/cPages/lastOpened/value")?;
        ensure!(
            (new_page == old_page || new_page == plan.page || new_page == target)
                && decimal(old_stamp.0)? == decimal(new_stamp.0)?
                && decimal(new_stamp.1)? >= decimal(old_stamp.1)?
                && (new_page == old_page || decimal(new_stamp.1)? > decimal(old_stamp.1)?),
            "invalid inserted opening register"
        );
        let opened_index = after_metadata
            .get("lastOpenedPage")
            .and_then(Value::as_u64)
            .context("invalid inserted remembered index")?;
        let old_opened = decimal(string(&before_metadata, "/lastOpened")?)?;
        let new_opened = decimal(string(&after_metadata, "/lastOpened")?)?;
        let old_index = before_metadata
            .get("lastOpenedPage")
            .and_then(Value::as_u64)
            .context("invalid original remembered index")?;
        ensure!(
            opened_index < order.len() as u64
                && order[opened_index as usize].as_str() == new_page
                && new_opened >= old_opened
                && (opened_index == old_index || new_opened > old_opened),
            "invalid inserted remembered page/time"
        );
        fn mask(mut value: ExactJson, paths: &[&[&str]]) -> Result<ExactJson> {
            for path in paths {
                let mut field = &mut value;
                for key in *path {
                    let ExactJson::Object(fields) = field else {
                        anyhow::bail!("invalid development delta path")
                    };
                    field = fields
                        .get_mut(*key)
                        .context("missing development delta field")?;
                }
                *field = ExactJson::Null;
            }
            Ok(value)
        }
        ensure!(
            mask(
                protected(policy.content.as_bytes(), FileKind::Content)?,
                &[&["cPages", "pages"], &["pageCount"], &["sizeInBytes"]]
            )? == mask(
                protected(content_bytes, FileKind::Content)?,
                &[&["cPages", "pages"], &["pageCount"], &["sizeInBytes"]]
            )?,
            "protected content changed beyond exact insertion delta"
        );
        ensure!(
            mask(
                protected(policy.metadata.as_bytes(), FileKind::Metadata)?,
                &[&["lastModified"]]
            )? == mask(
                protected(metadata_bytes, FileKind::Metadata)?,
                &[&["lastModified"]]
            )?,
            "protected metadata changed beyond exact insertion delta"
        );
        for (_, pin, _) in &self.0 {
            pin.current()?;
        }
        for pin in &added_pins {
            pin.current()?;
        }
        check(deadline)
    }
}

// Reusing the explicit current target may change only that target's native
// stroke/thumbnail files and the accounted document metadata. No new page exists.
impl Samples {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn verify_reuse_pen(
        &self,
        policy: &OpeningMetadata,
        plan: &CapturePlan,
        target: &str,
        old_rm_bytes: u64,
        old_thumbnail_bytes: u64,
        source: &Path,
        original: &[SourceEntry],
        current: &[SourceEntry],
        deadline: Instant,
    ) -> Result<()> {
        ensure!(
            target == plan.page && original.len() == current.len(),
            "reuse must preserve current target and complete inventory"
        );
        let paths = [
            format!("{}/{target}.rm", plan.document),
            format!("{}.thumbnails/{target}.png", plan.document),
        ];
        let mutable = [
            FileKind::Content.path(plan),
            FileKind::Metadata.path(plan),
            paths[0].clone(),
            paths[1].clone(),
        ];
        for (old, new) in original.iter().zip(current) {
            ensure!(old.path == new.path, "reuse changed inventory paths");
            if mutable.contains(&old.path) {
                ensure!(
                    old.sha256.is_some() && new.sha256.is_some(),
                    "reuse changed target file type"
                );
            } else {
                ensure!(old == new, "reuse changed protected source: {}", old.path);
            }
        }
        let mut sizes = Vec::new();
        let mut pins = Vec::new();
        for path in &paths {
            let (pin, bytes) = read_pin(&source.join(path), deadline)?;
            ensure!(
                current.iter().any(|entry| entry.path == *path
                    && entry.sha256.as_deref() == Some(sha(&bytes).as_str())),
                "reused target changed after inventory"
            );
            sizes.push(bytes.len() as i128);
            pins.push(pin);
        }
        ensure!(
            sizes[0] > i128::from(old_rm_bytes)
                && original
                    .iter()
                    .find(|entry| entry.path == paths[0])
                    .context("original reused target absent")?
                    .sha256
                    != current
                        .iter()
                        .find(|entry| entry.path == paths[0])
                        .context("current reused target absent")?
                        .sha256,
            "reuse pen must change/grow native target stroke data"
        );
        let content = &self
            .0
            .iter()
            .find(|(kind, _, _)| matches!(kind, FileKind::Content))
            .context("missing reuse content")?
            .2;
        let metadata = &self
            .0
            .iter()
            .find(|(kind, _, _)| matches!(kind, FileKind::Metadata))
            .context("missing reuse metadata")?
            .2;
        let old_content = parse(policy.content.as_bytes())?;
        let new_content = parse(content)?;
        let old_meta = parse(policy.metadata.as_bytes())?;
        let new_meta = parse(metadata)?;
        let index = plan
            .order
            .iter()
            .position(|page| page == target)
            .context("reused target outside original order")?;
        let modified_path = format!("/cPages/pages/{index}/modifed");
        let old_modified = decimal(string(&old_content, &modified_path)?)?;
        let new_modified = decimal(string(&new_content, &modified_path)?)?;
        let old_meta_modified = decimal(string(&old_meta, "/lastModified")?)?;
        let new_meta_modified = decimal(string(&new_meta, "/lastModified")?)?;
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        ensure!(
            new_modified >= old_modified
                && new_meta_modified >= old_meta_modified
                && u128::from(new_modified) <= wall
                && u128::from(new_meta_modified) <= wall,
            "reuse modification timestamps outside declared bounds"
        );
        let delta = sizes[0] - i128::from(old_rm_bytes) + sizes[1]
            - i128::from(old_thumbnail_bytes)
            + content.len() as i128
            - policy.content.len() as i128
            + metadata.len() as i128
            - policy.metadata.len() as i128;
        ensure!(delta > 0 && i128::from(decimal(string(&new_content, "/sizeInBytes")?)?) - i128::from(decimal(string(&old_content, "/sizeInBytes")?)?) == delta, "reuse sizeInBytes must account exactly for target files and document JSON byte changes");
        let old_stamp = string(&old_content, "/cPages/lastOpened/timestamp")?
            .split_once(':')
            .context("invalid reuse baseline stamp")?;
        let new_stamp = string(&new_content, "/cPages/lastOpened/timestamp")?
            .split_once(':')
            .context("invalid reuse current stamp")?;
        let old_page = string(&old_content, "/cPages/lastOpened/value")?;
        let new_page = string(&new_content, "/cPages/lastOpened/value")?;
        ensure!(
            (new_page == old_page || new_page == target)
                && decimal(old_stamp.0)? == decimal(new_stamp.0)?
                && decimal(new_stamp.1)? >= decimal(old_stamp.1)?
                && (new_page == old_page || decimal(new_stamp.1)? > decimal(old_stamp.1)?),
            "reuse opening register changed unexpectedly"
        );
        let old_index = old_meta
            .get("lastOpenedPage")
            .and_then(Value::as_u64)
            .context("invalid old reuse index")?;
        let new_index = new_meta
            .get("lastOpenedPage")
            .and_then(Value::as_u64)
            .context("invalid new reuse index")?;
        let old_opened = decimal(string(&old_meta, "/lastOpened")?)?;
        let new_opened = decimal(string(&new_meta, "/lastOpened")?)?;
        ensure!(
            new_index < plan.order.len() as u64
                && plan.order[new_index as usize].as_str() == new_page
                && new_opened >= old_opened
                && (old_index == new_index || new_opened > old_opened),
            "reuse remembered page/time changed unexpectedly"
        );
        fn mask(mut value: ExactJson, kind: FileKind, index: usize) -> Result<ExactJson> {
            let ExactJson::Object(root) = &mut value else {
                anyhow::bail!("invalid reuse document")
            };
            match kind {
                FileKind::Metadata => {
                    *root
                        .get_mut("lastModified")
                        .context("missing lastModified")? = ExactJson::Null;
                }
                FileKind::Content => {
                    *root.get_mut("sizeInBytes").context("missing sizeInBytes")? = ExactJson::Null;
                    let Some(ExactJson::Object(cpages)) = root.get_mut("cPages") else {
                        anyhow::bail!("invalid reused cPages")
                    };
                    let Some(ExactJson::Array(pages)) = cpages.get_mut("pages") else {
                        anyhow::bail!("invalid reused pages")
                    };
                    let Some(ExactJson::Object(page)) = pages.get_mut(index) else {
                        anyhow::bail!("invalid reused page")
                    };
                    *page
                        .get_mut("modifed")
                        .context("missing reused modified timestamp")? = ExactJson::Null;
                }
            }
            Ok(value)
        }
        ensure!(
            mask(
                protected(policy.content.as_bytes(), FileKind::Content)?,
                FileKind::Content,
                index
            )? == mask(
                protected(content, FileKind::Content)?,
                FileKind::Content,
                index
            )?,
            "reuse changed original page/order/mapping or protected content"
        );
        ensure!(
            mask(
                protected(policy.metadata.as_bytes(), FileKind::Metadata)?,
                FileKind::Metadata,
                index
            )? == mask(
                protected(metadata, FileKind::Metadata)?,
                FileKind::Metadata,
                index
            )?,
            "reuse changed protected metadata"
        );
        for (_, pin, _) in &self.0 {
            pin.current()?;
        }
        for pin in pins {
            pin.current()?;
        }
        check(deadline)
    }
}

// Preserve number tokens before any f64 conversion. Distinct large integers,
// high-precision decimals and underflowing values must never compare equal.
#[derive(PartialEq, Eq)]
enum ExactJson {
    Object(BTreeMap<String, ExactJson>),
    Array(Vec<ExactJson>),
    String(String),
    Number(String),
    Bool(bool),
    Null,
}
fn exact(raw: &serde_json::value::RawValue, depth: usize) -> Result<ExactJson> {
    ensure!(depth <= 64, "metadata nesting exceeds bound");
    let text = raw.get().trim();
    Ok(match text.as_bytes().first().context("empty JSON value")? {
        b'{' => {
            // The preceding StrictValue parse rejects duplicates recursively.
            let values: BTreeMap<String, Box<serde_json::value::RawValue>> =
                serde_json::from_str(text)?;
            ExactJson::Object(
                values
                    .into_iter()
                    .map(|(key, value)| Ok((key, exact(&value, depth + 1)?)))
                    .collect::<Result<_>>()?,
            )
        }
        b'[' => {
            let values: Vec<Box<serde_json::value::RawValue>> = serde_json::from_str(text)?;
            ExactJson::Array(
                values
                    .into_iter()
                    .map(|v| exact(&v, depth + 1))
                    .collect::<Result<_>>()?,
            )
        }
        b'"' => ExactJson::String(serde_json::from_str(text)?),
        b't' | b'f' => ExactJson::Bool(serde_json::from_str(text)?),
        b'n' => {
            ensure!(text == "null", "invalid null");
            ExactJson::Null
        }
        b'-' | b'0'..=b'9' => ExactJson::Number(text.into()),
        _ => anyhow::bail!("invalid JSON value"),
    })
}
fn protected(bytes: &[u8], kind: FileKind) -> Result<ExactJson> {
    ensure!(bytes.len() <= MAX_JSON, "metadata JSON exceeds bound");
    let raw: Box<serde_json::value::RawValue> = serde_json::from_slice(bytes)?;
    let mut value = exact(&raw, 0)?;
    let paths: &[&[&str]] = match kind {
        FileKind::Metadata => &[&["lastOpened"], &["lastOpenedPage"]],
        FileKind::Content => &[
            &["cPages", "lastOpened", "value"],
            &["cPages", "lastOpened", "timestamp"],
        ],
    };
    for path in paths {
        let mut field = &mut value;
        for key in *path {
            let ExactJson::Object(fields) = field else {
                anyhow::bail!("invalid activity path");
            };
            field = fields.get_mut(*key).context("missing activity key")?;
        }
        *field = ExactJson::Null;
    }
    Ok(value)
}

fn string<'a>(value: &'a Value, path: &str) -> Result<&'a str> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .context("missing or mistyped activity field")
}
fn decimal(value: &str) -> Result<u64> {
    ensure!(
        !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit()),
        "invalid decimal activity value"
    );
    let number: u64 = value.parse()?;
    ensure!(
        number.to_string() == value,
        "noncanonical decimal activity value"
    );
    Ok(number)
}
/// Check the complete inventory first. Only these two existing regular files
/// may differ; all directories and all other hashes must match exactly.
pub(super) fn verify_inventory(
    plan: &CapturePlan,
    original: &[SourceEntry],
    current: &[SourceEntry],
) -> Result<()> {
    ensure!(
        original.len() == current.len(),
        "source inventory size changed"
    );
    let allowed = [FileKind::Content.path(plan), FileKind::Metadata.path(plan)];
    for (old, new) in original.iter().zip(current) {
        ensure!(old.path == new.path, "source inventory path changed");
        if allowed.contains(&old.path) {
            ensure!(
                old.sha256.is_some() && new.sha256.is_some(),
                "metadata file type changed"
            );
        } else {
            ensure!(
                old == new,
                "protected source bytes/type changed: {}",
                old.path
            );
        }
    }
    Ok(())
}

// A Value parse alone would silently discard repeated keys, including unknown
// fields. Reject them recursively before any protected-value comparison.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Strict;
        impl<'de> Visitor<'de> for Strict {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique JSON fields")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut map = Map::new();
                while let Some(key) = a.next_key::<String>()? {
                    if map.contains_key(&key) {
                        return Err(de::Error::custom("repeated metadata field"));
                    }
                    map.insert(key, a.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(map)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = a.next_element::<StrictValue>()? {
                    values.push(value.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(serde_json::json!(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
        }
        d.deserialize_any(Strict)
    }
}
fn parse(bytes: &[u8]) -> Result<Value> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_JSON,
        "metadata JSON exceeds bound"
    );
    let value = serde_json::from_slice::<StrictValue>(bytes)?.0;
    ensure!(value.is_object(), "metadata must be an object");
    Ok(value)
}

#[cfg(test)]
mod tests;
