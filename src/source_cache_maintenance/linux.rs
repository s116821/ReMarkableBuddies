//! Header/lease custody deliberately cannot become a Store or recover its index.
use super::*;
use crate::{
    config::Config,
    retrieval::disk::validate_cleanup_entry,
    storage::{files, StorePaths, MAX_RECORD},
};
use serde::de::DeserializeOwned;
use std::{
    fs::{self, File},
    io,
    os::unix::fs::MetadataExt,
};
type Admission<T> = std::result::Result<T, Status>;
const KIND: &str = "normalized-sources-v1";
const ENTRY_BYTES: u64 = 33554432;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    format: u32,
    actor: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheOwner {
    format: u32,
    actor: Uuid,
    kind: String,
}
struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
pub(super) struct Custody {
    config_path: PathBuf,
    config: Config,
    root: PathBuf,
    directory: File,
    store: Lease,
    cache: Lease,
}
fn bad<T>(_: T) -> Status {
    Status::Unsupported
}
fn unavailable<T>(_: T) -> Status {
    Status::Unavailable
}
fn identity(meta: &fs::Metadata) -> String {
    frame(
        "buddy-cache-file-identity-v1",
        &[
            "unix".into(),
            meta.dev().to_string(),
            meta.ino().to_string(),
        ],
    )
}
#[cfg(test)]
thread_local! {
    static ACCESSES: std::cell::RefCell<Vec<(bool,PathBuf)>> = const { std::cell::RefCell::new(Vec::new()) };
    static DENY_DURABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}
