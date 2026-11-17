//! Private local Capacities enrollment selection and read-only grant custody.
use crate::{note_export::Failure, storage::Uuid};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::{collections::BTreeSet, fmt, path::Path};

#[derive(Clone, Serialize)]
pub struct ExportConnection {
    pub schema_version: u32,
    pub space_id: Uuid,
    pub grant_id: Uuid,
    pub tags: Vec<Uuid>,
}

impl fmt::Debug for ExportConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[Capacities connection selection]")
    }
}

impl<'de> Deserialize<'de> for ExportConnection {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            schema_version: u32,
            space_id: Uuid,
            grant_id: Uuid,
            tags: Vec<Uuid>,
        }

        struct Object;
        impl<'de> de::Visitor<'de> for Object {
            type Value = ExportConnection;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("local Capacities connection object")
            }
            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let fields = Fields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ExportConnection {
                    schema_version: fields.schema_version,
                    space_id: fields.space_id,
                    grant_id: fields.grant_id,
                    tags: fields.tags,
                })
            }
        }
        d.deserialize_map(Object)
            .map_err(|_| de::Error::custom("invalid Capacities connection"))
    }
}

impl ExportConnection {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "unsupported Capacities connection version"
        );
        anyhow::ensure!(
            !self.space_id.is_nil()
                && !self.grant_id.is_nil()
                && self.tags.len() <= 16
                && self.tags.iter().all(|id| !id.is_nil())
                && self.tags.iter().collect::<BTreeSet<_>>().len() == self.tags.len(),
            "invalid Capacities connection"
        );
        Ok(())
    }
}

#[cfg(target_os = "linux")]
const MAX_GRANT: u64 = 16 * 1024;

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    schema_version: u32,
    grant_id: Uuid,
    space_id: Uuid,
    client_id: String,
    resource: String,
    scopes: Vec<String>,
    expires_at: u64,
    access_token: String,
    refresh_token: String,
}

#[cfg(target_os = "linux")]
fn admitted_grant(
    bytes: &[u8],
    space: Uuid,
    id: Uuid,
    may_write: bool,
    now: u64,
) -> Result<super::Secret, Failure> {
    const RESOURCE: &str = "https://api.capacities.io";
    let invalid = Failure::MissingCapacitiesCredentials;
    let raw = super::inspection::strict_json(bytes).map_err(|_| invalid)?;
    if !raw.is_object() {
        return Err(invalid);
    }
    let grant: Grant = serde_json::from_value(raw).map_err(|_| invalid)?;
    if grant.schema_version != 1 {
        return Err(Failure::Unsupported);
    }
    let scopes: BTreeSet<_> = grant.scopes.iter().map(String::as_str).collect();
    let text = |value: &str, limit| {
        !value.is_empty()
            && value.len() <= limit
            && value.bytes().all(|byte| byte.is_ascii_graphic())
    };
    if grant.grant_id != id
        || grant.space_id != space
        || grant.resource != RESOURCE
        || !text(&grant.client_id, 256)
        || !text(&grant.access_token, 4096)
        || grant.access_token.starts_with("cap-api-")
        || !text(&grant.refresh_token, 4096)
        || scopes.len() != grant.scopes.len()
        || !scopes
            .iter()
            .all(|scope| matches!(*scope, "api:read" | "api:write" | "offline_access"))
        || !scopes.contains("api:read")
        || !scopes.contains("offline_access")
        || (may_write && !scopes.contains("api:write"))
        || grant.expires_at <= now
    {
        return Err(Failure::Authentication);
    }
    Ok(super::Secret(grant.access_token))
}

pub(crate) fn access_token(
    root: &Path,
    space: Uuid,
    id: Uuid,
    may_write: bool,
) -> Result<super::Secret, Failure> {
    if space.is_nil() || id.is_nil() {
        return Err(Failure::MissingCapacitiesCredentials);
    }
    #[cfg(target_os = "linux")]
    {
        let target = root.join(format!("capacities-grant-{id}.json"));
        let bytes = super::model_credential::linux::read_private(root, &target, MAX_GRANT)
            .map_err(|_| Failure::MissingCapacitiesCredentials)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Failure::Authentication)?
            .as_secs();
        admitted_grant(&bytes, space, id, may_write, now)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, may_write);
        Err(Failure::Unsupported)
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    pub(crate) fn grant_fixture(id: Uuid, space: Uuid, expires_at: u64) -> Value {
        json!({
            "schema_version": 1,
            "grant_id": id,
            "space_id": space,
            "client_id": "fixture-public-client",
            "resource": "https://api.capacities.io",
            "scopes": ["api:read", "api:write", "offline_access"],
            "expires_at": expires_at,
            "access_token": "synthetic-access-token",
            "refresh_token": "synthetic-refresh-token"
        })
    }

    #[test]
    fn exact_private_file_admits_only_the_original_current_oauth_grant() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let id = Uuid::new_v4();
        let space = Uuid::new_v4();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let file = root.path().join(format!("capacities-grant-{id}.json"));
        let mut record = grant_fixture(id, space, now + 900);
        let write = |record: &Value, mode| {
            fs::write(&file, serde_json::to_vec(record).unwrap()).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        };
        write(&record, 0o600);
        assert_eq!(
            access_token(root.path(), space, id, true).unwrap().expose(),
            "synthetic-access-token"
        );
        assert_eq!(
            access_token(root.path(), space, Uuid::new_v4(), false).unwrap_err(),
            Failure::MissingCapacitiesCredentials
        );

        record["grant_id"] = json!(Uuid::new_v4());
        write(&record, 0o600);
        assert_eq!(
            access_token(root.path(), space, id, false).unwrap_err(),
            Failure::Authentication
        );

        record["grant_id"] = json!(id);
        record["scopes"] = json!(["api:read", "offline_access"]);
        write(&record, 0o600);
        assert!(access_token(root.path(), space, id, false).is_ok());
        assert_eq!(
            access_token(root.path(), space, id, true).unwrap_err(),
            Failure::Authentication
        );

        record["expires_at"] = json!(now - 1);
        write(&record, 0o600);
        assert_eq!(
            access_token(root.path(), space, id, false).unwrap_err(),
            Failure::Authentication
        );

        record["expires_at"] = json!(now + 900);
        record["schema_version"] = json!(2);
        write(&record, 0o600);
        assert_eq!(
            access_token(root.path(), space, id, false).unwrap_err(),
            Failure::Unsupported
        );
        record["schema_version"] = json!(1);
        write(&record, 0o644);
        assert_eq!(
            access_token(root.path(), space, id, false).unwrap_err(),
            Failure::MissingCapacitiesCredentials
        );
    }
}
