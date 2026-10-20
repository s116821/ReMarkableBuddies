use super::*;
use crate::{
    config::{basic::checked_document, inspection, Config},
    storage::{
        digest, files, maintenance::Refusal, valid_digest, Store, StorePaths, Uuid, MAX_RECORD,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path},
    sync::Arc,
};

type Checked<T> = std::result::Result<T, Status>;
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum State {
    Absent,
    Present {
        sha256: String,
        metadata_sha256: String,
        mode: u32,
    },
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    config_sha256: String,
    config_path_sha256: String,
    credential_path_sha256: String,
    parent_identity_sha256: String,
    state: State,
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    Install,
    Replace,
    Remove,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Change {
    Install { value: String },
    Replace { value: String },
    Remove,
}
impl Change {
    fn action(&self) -> Action {
        match self {
            Self::Install { .. } => Action::Install,
            Self::Replace { .. } => Action::Replace,
            Self::Remove => Action::Remove,
        }
    }
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Install { value } | Self::Replace { value } => Some(value.as_bytes()),
            Self::Remove => None,
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::Install { value } | Self::Replace { value } => {
                value.len() <= 8192 && !value.trim().is_empty()
            }
            Self::Remove => true,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Desired {
    Absent,
    Present { sha256: String, mode: u32 },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    credential_version: u32,
    operation: Uuid,
    base: Binding,
    action: Action,
    target: Desired,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    credential_version: u32,
    operation: Uuid,
    base: Binding,
    change: Change,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    intent: Intent,
    change: Change,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusRequest {
    intent: Intent,
}

fn private_mode(mode: u32) -> bool {
    matches!(mode, 0o400 | 0o600)
}
impl Binding {
    fn valid(&self) -> bool {
        [
            &self.config_sha256,
            &self.config_path_sha256,
            &self.credential_path_sha256,
            &self.parent_identity_sha256,
        ]
        .into_iter()
        .all(|s| valid_digest(s))
            && match &self.state {
                State::Absent => true,
                State::Present {
                    sha256,
                    metadata_sha256,
                    mode,
                } => valid_digest(sha256) && valid_digest(metadata_sha256) && private_mode(*mode),
            }
    }
    fn same_location(&self, other: &Self) -> bool {
        self.config_sha256 == other.config_sha256
            && self.config_path_sha256 == other.config_path_sha256
            && self.credential_path_sha256 == other.credential_path_sha256
            && self.parent_identity_sha256 == other.parent_identity_sha256
    }
}
impl Intent {
    fn validate(&self) -> Checked<()> {
        if self.credential_version != 1 {
            return Err(Status::Unsupported);
        }
        if self.operation.is_nil() || !self.base.valid() {
            return Err(Status::InvalidInput);
        }
        let valid = match (&self.action, &self.base.state, &self.target) {
            (
                Action::Install,
                State::Absent,
                Desired::Present {
                    sha256,
                    mode: 0o600,
                },
            ) => valid_digest(sha256),
            (
                Action::Replace,
                State::Present {
                    sha256: old,
                    mode: old_mode,
                    ..
                },
                Desired::Present { sha256, mode },
            ) => valid_digest(sha256) && old != sha256 && old_mode == mode,
            (Action::Remove, State::Present { .. }, Desired::Absent) => true,
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(Status::InvalidInput)
        }
    }
    fn matches_change(&self, change: &Change) -> bool {
        change.valid()
            && self.action == change.action()
            && match (&self.target, change.bytes()) {
                (Desired::Absent, None) => true,
                (Desired::Present { sha256, .. }, Some(bytes)) => digest(bytes) == *sha256,
                _ => false,
            }
    }
}
fn clean(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn path_hash(kind: &str, path: &Path) -> Checked<String> {
    let s = path.to_str().ok_or(Status::Unsupported)?;
    Ok(digest(
        format!("urn:remarkable-buddies:model-credential-{kind}:v1:{s}").as_bytes(),
    ))
}
fn metadata_hash(meta: &fs::Metadata, file: bool) -> String {
    let mut values = BTreeMap::from([
        ("device", meta.dev().to_string()),
        ("inode", meta.ino().to_string()),
        ("uid", meta.uid().to_string()),
        ("gid", meta.gid().to_string()),
        ("mode", meta.mode().to_string()),
    ]);
    if file {
        values.extend([
            ("links", meta.nlink().to_string()),
            ("length", meta.len().to_string()),
            ("mtime_seconds", meta.mtime().to_string()),
            ("mtime_nanoseconds", meta.mtime_nsec().to_string()),
            ("ctime_seconds", meta.ctime().to_string()),
            ("ctime_nanoseconds", meta.ctime_nsec().to_string()),
        ]);
    }
    metadata_values_hash(&values, file)
}
fn metadata_values_hash(values: &BTreeMap<&str, String>, file: bool) -> String {
    let mut bytes = format!(
        "urn:remarkable-buddies:model-credential-{}:v1:",
        if file { "file-metadata" } else { "parent" }
    )
    .into_bytes();
    serde_json::to_writer(&mut bytes, values).expect("fixed metadata serializes");
    digest(&bytes)
}
fn owner(meta: &fs::Metadata) -> bool {
    // IDs are process metadata, never credential/provider discovery.
    meta.uid() == unsafe { nix::libc::geteuid() } && meta.gid() == unsafe { nix::libc::getegid() }
}
fn parent(path: &Path) -> Checked<fs::Metadata> {
    files::safe_path(path).map_err(|_| Status::Unsupported)?;
    let meta = fs::symlink_metadata(path).map_err(|_| Status::Unavailable)?;
    if !meta.is_dir() || !owner(&meta) || meta.mode() & 0o7777 != 0o700 {
        return Err(Status::Unsupported);
    }
    Ok(meta)
}
struct Observation {
    config: Config,
    path: PathBuf,
    root: PathBuf,
    target: PathBuf,
    binding: Binding,
}
fn observe(path: Option<PathBuf>) -> Checked<Observation> {
    let path = path.filter(|p| clean(p)).ok_or(Status::Unavailable)?;
    let raw = files::read(&path, MAX_RECORD as u64).map_err(|_| Status::Unavailable)?;
    let (config, _) = checked_document(&raw)?;
    let name = config
        .model_credential
        .as_deref()
        .ok_or(Status::Unsupported)?;
    let root = config.paths.credentials.clone();
    if !clean(&root)
        || [&config.paths.data, &config.paths.cache]
            .into_iter()
            .any(|p| !clean(p) || overlap(&root, p))
        || overlap(&path, &root)
    {
        return Err(Status::Unsupported);
    }
    let root_meta = parent(&root)?;
    let canonical_root = fs::canonicalize(&root).map_err(|_| Status::Unavailable)?;
    let canonical_config = fs::canonicalize(&path).map_err(|_| Status::Unavailable)?;
    if overlap(&canonical_root, &canonical_config) {
        return Err(Status::Unsupported);
    }
    let config_meta = fs::symlink_metadata(&path).map_err(|_| Status::Unavailable)?;
    let target = root.join(name);
    let state = match fs::symlink_metadata(&target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::Absent,
        Err(_) => return Err(Status::Unavailable),
        Ok(meta) => {
            let mode = meta.mode() & 0o7777;
            if !meta.is_file()
                || meta.nlink() != 1
                || !owner(&meta)
                || !private_mode(mode)
                || (meta.dev() == config_meta.dev() && meta.ino() == config_meta.ino())
            {
                return Err(Status::Unsupported);
            }
            if meta.len() > MAX_RECORD as u64 {
                return Err(Status::Unavailable);
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(&target)
                .map_err(|_| Status::Unavailable)?;
            let identity = metadata_hash(&meta, true);
            if metadata_hash(&file.metadata().map_err(|_| Status::Unavailable)?, true) != identity {
                return Err(Status::Unavailable);
            }
            let mut bytes = Vec::new();
            (&file)
                .take((MAX_RECORD + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| Status::Unavailable)?;
            if bytes.len() != meta.len() as usize
                || metadata_hash(&file.metadata().map_err(|_| Status::Unavailable)?, true)
                    != identity
                || metadata_hash(
                    &fs::symlink_metadata(&target).map_err(|_| Status::Unavailable)?,
                    true,
                ) != identity
            {
                return Err(Status::Unavailable);
            }
            State::Present {
                sha256: digest(&bytes),
                metadata_sha256: identity,
                mode,
            }
        }
    };
    let parent_identity_sha256 = metadata_hash(&root_meta, false);
    if metadata_hash(&parent(&root)?, false) != parent_identity_sha256 {
        return Err(Status::Unavailable);
    }
    let binding = Binding {
        config_sha256: digest(&raw),
        config_path_sha256: path_hash("config-path", &path)?,
        credential_path_sha256: path_hash("file-path", &target)?,
        parent_identity_sha256,
        state,
    };
    Ok(Observation {
        config,
        path,
        root,
        target,
        binding,
    })
}
fn no_arrays(v: &Value) -> bool {
    match v {
        Value::Array(_) => false,
        Value::Object(m) => m.values().all(no_arrays),
        _ => true,
    }
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Checked<T> {
    let raw = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    if !raw.is_object() || !no_arrays(&raw) {
        return Err(Status::InvalidInput);
    }
    serde_json::from_value(raw).map_err(|_| Status::InvalidInput)
}
pub(super) fn inspect(path: Option<PathBuf>) -> Report {
    match observe(path) {
        Ok(doc) => Report::success(
            Operation::Inspect,
            Outcome::ReadOnly,
            json!({"schema_version":1,"binding":doc.binding}),
        ),
        Err(s) => Report::refusal(Operation::Inspect, s),
    }
}
pub(super) fn run(op: Operation, path: Option<PathBuf>, input: &mut impl Read) -> Report {
    let result = (|| -> Checked<Report> {
        if op == Operation::Inspect {
            return Err(Status::InvalidInput);
        }
        let mut bytes = Vec::new();
        input
            .take((MAX_IO + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Status::InvalidInput)?;
        if bytes.len() > MAX_IO {
            return Err(Status::InvalidInput);
        }
        match op {
            Operation::Prepare => {
                let request: Prepare = parse(&bytes)?;
                if request.credential_version != 1 {
                    return Err(Status::Unsupported);
                }
                if request.operation.is_nil() || !request.base.valid() || !request.change.valid() {
                    return Err(Status::InvalidInput);
                }
                let doc = observe(path)?;
                if doc.binding != request.base {
                    return Err(Status::Conflict);
                }
                let target = match (&doc.binding.state, &request.change) {
                    (State::Absent, Change::Install { value }) => Some(Desired::Present {
                        sha256: digest(value.as_bytes()),
                        mode: 0o600,
                    }),
                    (State::Present { sha256, mode, .. }, Change::Replace { value }) => {
                        if *sha256 == digest(value.as_bytes()) {
                            None
                        } else {
                            Some(Desired::Present {
                                sha256: digest(value.as_bytes()),
                                mode: *mode,
                            })
                        }
                    }
                    (State::Present { .. }, Change::Remove) => Some(Desired::Absent),
                    (State::Absent, Change::Remove) => None,
                    _ => return Err(Status::Conflict),
                };
                match target {
                    Some(target) => Ok(Report::success(
                        op,
                        Outcome::Prepared,
                        json!({"intent":Intent{credential_version:1,operation:request.operation,base:doc.binding,action:request.change.action(),target}}),
                    )),
                    None => Ok(Report::success(
                        op,
                        Outcome::NoChange,
                        json!({"binding":doc.binding}),
                    )),
                }
            }
            Operation::Apply => {
                let request: Apply = parse(&bytes)?;
                request.intent.validate()?;
                if !request.intent.matches_change(&request.change) {
                    return Err(Status::InvalidInput);
                }
                let doc = observe(path)?;
                if doc.binding != request.intent.base {
                    return Err(Status::Conflict);
                }
                apply(doc, request.intent, request.change)
            }
            Operation::Status => {
                let request: StatusRequest = parse(&bytes)?;
                request.intent.validate()?;
                let doc = observe(path)?;
                if !doc.binding.same_location(&request.intent.base) {
                    return Err(Status::Conflict);
                }
                if desired(&doc.binding.state, &request.intent.target) {
                    Ok(Report::success(
                        op,
                        Outcome::DesiredFileObserved,
                        json!({"present":matches!(request.intent.target,Desired::Present{..})}),
                    ))
                } else {
                    let mut report = Report::refusal(op, Status::Ok);
                    report.outcome = Outcome::Unresolved;
                    Ok(report)
                }
            }
            Operation::Inspect => unreachable!(),
        }
    })();
    match result {
        Ok(r) => r,
        Err(s) => Report::refusal(op, s),
    }
}
fn desired(state: &State, target: &Desired) -> bool {
    match (state, target) {
        (State::Absent, Desired::Absent) => true,
        (
            State::Present { sha256, mode, .. },
            Desired::Present {
                sha256: hash,
                mode: wanted,
            },
        ) => sha256 == hash && mode == wanted,
        _ => false,
    }
}
fn apply(doc: Observation, intent: Intent, change: Change) -> Checked<Report> {
    doc.config.validate().map_err(|_| Status::Unsupported)?;
    let root = fs::canonicalize(&doc.root).map_err(|_| Status::Unavailable)?;
    let root_meta = parent(&doc.root)?;
    for other in [&doc.config.paths.data, &doc.config.paths.cache] {
        let other = fs::canonicalize(other).map_err(|_| Status::Unavailable)?;
        let meta = fs::metadata(&other).map_err(|_| Status::Unavailable)?;
        if overlap(&root, &other)
            || (meta.dev() == root_meta.dev() && meta.ino() == root_meta.ino())
        {
            return Err(Status::Unsupported);
        }
    }
    let store = Arc::new(
        Store::open_existing_for_config(doc.config.paths.clone()).map_err(|r| match r {
            Refusal::Busy => Status::Busy,
            Refusal::Unavailable => Status::Unavailable,
            Refusal::Unsupported => Status::Unsupported,
        })?,
    );
    let custody = Custody::acquire(store.clone()).map_err(|_| Status::Unsupported)?;
    let mut report = store.publish_declared_model_credential(&doc.path, &intent, &change);
    report._custody = Some(custody);
    Ok(report)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Staged,
    BeforeSyscall,
    AfterSyscall,
    BeforeSync,
    BeforeReadback,
}
struct Stage {
    path: PathBuf,
    parent_identity: String,
    identity: (u64, u64),
}
impl Stage {
    fn create(root: &Path, bytes: &[u8], mode: u32) -> Checked<Self> {
        if !private_mode(mode) {
            return Err(Status::Unsupported);
        }
        let path = root.join(format!(".credential-stage-{}", Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| Status::Unavailable)?;
        let meta = file.metadata().map_err(|_| Status::Unavailable)?;
        let stage = Self {
            path,
            parent_identity: metadata_hash(&parent(root)?, false),
            identity: (meta.dev(), meta.ino()),
        };
        let mut file = file;
        file.write_all(bytes).map_err(|_| Status::Unavailable)?;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|_| Status::Unavailable)?;
        file.sync_all().map_err(|_| Status::Unavailable)?;
        let meta = file.metadata().map_err(|_| Status::Unavailable)?;
        if !meta.is_file() || !owner(&meta) || meta.nlink() != 1 || meta.mode() & 0o7777 != mode {
            return Err(Status::Unsupported);
        }
        Ok(stage)
    }
    fn cleanup(&self) -> Checked<()> {
        let root = self.path.parent().ok_or(Status::Unavailable)?;
        if metadata_hash(&parent(root)?, false) != self.parent_identity {
            return Err(Status::Unavailable);
        }
        match fs::symlink_metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(m) if m.is_file() && (m.dev(), m.ino()) == self.identity => {
                fs::remove_file(&self.path).map_err(|_| Status::Unavailable)
            }
            _ => Err(Status::Unavailable),
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

pub(crate) fn publish_locked(
    path: &Path,
    paths: &StorePaths,
    intent: &Intent,
    change: &Change,
) -> Report {
    publish_with(path, paths, intent, change, |_| Ok(()))
}
pub(crate) fn publish_with(
    path: &Path,
    paths: &StorePaths,
    intent: &Intent,
    change: &Change,
    mut hook: impl FnMut(Phase) -> Checked<()>,
) -> Report {
    let mut attempted = false;
    let result = (|| -> Checked<Report> {
        intent.validate()?;
        let doc = observe(Some(path.to_owned()))?;
        if doc.binding != intent.base {
            return Err(Status::Conflict);
        }
        if doc.config.paths.data != paths.data
            || doc.config.paths.cache != paths.cache
            || doc.config.paths.credentials != paths.credentials
        {
            return Err(Status::Conflict);
        }
        if !intent.matches_change(change) {
            return Err(Status::InvalidInput);
        }
        let stage = match (&intent.target, change.bytes()) {
            (Desired::Present { mode, .. }, Some(bytes)) => {
                Some(Stage::create(&doc.root, bytes, *mode)?)
            }
            _ => None,
        };
        hook(Phase::Staged)?;
        if observe(Some(path.to_owned()))?.binding != intent.base {
            return Err(Status::Conflict);
        }
        hook(Phase::BeforeSyscall)?;
        attempted = true;
        match (&intent.action, &stage) {
            (Action::Install, Some(stage)) => {
                fs::hard_link(&stage.path, &doc.target).map_err(|_| Status::Unavailable)?;
                stage.cleanup()?;
            }
            (Action::Replace, Some(stage)) => {
                fs::rename(&stage.path, &doc.target).map_err(|_| Status::Unavailable)?
            }
            (Action::Remove, None) => {
                fs::remove_file(&doc.target).map_err(|_| Status::Unavailable)?
            }
            _ => return Err(Status::InvalidInput),
        }
        hook(Phase::AfterSyscall)?;
        hook(Phase::BeforeSync)?;
        files::sync_dir(&doc.root).map_err(|_| Status::Unavailable)?;
        hook(Phase::BeforeReadback)?;
        let observed = observe(Some(path.to_owned()))?;
        if !observed.binding.same_location(&intent.base)
            || !desired(&observed.binding.state, &intent.target)
        {
            return Err(Status::Unavailable);
        }
        let mut report = Report::success(
            Operation::Apply,
            Outcome::Verified,
            json!({"present":matches!(intent.target,Desired::Present{..})}),
        );
        report.disk_readback_verified = true;
        Ok(report)
    })();
    match result {
        Ok(r) => r,
        Err(s) => {
            let mut r =
                Report::refusal(Operation::Apply, if attempted { Status::Failed } else { s });
            if attempted {
                r.outcome = Outcome::Unknown
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SECRET: &str = "SYNTHETIC_SECRET\r\n tail α ";
    struct Fixture {
        root: PathBuf,
        path: PathBuf,
        target: PathBuf,
        paths: StorePaths,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("buddy-credential-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let paths = StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            };
            drop(Store::open(paths.clone()).unwrap());
            let path = root.join("config.json");
            fs::write(&path,serde_json::to_vec(&json!({"paths":paths,"model_credential":"fixture","model":"SYNTHETIC_UNRELATED"})).unwrap()).unwrap();
            let target = paths.credentials.join("fixture");
            for p in [
                paths.data.join("sentinel"),
                paths.cache.join("sentinel"),
                paths.credentials.join("other"),
            ] {
                fs::write(p, b"SYNTHETIC_UNRELATED").unwrap();
            }
            Self {
                root,
                path,
                target,
                paths,
            }
        }
        fn run(&self, op: Operation, input: Value) -> Report {
            run(
                op,
                Some(self.path.clone()),
                &mut serde_json::to_vec(&input).unwrap().as_slice(),
            )
        }
        fn prepare(&self, change: Value) -> Intent {
            let base = observe(Some(self.path.clone())).unwrap().binding;
            let r=finish(self.run(Operation::Prepare,json!({"credential_version":1,"operation":Uuid::new_v4(),"base":base,"change":change})));
            assert_eq!(r["outcome"], "prepared");
            serde_json::from_value(r["data"]["intent"].clone()).unwrap()
        }
        fn set(&self, bytes: &[u8], mode: u32) {
            files::atomic(&self.target, bytes).unwrap();
            fs::set_permissions(&self.target, fs::Permissions::from_mode(mode)).unwrap();
        }
        fn unchanged(&self) {
            for p in [
                self.paths.data.join("sentinel"),
                self.paths.cache.join("sentinel"),
                self.paths.credentials.join("other"),
            ] {
                assert_eq!(fs::read(p).unwrap(), b"SYNTHETIC_UNRELATED");
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
                assert!(self
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("buddy-credential-"));
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }
    fn finish(report: Report) -> Value {
        let mut bytes = Vec::new();
        report.write_to(&mut bytes).unwrap();
        assert!(bytes.len() <= MAX_IO && bytes.ends_with(b"\n"));
        assert!(!String::from_utf8_lossy(&bytes).contains("SYNTHETIC_SECRET"));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["runtime_activation"], "unverified");
        v
    }
    fn change(action: &str) -> Value {
        if action == "remove" {
            json!({"action":action})
        } else {
            json!({"action":action,"value":SECRET})
        }
    }
    #[test]
    fn independent_five_vectors_pin_private_fingerprints() {
        let v: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/model-credential-reference-vectors.json"
        ))
        .unwrap();
        for item in v["metadata_vectors"].as_array().unwrap() {
            let map = item["metadata"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap().to_owned()))
                .collect();
            assert_eq!(
                metadata_values_hash(&map, item["kind"] == "file-metadata"),
                item["sha256"].as_str().unwrap()
            );
        }
        for item in v["path_vectors"].as_array().unwrap() {
            assert_eq!(
                path_hash(
                    item["kind"].as_str().unwrap(),
                    Path::new(item["exact_path"].as_str().unwrap())
                )
                .unwrap(),
                item["sha256"].as_str().unwrap()
            );
        }
        assert_eq!(
            digest(v["synthetic_secret_utf8"].as_str().unwrap().as_bytes()),
            v["synthetic_secret_sha256"].as_str().unwrap()
        );
    }
    #[test]
    fn real_install_replace_private_modes_remove_and_noop_preserve_sentinels() {
        let f = Fixture::new();
        let original = fs::read(&f.path).unwrap();
        for action in ["install", "replace", "remove"] {
            if action == "replace" {
                fs::set_permissions(&f.target, fs::Permissions::from_mode(0o400)).unwrap();
            }
            let ch = if action == "replace" {
                json!({"action":"replace","value":"SYNTHETIC_SECRET_CHANGED\r\n α "})
            } else {
                change(action)
            };
            let intent = f.prepare(ch.clone());
            let before = finish(f.run(Operation::Status, json!({"intent":intent})));
            assert_eq!(before["outcome"], "unresolved");
            let applied = finish(f.run(Operation::Apply, json!({"intent":intent,"change":ch})));
            assert_eq!(applied["outcome"], "verified");
            assert_eq!(applied["disk_readback_verified"], true);
            assert_eq!(
                finish(f.run(Operation::Status, json!({"intent":intent})))["outcome"],
                "desired-file-observed"
            );
            if let Some(s) = ch.get("value") {
                assert_eq!(fs::read(&f.target).unwrap(), s.as_str().unwrap().as_bytes());
                assert_eq!(
                    fs::metadata(&f.target).unwrap().mode() & 0o777,
                    if action == "replace" { 0o400 } else { 0o600 }
                );
            }
        }
        assert!(!f.target.exists());
        assert_eq!(fs::read(&f.path).unwrap(), original);
        f.unchanged();
        let base = observe(Some(f.path.clone())).unwrap().binding;
        assert_eq!(finish(f.run(Operation::Prepare,json!({"credential_version":1,"operation":Uuid::new_v4(),"base":base,"change":{"action":"remove"}})))["outcome"],"no-change");
        f.set(SECRET.as_bytes(), 0o600);
        let before = observe(Some(f.path.clone())).unwrap().binding;
        assert_eq!(finish(f.run(Operation::Prepare,json!({"credential_version":1,"operation":Uuid::new_v4(),"base":before,"change":change("replace")})))["outcome"],"no-change");
        assert!(observe(Some(f.path.clone())).unwrap().binding == before);
    }
    #[test]
    fn original_config_file_metadata_and_parent_changes_never_refresh() {
        for kind in ["config", "content", "inode", "mode", "parent"] {
            let f = Fixture::new();
            f.set(b"original", 0o600);
            let intent = f.prepare(change("replace"));
            match kind {
                "config" => {
                    let mut b = fs::read(&f.path).unwrap();
                    b.push(b' ');
                    fs::write(&f.path, b).unwrap();
                }
                "content" => f.set(b"intervening", 0o600),
                "inode" => f.set(b"original", 0o600),
                "mode" => {
                    fs::set_permissions(&f.target, fs::Permissions::from_mode(0o400)).unwrap()
                }
                _ => {
                    fs::rename(&f.paths.credentials, f.root.join("old-credentials")).unwrap();
                    files::directory(&f.paths.credentials).unwrap();
                    f.set(b"original", 0o600);
                }
            }
            let before = fs::read(&f.target).unwrap();
            assert_eq!(
                f.run(
                    Operation::Apply,
                    json!({"intent":intent,"change":change("replace")})
                )
                .status,
                Status::Conflict
            );
            assert_eq!(fs::read(&f.target).unwrap(), before);
        }
    }
    #[test]
    fn unsafe_modes_links_aliases_and_unconfigured_reference_refuse() {
        let f = Fixture::new();
        f.set(b"original", 0o600);
        fs::set_permissions(&f.target, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(inspect(Some(f.path.clone())).status, Status::Unsupported);
        fs::set_permissions(&f.target, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&f.target, f.root.join("alias")).unwrap();
        assert_eq!(inspect(Some(f.path.clone())).status, Status::Unsupported);
        fs::remove_file(f.root.join("alias")).unwrap();
        fs::remove_file(&f.target).unwrap();
        std::os::unix::fs::symlink(f.paths.credentials.join("other"), &f.target).unwrap();
        assert_eq!(inspect(Some(f.path.clone())).status, Status::Unsupported);
        fs::remove_file(&f.target).unwrap();
        fs::hard_link(&f.path, &f.target).unwrap();
        assert_eq!(inspect(Some(f.path.clone())).status, Status::Unsupported);
        fs::remove_file(&f.target).unwrap();
        fs::set_permissions(&f.paths.credentials, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(inspect(Some(f.path.clone())).status, Status::Unsupported);
        fs::set_permissions(&f.paths.credentials, fs::Permissions::from_mode(0o700)).unwrap();
        let mut cfg: Value = serde_json::from_slice(&fs::read(&f.path).unwrap()).unwrap();
        cfg.as_object_mut().unwrap().remove("model_credential");
        fs::write(&f.path, serde_json::to_vec(&cfg).unwrap()).unwrap();
        assert_eq!(inspect(Some(f.path.clone())).status, Status::Unsupported);
    }
    #[test]
    fn no_clobber_install_race_retains_other_writer_and_unknown() {
        let f = Fixture::new();
        let intent = f.prepare(change("install"));
        let ch: Change = serde_json::from_value(change("install")).unwrap();
        let report = publish_with(&f.path, &f.paths, &intent, &ch, |phase| {
            if phase == Phase::BeforeSyscall {
                f.set(b"race-winner", 0o600)
            }
            Ok(())
        });
        assert_eq!(finish(report)["outcome"], "unknown");
        assert_eq!(fs::read(&f.target).unwrap(), b"race-winner");
        assert_eq!(
            finish(f.run(Operation::Status, json!({"intent":intent})))["outcome"],
            "unresolved"
        );
        f.unchanged();
    }
    #[test]
    fn before_and_after_attempt_faults_are_phase_truthful_for_all_actions() {
        for action in ["install", "replace", "remove"] {
            for phase in [
                Phase::Staged,
                Phase::BeforeSyscall,
                Phase::AfterSyscall,
                Phase::BeforeSync,
                Phase::BeforeReadback,
            ] {
                let f = Fixture::new();
                if action != "install" {
                    f.set(b"original", 0o400)
                }
                let before = fs::read(&f.target).ok();
                let intent = f.prepare(change(action));
                let ch: Change = serde_json::from_value(change(action)).unwrap();
                let r = publish_with(&f.path, &f.paths, &intent, &ch, |p| {
                    if p == phase {
                        Err(Status::Unavailable)
                    } else {
                        Ok(())
                    }
                });
                let v = finish(r);
                if matches!(phase, Phase::Staged | Phase::BeforeSyscall) {
                    assert_eq!(v["outcome"], "not-dispatched");
                    assert_eq!(fs::read(&f.target).ok(), before);
                } else {
                    assert_eq!(v["outcome"], "unknown");
                    assert_eq!(
                        finish(f.run(Operation::Status, json!({"intent":intent})))["outcome"],
                        "desired-file-observed"
                    );
                }
                f.unchanged();
            }
        }
        let f = Fixture::new();
        f.set(b"original", 0o600);
        let i = f.prepare(change("replace"));
        let ch: Change = serde_json::from_value(change("replace")).unwrap();
        let r = publish_with(&f.path, &f.paths, &i, &ch, |p| {
            if p == Phase::BeforeReadback {
                f.set(b"different-readback", 0o600)
            }
            Ok(())
        });
        assert_eq!(finish(r)["outcome"], "unknown");
    }
    #[test]
    fn consuming_prefix_keeps_actual_store_busy_and_emits_only_one_prefix() {
        let f = Fixture::new();
        let intent = f.prepare(change("install"));
        let r = f.run(
            Operation::Apply,
            json!({"intent":intent,"change":change("install")}),
        );
        assert_eq!(r.status, Status::Ok);
        struct Prefix<'a> {
            f: &'a Fixture,
            bytes: Vec<u8>,
        }
        impl Write for Prefix<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                assert!(matches!(
                    Store::open_existing_for_config(self.f.paths.clone()),
                    Err(Refusal::Busy)
                ));
                if self.bytes.len() == 17 {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                let n = (17 - self.bytes.len()).min(b.len());
                self.bytes.extend_from_slice(&b[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut sink = Prefix {
            f: &f,
            bytes: Vec::new(),
        };
        assert!(r.write_to(&mut sink).is_err());
        assert_eq!(sink.bytes.len(), 17);
        assert!(Store::open_existing_for_config(f.paths.clone()).is_ok());
    }
    #[test]
    fn strict_requests_caps_and_inspect_never_read_or_create_store() {
        struct Never;
        impl Read for Never {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("inspect read stdin")
            }
        }
        assert_eq!(
            run(Operation::Inspect, None, &mut Never).status,
            Status::InvalidInput
        );
        let f = Fixture::new();
        let b = observe(Some(f.path.clone())).unwrap().binding;
        let valid = json!({"credential_version":1,"operation":Uuid::new_v4(),"base":b,"change":change("install")});
        let raw = serde_json::to_vec(&valid).unwrap();
        let mut exact = raw.clone();
        exact.resize(MAX_IO, b' ');
        assert_eq!(
            run(
                Operation::Prepare,
                Some(f.path.clone()),
                &mut exact.as_slice()
            )
            .status,
            Status::Ok
        );
        exact.push(b' ');
        assert_eq!(
            run(
                Operation::Prepare,
                Some(f.path.clone()),
                &mut exact.as_slice()
            )
            .status,
            Status::InvalidInput
        );
        for input in [
            b"[]".as_slice(),
            b"{\"intent\":[]}",
            b"{\"intent\":{},\"intent\":{}}",
            b"\xff",
        ] {
            let mut slice = input;
            assert_eq!(
                run(Operation::Apply, Some(f.path.clone()), &mut slice).status,
                Status::InvalidInput
            );
        }
        for ch in [
            json!({"action":"install","value":null}),
            json!({"action":"install","value":" \r\n "}),
            json!({"action":"remove","value":"secret"}),
            json!({"action":"install","value":"x".repeat(8193)}),
            json!({"action":"install","value":"ok","extra":true}),
        ] {
            let mut x = valid.clone();
            x["change"] = ch;
            assert_eq!(f.run(Operation::Prepare, x).status, Status::InvalidInput);
        }
    }
    #[test]
    fn actual_config_writer_cannot_interleave_credential_publication_mutex() {
        use std::{sync::mpsc, time::Duration};
        let f = Fixture::new();
        let intent = f.prepare(change("install"));
        let ch: Change = serde_json::from_value(change("install")).unwrap();
        let store = Arc::new(Store::open_existing_for_config(f.paths.clone()).unwrap());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let owner = store.clone();
        let path = f.path.clone();
        let first = std::thread::spawn(move || {
            owner.credential_publication_test_hook(&path, &intent, &ch, |phase| {
                if phase == Phase::BeforeSyscall {
                    entered_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let original = fs::read(&f.path).unwrap();
        let mut next: Value = serde_json::from_slice(&original).unwrap();
        next["debug_dump"] = json!(true);
        let next = serde_json::to_vec(&next).unwrap();
        let owner = store.clone();
        let path = f.path.clone();
        let hash = digest(&original);
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let second = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let r = owner.replace_config_document(&path, &hash, &next);
            assert!(done_tx.send(r).is_ok());
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // Barrier establishes the held publication lock; timeout is only a bounded
        // negative assertion. Actual completion is required after explicit release.
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(fs::read(&f.path).unwrap(), original);
        assert!(!f.target.exists());
        resume_tx.send(()).unwrap();
        assert_eq!(finish(first.join().unwrap())["outcome"], "verified");
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            crate::storage::ConfigPublication::Verified(_)
        ));
        second.join().unwrap();
        assert_eq!(fs::read(&f.target).unwrap(), SECRET.as_bytes());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&f.path).unwrap()).unwrap()["debug_dump"],
            true
        );
    }
}
