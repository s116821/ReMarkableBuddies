//! Closed portable wire identities. Local selection/admission tokens never cross.
use super::*;
use crate::memory::catalog::Owner as MemoryOwner;
use serde::{Deserialize, Serialize};

pub(super) const WIRE: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Owner {
    Memory(MemoryOwner),
    Preferences(PreferenceOwner),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreferenceOwner {
    kind: PreferenceKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PreferenceKind {
    PortablePreferences,
}
impl Owner {
    pub fn key_sha256(&self) -> Result<String> {
        match self {
            Self::Memory(owner) => owner.key_sha256(),
            Self::Preferences(_) => Ok(crate::config::portable::record::key_sha256()),
        }
    }
}
impl From<MemoryOwner> for Owner {
    fn from(owner: MemoryOwner) -> Self {
        Self::Memory(owner)
    }
}

pub(super) fn root_key(group: Uuid) -> String {
    digest(format!("urn:rmb:shared-root:v1:{group}").as_bytes())
}
pub(super) fn catalog_key(group: Uuid) -> String {
    digest(format!("urn:rmb:shared-catalog:v1:{group}").as_bytes())
}
pub(super) fn genesis_base(root: &str, key: &str) -> String {
    digest(format!("urn:rmb:shared-genesis:v1:{root}:{key}").as_bytes())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Root {
    pub schema: u32,
    pub group: Uuid,
    pub protocol: String,
    pub binding_sha256: String,
    pub profile: Uuid,
    pub catalog_slot: String,
}
impl Root {
    pub fn validate(&self, binding: &Binding) -> Result<()> {
        ensure!(
            self.schema == WIRE && self.protocol == "buddy-shared-fixed-slot-v1",
            "unsupported shared root"
        );
        ensure!(
            self.group == binding.group
                && self.binding_sha256 == binding.digest
                && !self.profile.is_nil(),
            "shared root binding differs"
        );
        drive::id(&self.catalog_slot)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub owner: Owner,
    pub genesis_slot: String,
}
impl Entry {
    pub fn validate(&self, key: &str, profile: Uuid) -> Result<()> {
        ensure!(self.owner.key_sha256()? == key, "catalog owner key differs");
        if let Owner::Memory(MemoryOwner::GlobalFact { key }) = &self.owner {
            ensure!(key.profile == profile, "foreign global profile");
        }
        drive::id(&self.genesis_slot)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Catalog {
    pub schema: u32,
    pub group: Uuid,
    pub root_sha256: String,
    pub base_sha256: String,
    pub slot: String,
    pub operation: Uuid,
    #[serde(deserialize_with = "unique_entries")]
    pub entries: BTreeMap<String, Entry>,
    pub next_slot: String,
}
impl Catalog {
    pub fn validate(
        &self,
        root: &Root,
        root_digest: &str,
        slot: &str,
        base: &str,
        prior: &BTreeMap<String, Entry>,
    ) -> Result<()> {
        ensure!(
            self.schema == WIRE
                && self.group == root.group
                && self.root_sha256 == root_digest
                && self.base_sha256 == base
                && self.slot == slot
                && !self.operation.is_nil(),
            "catalog ancestry differs"
        );
        drive::id(&self.next_slot)?;
        ensure!(
            self.next_slot != slot && self.entries.len() <= crate::memory::catalog::MAX_AGGREGATES,
            "catalog bound or successor differs"
        );
        for (key, entry) in &self.entries {
            entry.validate(key, root.profile)?;
        }
        ensure!(
            prior
                .iter()
                .all(|(key, entry)| self.entries.get(key) == Some(entry)),
            "catalog removed or replaced a mapping"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Commit {
    pub schema: u32,
    pub group: Uuid,
    pub root_sha256: String,
    pub owner: Owner,
    pub key: String,
    pub base_sha256: String,
    pub slot: String,
    pub operation: Uuid,
    pub actor: Uuid,
    pub manifest_id: String,
    pub manifest: ObjectRef,
    pub next_slot: String,
}
impl Commit {
    pub fn validate(
        &self,
        root: &Root,
        root_digest: &str,
        key: &str,
        entry: &Entry,
        slot: &str,
        base: &str,
    ) -> Result<()> {
        ensure!(
            self.schema == WIRE
                && self.group == root.group
                && self.root_sha256 == root_digest
                && self.owner == entry.owner
                && self.key == key
                && self.base_sha256 == base
                && self.slot == slot
                && !self.operation.is_nil()
                && !self.actor.is_nil(),
            "commit ancestry differs"
        );
        entry.validate(key, root.profile)?;
        drive::id(&self.manifest_id)?;
        drive::id(&self.next_slot)?;
        ensure!(
            self.next_slot != slot
                && self.manifest_id != slot
                && valid_digest(&self.manifest.sha256)
                && self.manifest.bytes > 0
                && self.manifest.bytes <= MAX_METADATA as u64,
            "commit object bounds differ"
        );
        Ok(())
    }
}

fn unique_entries<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, Entry>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, Entry>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique scope mappings")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry()? {
                if result.len() >= crate::memory::catalog::MAX_AGGREGATES
                    || result.insert(key, value).is_some()
                {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive catalog mapping",
                    ));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Unique)
}
