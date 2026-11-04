//! Inject as an example into unchanged published 68e42968 source for offline qualification.
//! This is an uncompiled review candidate, not execution evidence.
use remarkable_reader_buddy::{
    config::SyncPolicy,
    conversation::Ledger,
    storage::{
        drive::*,
        files,
        sync::{Status, SyncEngine, Worker},
        Namespace, Store, StorePaths, Uuid,
    },
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Cloud {
    files: BTreeMap<String, (RemoteFile, Vec<u8>)>,
    calls: usize,
    uploads: usize,
    allocated: usize,
    block_authorize: bool,
    entered: bool,
    released: bool,
}
#[derive(Clone, Default)]
struct MemoryDrive(Arc<(Mutex<Cloud>, Condvar)>);
impl DriveTransport for MemoryDrive {
    fn authorize(&mut self, _: Uuid) -> anyhow::Result<()> {
        let mut c = self.0 .0.lock().unwrap();
        c.calls += 1;
        if c.block_authorize {
            c.entered = true;
            self.0 .1.notify_all();
            while !c.released {
                c = self.0 .1.wait(c).unwrap();
            }
        }
        Ok(())
    }
    fn start_token(&mut self) -> anyhow::Result<String> {
        self.0 .0.lock().unwrap().calls += 1;
        Ok("0".into())
    }
    fn list(&mut self, page: Option<&str>) -> anyhow::Result<Page<RemoteFile>> {
        anyhow::ensure!(page.is_none(), "single bounded fixture page");
        let mut c = self.0 .0.lock().unwrap();
        c.calls += 1;
        Ok(Page {
            items: c.files.values().map(|(f, _)| f.clone()).collect(),
            next: None,
            checkpoint: None,
        })
    }
    fn changes(&mut self, page: &str) -> anyhow::Result<Page<Change>> {
        anyhow::ensure!(page == "0", "fixture cursor");
        self.0 .0.lock().unwrap().calls += 1;
        // Outbound-only positive control: no echoed changes or inbound-sync claim.
        Ok(Page {
            items: vec![],
            next: None,
            checkpoint: Some("0".into()),
        })
    }
    fn download(&mut self, id: &str, limit: u64) -> anyhow::Result<Vec<u8>> {
        let mut c = self.0 .0.lock().unwrap();
        c.calls += 1;
        let bytes = &c
            .files
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("fixture object absent"))?
            .1;
        anyhow::ensure!(bytes.len() as u64 <= limit, "fixture byte limit");
        Ok(bytes.clone())
    }
    fn allocate(&mut self) -> anyhow::Result<String> {
        let mut c = self.0 .0.lock().unwrap();
        c.calls += 1;
        c.allocated += 1;
        anyhow::ensure!(c.allocated <= 32, "fixture allocation bound");
        Ok(format!("fixture-{}", c.allocated))
    }
    fn upload(
        &mut self,
        id: &str,
        tag: &Tag,
        source: &mut files::Source,
        _: &mut UploadSession,
    ) -> anyhow::Result<UploadProgress> {
        let bytes = source.chunk(0, source.len())?;
        anyhow::ensure!(
            remarkable_reader_buddy::storage::digest(&bytes) == tag.sha256,
            "actual outbound bytes match tag"
        );
        let mut c = self.0 .0.lock().unwrap();
        c.calls += 1;
        c.uploads += 1;
        if let Some((prior, content)) = c.files.get(id) {
            anyhow::ensure!(
                prior.tag()? == *tag && content == &bytes,
                "fixture idempotent upload"
            );
        } else {
            let file = RemoteFile {
                id: id.into(),
                trashed: false,
                app_properties: BTreeMap::from([
                    ("buddy_protocol".into(), "1".into()),
                    ("collection".into(), tag.collection.to_string()),
                    ("kind".into(), tag.kind.clone()),
                    ("sha256".into(), tag.sha256.clone()),
                ]),
            };
            c.files.insert(id.into(), (file, bytes));
        }
        Ok(UploadProgress::Complete)
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 4,
        "expected action, isolated root and conversation"
    );
    let root = PathBuf::from(&args[2]);
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    let conversation: Uuid = args[3].parse()?;
    if args[1] == "refuse-reopen" {
        let generation: Uuid = serde_json::from_slice(&std::fs::read(paths.data.join("CURRENT"))?)?;
        let format: u32 = serde_json::from_slice(&std::fs::read(
            paths
                .data
                .join("generations")
                .join(generation.to_string())
                .join("format.json"),
        )?)?;
        anyhow::ensure!(format == 4, "target must actually be adopted format4");
        let error = match Store::open(paths) {
            Ok(_) => anyhow::bail!("predecessor admitted format4"),
            Err(error) => error,
        };
        anyhow::ensure!(
            error
                .chain()
                .any(|cause| cause.to_string() == "unsupported generation format"),
            "refusal must be the pinned predecessor format fence: {error:#}"
        );
        println!(
            "{}",
            json!({"action":"refuse-reopen","observed_format":format,"refused":true,"error":error.to_string(),"handler_not_constructed":true})
        );
        return Ok(());
    }
    anyhow::ensure!(
        args[1] == "positive-controls-and-hold",
        "unsupported driver action"
    );
    let store = Arc::new(Store::open(paths)?);
    let ledger = Ledger::new(store.clone());
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    drop(ledger);
    let manifests = store.manifests()?;
    anyhow::ensure!(
        !manifests.is_empty(),
        "positive control requires committed ordinary history"
    );
    for manifest in &manifests {
        let mut objects = BTreeMap::new();
        for reference in manifest.records.iter().chain(&manifest.media) {
            objects.insert(reference.sha256.clone(), store.read_object(reference)?);
        }
        store.import(manifest.clone(), objects)?;
        // All verified objects already exist; import_staged must preserve idempotence.
        store.import_staged(manifest.clone(), &root.join("unused-incoming"))?;
    }
    let archive = root.join("ordinary-positive-control-backup");
    anyhow::ensure!(
        store.export(&archive)?,
        "positive archive must retain complete media"
    );
    Store::inspect_backup(&archive)?;
    store.restore(&archive)?;
    let drive = MemoryDrive::default();
    let policy = SyncPolicy {
        enabled: true,
        namespaces: BTreeSet::from([Namespace::Conversation]),
        collection: Some(Uuid::new_v4()),
        create_new: true,
        ..Default::default()
    };
    let mut engine = SyncEngine::new(
        store.clone(),
        policy.clone(),
        drive.clone(),
        Arc::new(AtomicBool::new(false)),
    )?;
    let mut steps = 0;
    let mut current = false;
    for _ in 0..8 {
        steps += 1;
        if engine.step()? == Status::Current {
            current = true;
            break;
        }
    }
    anyhow::ensure!(
        current,
        "actual old outbox must complete bounded positive control"
    );
    drop(engine);
    {
        let mut c = drive.0 .0.lock().unwrap();
        anyhow::ensure!(
            c.uploads > 0
                && c.files
                    .values()
                    .any(|(f, _)| f.tag().is_ok_and(|t| t.kind == "manifest")),
            "actual manifest publication required"
        );
        c.block_authorize = true;
    }
    let worker = Worker::start(store.clone(), policy, drive.clone())?
        .ok_or_else(|| anyhow::anyhow!("actual old worker refused valid control"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let observed = {
        let mut c = drive.0 .0.lock().unwrap();
        while !c.entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "worker authorization boundary timeout"
            );
            c = drive.0 .1.wait_timeout(c, remaining).unwrap().0;
        }
        json!({"engine_steps":steps,"transport_calls":c.calls,"uploads":c.uploads,
               "manifest_objects":c.files.values().filter(|(f,_)| f.tag().is_ok_and(|t| t.kind == "manifest")).count(),
               "worker_authorize_entered":c.entered,"transport":"pure-memory outbound fixture; no inbound feed or account"})
    };
    println!(
        "{}",
        json!({"action":"positive-controls-and-hold","ready":true,"conversation":conversation,"import":true,"import_staged":true,"ordinary_export":true,"ordinary_restore":true,"sync_positive_control":observed})
    );
    io::stdout().flush()?;
    let mut release = String::new();
    io::stdin().lock().read_line(&mut release)?;
    anyhow::ensure!(
        release.trim() == "release",
        "explicit coordinator release required"
    );
    {
        let mut c = drive.0 .0.lock().unwrap();
        c.released = true;
        drive.0 .1.notify_all();
    }
    drop(worker);
    anyhow::ensure!(
        Arc::strong_count(&store) == 1,
        "worker must release actual Store ownership before closed receipt"
    );
    drop(store);
    println!("{}", json!({"action":"released","closed":true}));
    Ok(())
}
