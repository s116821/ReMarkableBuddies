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