fn access(path: &Path, enumerate: bool) {
    #[cfg(test)]
    DENY_DURABLE.with(|deny| {
        if deny.get() {
            assert!(
                !path.components().any(|p| matches!(
                    p.as_os_str().to_str(),
                    Some("commits" | "objects" | "quarantine" | "sync")
                )),
                "durable payload access denied"
            );
            assert!(
                !enumerate || path.file_name().is_some_and(|n| n == KIND),
                "only fixed cache namespace enumeration allowed"
            );
        }
    });
    #[cfg(test)]
    ACCESSES.with(|v| v.borrow_mut().push((enumerate, path.into())));
    #[cfg(not(test))]
    let _ = (path, enumerate);
}
fn read(path: &Path, cap: u64) -> Admission<Vec<u8>> {
    access(path, false);
    files::safe_path(path).map_err(bad)?;
    let named = fs::symlink_metadata(path).map_err(unavailable)?;
    if !named.is_file() {
        return Err(Status::Unsupported);
    }
    let f = File::open(path).map_err(unavailable)?;
    if identity(&named) != identity(&f.metadata().map_err(unavailable)?) || named.len() > cap {
        return Err(Status::Unsupported);
    }
    let mut bytes = Vec::new();
    f.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(unavailable)?;
    if bytes.len() as u64 > cap
        || identity(&named) != identity(&fs::symlink_metadata(path).map_err(unavailable)?)
    {
        return Err(Status::Unsupported);
    }
    Ok(bytes)
}
fn decoded<T: DeserializeOwned>(bytes: &[u8]) -> Admission<T> {
    let raw = inspection::strict_json(bytes).map_err(bad)?;
    serde_json::from_value(raw).map_err(bad)
}
fn object<T: DeserializeOwned>(bytes: &[u8]) -> Admission<T> {
    let raw = inspection::strict_json(bytes).map_err(bad)?;
    if !raw.is_object() {
        return Err(Status::Unsupported);
    }
    serde_json::from_value(raw).map_err(bad)
}
fn directory(path: &Path) -> Admission<File> {
    files::safe_path(path).map_err(bad)?;
    let m = fs::symlink_metadata(path).map_err(unavailable)?;
    if !m.is_dir() {
        return Err(Status::Unsupported);
    }
    let f = File::open(path).map_err(unavailable)?;
    if identity(&m) != identity(&f.metadata().map_err(unavailable)?) {
        return Err(Status::Unsupported);
    }
    Ok(f)
}
fn lease(path: &Path) -> Admission<Lease> {
    files::safe_path(path).map_err(bad)?;
    let m = fs::symlink_metadata(path).map_err(unavailable)?;
    if !m.is_file() {
        return Err(Status::Unsupported);
    }
    let f = File::open(path).map_err(unavailable)?;
    f.try_lock().map_err(|e| match e {
        fs::TryLockError::WouldBlock => Status::Busy,
        _ => Status::Unsupported,
    })?;
    let result = Lease(f);
    check_named(&result.0, path)?;
    Ok(result)
}
fn check_named(f: &File, path: &Path) -> Admission<()> {
    files::safe_path(path).map_err(bad)?;
    if identity(&f.metadata().map_err(unavailable)?)
        != identity(&fs::symlink_metadata(path).map_err(unavailable)?)
    {
        return Err(Status::Conflict);
    }
    Ok(())
}
fn canonical(path: &Path) -> Admission<PathBuf> {
    files::safe_path(path).map_err(bad)?;
    if path.exists() {
        return path.canonicalize().map_err(unavailable);
    }
    let parent = canonical(path.parent().ok_or(Status::Unsupported)?)?;
    Ok(parent.join(path.file_name().ok_or(Status::Unsupported)?))
}
fn roots(paths: &StorePaths) -> Admission<String> {
    paths.validate().map_err(bad)?;
    let roots = [
        canonical(&paths.data)?,
        canonical(&paths.cache)?,
        canonical(&paths.credentials)?,
    ];
    for (i, p) in roots.iter().enumerate() {
        for q in &roots[i + 1..] {
            if p.starts_with(q) || q.starts_with(p) {
                return Err(Status::Unsupported);
            }
        }
    }
    let text = roots
        .iter()
        .map(|p| p.to_str().map(str::to_owned).ok_or(Status::Unsupported))
        .collect::<Admission<Vec<_>>>()?;
    Ok(frame("buddy-cache-roots-v1", &text))
}
impl Custody {
    fn open(config_path: Option<PathBuf>) -> Admission<Self> {
        let path = config_path
            .filter(|p| p.is_absolute())
            .ok_or(Status::Unavailable)?;
        let bytes = read(&path, MAX_RECORD as u64)?;
        let (config, _) = inspection::document(&bytes).map_err(|s| match s {
            inspection::Status::InvalidInput => Status::InvalidInput,
            inspection::Status::Unsupported => Status::Unsupported,
            _ => Status::Unavailable,
        })?;
        roots(&config.paths)?;
        let _data = directory(&config.paths.data)?;
        let store = lease(&config.paths.data.join("store.lock"))?;
        let root = config.paths.cache.join(KIND);
        let directory = directory(&root)?;
        let cache = lease(&root.join("cache.lock"))?;
        Ok(Self {
            config_path: path,
            config,
            root,
            directory,
            store,
            cache,
        })
    }
    fn check_leases(&self) -> Admission<()> {
        check_named(&self.store.0, &self.config.paths.data.join("store.lock"))?;
        check_named(&self.cache.0, &self.root.join("cache.lock"))?;
        check_named(&self.directory, &self.root)
    }
    fn scan(&self) -> Admission<()> {
        access(&self.root, true);
        let mut slots = 0;
        for (n, item) in fs::read_dir(&self.root).map_err(unavailable)?.enumerate() {
            if n >= 18 {
                return Err(Status::Unsupported);
            }
            let item = item.map_err(unavailable)?;
            files::safe_path(&item.path()).map_err(bad)?;
            if !item.file_type().map_err(unavailable)?.is_file() {
                return Err(Status::Unsupported);
            }
            let name = item.file_name();
            let name = name.to_str().ok_or(Status::Unsupported)?;
            if matches!(name, "owner.json" | "cache.lock") {
                continue;
            }
            uuid(name.strip_suffix(".json").ok_or(Status::Unsupported)?).map_err(bad)?;
            slots += 1;
            if slots > 16 {
                return Err(Status::Unsupported);
            }
        }
        Ok(())
    }
    fn headers(&self, req: &Request) -> Admission<Intent> {
        self.check_leases()?;
        let path = canonical(&self.config_path)?;
        let config = read(&self.config_path, MAX_RECORD as u64)?;
        let (current, _) = inspection::document(&config).map_err(bad)?;
        if current.paths.data != self.config.paths.data
            || current.paths.cache != self.config.paths.cache
            || current.paths.credentials != self.config.paths.credentials
        {
            return Err(Status::Conflict);
        }
        let store_owner = read(&self.config.paths.data.join("identity.json"), 1024)?;
        let owner: Owner = object(&store_owner)?;
        if owner.format != 1 || owner.actor.is_nil() {
            return Err(Status::Unsupported);
        }
        let generation: Uuid = decoded(&read(&self.config.paths.data.join("CURRENT"), 128)?)?;
        if generation.is_nil() {
            return Err(Status::Unsupported);
        }
        let format: u32 = decoded(&read(
            &self
                .config
                .paths
                .data
                .join("generations")
                .join(generation.to_string())
                .join("format.json"),
            64,
        )?)?;
        if format != 1 {
            return Err(Status::Unsupported);
        }
        let cache_owner = read(&self.config.paths.cache.join("owner.json"), 1024)?;
        let parent: Owner = object(&cache_owner)?;
        let namespace_owner = read(&self.root.join("owner.json"), 1024)?;
        let namespace: CacheOwner = object(&namespace_owner)?;
        if parent.format != 1
            || parent.actor != owner.actor
            || namespace.format != 1
            || namespace.actor != owner.actor
            || namespace.kind != KIND
        {
            return Err(Status::Unsupported);
        }
        let result = Intent {
            intent_version: 1,
            operation: req.operation.to_string(),
            document: req.document.to_string(),
            config_path_sha256: frame(
                "buddy-cache-config-path-v1",
                &[path.to_str().ok_or(Status::Unsupported)?.into()],
            ),
            config_bytes_sha256: digest(&config),
            roots_sha256: roots(&self.config.paths)?,
            actor: owner.actor.to_string(),
            generation: generation.to_string(),
            store_owner_sha256: digest(&store_owner),
            cache_owner_sha256: digest(&cache_owner),
            namespace_owner_sha256: digest(&namespace_owner),
            namespace_identity_sha256: identity(&self.directory.metadata().map_err(unavailable)?),
            entry_identity_sha256: String::new(),
            entry_bytes: 0,
            entry_sha256: String::new(),
            intent_sha256: String::new(),
        };
        Ok(result)
    }
    fn observe(&self, req: &Request) -> Admission<(Intent, bool)> {
        let mut result = self.headers(req)?;
        self.scan()?;
        let selected = self.root.join(format!("{}.json", req.document));
        let present = match fs::symlink_metadata(&selected) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(_) => return Err(Status::Unavailable),
            Ok(before) => {
                let bytes = read(&selected, ENTRY_BYTES)?;
                validate_cleanup_entry(&bytes, req.document).map_err(bad)?;
                let after = fs::symlink_metadata(&selected).map_err(unavailable)?;
                if identity(&before) != identity(&after) {
                    return Err(Status::Conflict);
                }
                result.entry_identity_sha256 = identity(&after);
                result.entry_bytes = bytes.len() as u64;
                result.entry_sha256 = digest(&bytes);
                true
            }
        };
        result.intent_sha256 = result.hash();
        // Header-only readback after payload validation; no durable traversal.
        if !result.same_headers(&self.headers(req)?) {
            return Err(Status::Conflict);
        }
        self.check_leases()?;
        Ok((result, present))
    }
}
pub(super) fn run(operation: Operation, config: Option<PathBuf>, req: Request) -> Report {
    let mut r = Report::new(operation, Status::Unavailable);
    r.document = Some(req.document);
    let custody = match Custody::open(config) {
        Ok(c) => c,
        Err(s) => {
            r.status = s;
            return r;
        }
    };
    let first = match custody.observe(&req) {
        Ok(v) => v,
        Err(s) => {
            r.status = s;
            r.custody = Some(custody);
            return r;
        }
    };
    // Re-read every bound header and the selected file under both held leases.
    let (current, present) = match custody.observe(&req) {
        Ok(v) => v,
        Err(s) => {
            r.status = s;
            r.custody = Some(custody);
            return r;
        }
    };
    if first.0.intent_sha256 != current.intent_sha256 || first.1 != present {
        r.status = Status::Conflict;
    } else if req
        .original
        .as_ref()
        .is_some_and(|o| !o.same_headers(&current))
    {
        r.status = Status::Conflict;
    } else if !present {
        r.status = Status::Absent;
    } else {
        r.entry_bytes = Some(current.entry_bytes);
        r.entry_sha256 = Some(current.entry_sha256.clone());
        match operation {
            Operation::Inspect => r.status = Status::Ready,
            Operation::Prepare => {
                r.status = Status::Prepared;
                r.intent = Some(current);
            }
            Operation::Status | Operation::Apply => {
                let original = req.original.as_ref().expect("parsed original");
                if original.intent_sha256 != current.intent_sha256 {
                    r.status = Status::Conflict;
                } else if operation == Operation::Status {
                    r.status = Status::Unchanged;
                } else {
                    r.possible_effect = true;
                    r.status = Status::Unknown;
                    let selected = custody.root.join(format!("{}.json", req.document));
                    if fs::remove_file(&selected).is_ok() {
                        #[cfg(test)]
                        let fail_sync = FAULT.with(|v| v.get() == 1);
                        #[cfg(not(test))]
                        let fail_sync = false;
                        if !fail_sync && files::sync_dir(&custody.root).is_ok() {
                            #[cfg(test)]
                            let fail_readback = FAULT.with(|v| v.get() == 2);
                            #[cfg(not(test))]
                            let fail_readback = false;
                            if !fail_readback
                                && matches!(fs::symlink_metadata(&selected),Err(e) if e.kind()==io::ErrorKind::NotFound)
                                && custody.check_leases().is_ok()
                            {
                                r.status = Status::Applied;
                            }
                        }
                    }
                }
            }
        }
    }
    r.custody = Some(custody);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::{Index, NormalizedDocument, SourceFormat, SourceUnit};
    use serde_json::json;
    struct Fixture {
        root: PathBuf,
        config: PathBuf,
        paths: StorePaths,
        document: Uuid,
        generation: Uuid,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("buddy-cache-maintenance-{}", Uuid::new_v4()));
            let paths = StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            };
            let actor = Uuid::new_v4();
            let generation = Uuid::new_v4();
            let document = Uuid::new_v4();
            let gen = paths.data.join("generations").join(generation.to_string());
            fs::create_dir_all(gen.join("commits")).unwrap();
            fs::create_dir_all(gen.join("objects")).unwrap();
            fs::write(
                gen.join("commits/poison.json"),
                b"not a commit; must never be parsed",
            )
            .unwrap();
            fs::write(gen.join("objects/sentinel"), b"durable private sentinel").unwrap();
            let ns = paths.cache.join(KIND);
            fs::create_dir_all(&ns).unwrap();
            for p in [
                &paths.data.join("identity.json"),
                &paths.cache.join("owner.json"),
            ] {
                fs::write(
                    p,
                    serde_json::to_vec(&json!({"format":1,"actor":actor})).unwrap(),
                )
                .unwrap();
            }
            fs::write(
                paths.data.join("CURRENT"),
                serde_json::to_vec(&generation).unwrap(),
            )
            .unwrap();
            fs::write(gen.join("format.json"), b"1").unwrap();
            fs::write(paths.data.join("store.lock"), []).unwrap();
            fs::write(ns.join("cache.lock"), []).unwrap();
            fs::write(
                ns.join("owner.json"),
                serde_json::to_vec(&json!({"format":1,"actor":actor,"kind":KIND})).unwrap(),
            )
            .unwrap();
            fs::write(root.join("original.pdf"), b"original must survive").unwrap();
            let material = NormalizedDocument {
                document,
                format: SourceFormat::Pdf,
                source_sha256: digest(b"synthetic"),
                extractor_revision: "synthetic-v1".into(),
                units: vec![SourceUnit {
                    page: Some(1),
                    section: None,
                    text: "private normalized source sentinel".into(),
                }],
            };
            let index = Index::build(material.clone()).unwrap();
            fs::write(
                ns.join(format!("{document}.json")),
                serde_json::to_vec(&json!({"format":1,"key":index.key(),"document":material}))
                    .unwrap(),
            )
            .unwrap();
            let config = root.join("config.json");
            let c = Config {
                paths: paths.clone(),
                ..Config::default()
            };
            fs::write(&config, serde_json::to_vec(&c).unwrap()).unwrap();
            DENY_DURABLE.with(|v| v.set(true));
            ACCESSES.with(|v| v.borrow_mut().clear());
            Self {
                root,
                config,
                paths,
                document,
                generation,
            }
        }
        fn slot(&self) -> PathBuf {
            self.paths
                .cache
                .join(KIND)
                .join(format!("{}.json", self.document))
        }
        fn call(&self, op: Operation, v: serde_json::Value) -> Report {
            super::super::run(
                op,
                Path::new("-"),
                Some(self.config.clone()),
                &mut serde_json::to_vec(&v).unwrap().as_slice(),
            )
        }
        fn prepare(&self) -> Intent {
            let r = self.call(
                Operation::Prepare,
                json!({"request_version":1,"operation":Uuid::new_v4(),"document":self.document}),
            );
            assert_eq!(r.status, Status::Prepared);
            r.intent.clone().unwrap()
        }
        fn confirmed(&self, op: Operation, i: &Intent) -> Report {
            self.call(op, json!({"request_version":1,"intent":i}))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            DENY_DURABLE.with(|v| v.set(false));
            FAULT.with(|v| v.set(0));
            if !std::thread::panicking() {
                let root = self.root.canonicalize().unwrap();
                assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
                assert!(root
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("buddy-cache-maintenance-"));
                fs::remove_dir_all(root).unwrap();
            }
        }
    }
    fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, (Vec<u8>, u32)> {
        use std::os::unix::fs::PermissionsExt;
        let mut map = std::collections::BTreeMap::new();
        for e in fs::read_dir(root).unwrap() {
            let p = e.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                map.extend(snapshot(&p));
            } else if m.is_file() {
                map.insert(p, (fs::read(&p).unwrap(), m.permissions().mode()));
            }
        }
        map
    }
    #[test]
    fn preview_apply_status_preserve_every_other_file_and_never_recover() {
        let f = Fixture::new();
        let before = snapshot(&f.root);
        let i = f.prepare();
        assert_eq!(snapshot(&f.root), before);
        assert_eq!(f.confirmed(Operation::Status, &i).status, Status::Unchanged);
        let r = f.confirmed(Operation::Apply, &i);
        assert_eq!(r.status, Status::Applied);
        assert!(r.possible_effect);
        drop(r);
        let mut expected = before;
        expected.remove(&f.slot());
        assert_eq!(snapshot(&f.root), expected);
        assert_eq!(f.confirmed(Operation::Status, &i).status, Status::Absent);
        assert_eq!(f.confirmed(Operation::Apply, &i).status, Status::Absent);
        ACCESSES.with(|v| {
            let a = v.borrow();
            assert!(!a.is_empty());
            assert!(a
                .iter()
                .all(|(enumerate, p)| !enumerate || p == &f.paths.cache.join(KIND)));
            assert!(a.iter().all(|(_, p)| !p.starts_with(&f.paths.credentials)));
        });
    }
    #[test]
    fn dedicated_admission_does_not_need_durable_payload_directories() {
        let f = Fixture::new();
        let gen = f
            .paths
            .data
            .join("generations")
            .join(f.generation.to_string());
        // These are exclusively generated fixture trees and remain inside our root.
        for child in ["commits", "objects"] {
            let p = gen.join(child).canonicalize().unwrap();
            assert!(p.starts_with(f.root.canonicalize().unwrap()));
            fs::remove_dir_all(p).unwrap();
        }
        let i = f.prepare();
        assert_eq!(f.confirmed(Operation::Apply, &i).status, Status::Applied);
    }
    #[test]
    fn changed_config_marker_generation_and_file_identity_refuse() {
        for which in 0..5 {
            let f = Fixture::new();
            let i = f.prepare();
            match which {
                0 => {
                    let mut b = fs::read(&f.config).unwrap();
                    b.push(b' ');
                    fs::write(&f.config, b).unwrap();
                }
                1 => {
                    let p = f.paths.cache.join(KIND).join("owner.json");
                    let mut b = fs::read(&p).unwrap();
                    b.push(b' ');
                    fs::write(p, b).unwrap();
                }
                2 => {
                    let p = f.paths.data.join("identity.json");
                    let mut b = fs::read(&p).unwrap();
                    b.push(b' ');
                    fs::write(p, b).unwrap();
                }
                3 => {
                    let replacement = f.root.join("replacement");
                    fs::write(&replacement, fs::read(f.slot()).unwrap()).unwrap();
                    fs::rename(replacement, f.slot()).unwrap();
                }
                _ => {
                    fs::write(
                        f.paths.data.join("CURRENT"),
                        serde_json::to_vec(&Uuid::new_v4()).unwrap(),
                    )
                    .unwrap();
                }
            }
            let r = f.confirmed(Operation::Apply, &i);
            assert!(matches!(r.status, Status::Conflict | Status::Unavailable));
            assert!(!r.possible_effect);
            assert!(f.slot().exists());
        }
    }
    #[test]
    fn unknown_staging_unsafe_and_bad_material_preserved() {
        for which in 0..6 {
            let f = Fixture::new();
            let i = f.prepare();
            let ns = f.paths.cache.join(KIND);
            match which {
                0 => fs::write(ns.join("unknown"), b"sentinel").unwrap(),
                1 => fs::write(ns.join(format!(".stage-{}", Uuid::new_v4())), b"sentinel").unwrap(),
                2 => {
                    fs::remove_file(f.slot()).unwrap();
                    std::os::unix::fs::symlink(f.root.join("original.pdf"), f.slot()).unwrap();
                }
                3 => fs::write(f.slot(), b"{}").unwrap(),
                4 => {
                    let mut v: serde_json::Value =
                        serde_json::from_slice(&fs::read(f.slot()).unwrap()).unwrap();
                    v["format"] = json!(2);
                    fs::write(f.slot(), serde_json::to_vec(&v).unwrap()).unwrap();
                }
                _ => {
                    let file = File::options().write(true).open(f.slot()).unwrap();
                    file.set_len(ENTRY_BYTES + 1).unwrap();
                }
            }
            assert_eq!(
                f.confirmed(Operation::Apply, &i).status,
                Status::Unsupported
            );
            assert!(fs::symlink_metadata(f.slot()).is_ok());
        }
    }
    #[test]
    fn namespace_or_store_writer_and_missing_lock_never_initialize() {
        for store in [true, false] {
            let f = Fixture::new();
            let path = if store {
                f.paths.data.join("store.lock")
            } else {
                f.paths.cache.join(KIND).join("cache.lock")
            };
            let held = lease(&path).unwrap();
            assert_eq!(
                f.call(
                    Operation::Inspect,
                    json!({"request_version":1,"document":f.document})
                )
                .status,
                Status::Busy
            );
            drop(held);
            fs::remove_file(&path).unwrap();
            assert_eq!(
                f.call(
                    Operation::Inspect,
                    json!({"request_version":1,"document":f.document})
                )
                .status,
                Status::Unavailable
            );
            assert!(!path.exists());
        }
    }
    #[test]
    fn post_unlink_uncertainty_is_never_rollback_or_retry() {
        for fault in [1, 2] {
            let f = Fixture::new();
            let i = f.prepare();
            FAULT.with(|v| v.set(fault));
            let r = f.confirmed(Operation::Apply, &i);
            assert_eq!(r.status, Status::Unknown);
            assert!(r.possible_effect);
            drop(r);
            assert!(!f.slot().exists());
            FAULT.with(|v| v.set(0));
            assert_eq!(f.confirmed(Operation::Status, &i).status, Status::Absent);
        }
    }
    #[test]
    fn consuming_output_keeps_both_leases_through_partial_failure() {
        let f = Fixture::new();
        let i = f.prepare();
        let r = f.confirmed(Operation::Apply, &i);
        struct Fail<'a> {
            f: &'a Fixture,
            calls: usize,
        }
        impl Write for Fail<'_> {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                assert!(matches!(
                    lease(&self.f.paths.data.join("store.lock")),
                    Err(Status::Busy)
                ));
                assert!(matches!(
                    lease(&self.f.paths.cache.join(KIND).join("cache.lock")),
                    Err(Status::Busy)
                ));
                self.calls += 1;
                if self.calls == 1 {
                    Ok(b.len().min(7))
                } else {
                    Err(io::Error::other("synthetic output failure"))
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = Fail { f: &f, calls: 0 };
        assert!(r.write_to(&mut output).is_err());
        assert_eq!(output.calls, 2);
        assert!(lease(&f.paths.data.join("store.lock")).is_ok());
        assert!(lease(&f.paths.cache.join(KIND).join("cache.lock")).is_ok());
    }
}
