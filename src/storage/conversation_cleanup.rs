//! New bounded existing-only recovery and conditional conversation transaction.
//! Legacy recovery/publication callers are unchanged.
use super::*;
use crate::{
    config::inspection,
    conversation_maintenance::{Intent, Receipt},
};
use std::{fs::Metadata, io::Read};
const MIB: u64 = 1024 * 1024;
#[derive(Debug)]
pub(crate) struct WorkExhausted(&'static str);
impl std::fmt::Display for WorkExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for WorkExhausted {}
#[derive(Clone, Default, Debug)]
pub(crate) struct Budget {
    pub bytes: u64,
    pub visits: u64,
    exhausted: bool,
}
impl Budget {
    pub(crate) fn charge(&mut self, bytes: u64, visits: u64) -> Result<()> {
        if self.exhausted {
            return Err(WorkExhausted("conversation recovery budget exhausted").into());
        }
        match (
            self.bytes.checked_add(bytes),
            self.visits.checked_add(visits),
        ) {
            (Some(b), Some(v)) if b <= 512 * MIB && v <= 65536 => {
                self.bytes = b;
                self.visits = v;
                Ok(())
            }
            _ => {
                self.exhausted = true;
                Err(WorkExhausted("conversation recovery work exhausted").into())
            }
        }
    }
}
#[derive(Clone)]
struct Fact {
    path: PathBuf,
    len: u64,
    identity: (u64, u64),
    modified: Option<std::time::SystemTime>,
}
fn file(path: &Path) -> Result<Fact> {
    files::safe_path(path)?;
    let m = fs::symlink_metadata(path)?;
    ensure!(m.is_file(), "regular file required");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(m.nlink() == 1, "hardlink refused");
    }
    Ok(Fact {
        path: path.into(),
        len: m.len(),
        identity: identity(&m),
        modified: m.modified().ok(),
    })
}
fn identity(m: &Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    }
    #[cfg(not(unix))]
    {
        (m.len(), 0)
    }
}
impl Fact {
    fn check(&self) -> Result<()> {
        let n = file(&self.path)?;
        ensure!(
            self.len == n.len && self.identity == n.identity && self.modified == n.modified,
            "file identity changed"
        );
        Ok(())
    }
    fn read(&self, budget: &mut Budget, limit: u64) -> Result<Vec<u8>> {
        ensure!(self.len <= limit, "file bound");
        budget.charge(self.len, 1)?;
        self.check()?;
        let mut f = File::open(&self.path)?;
        ensure!(
            identity(&f.metadata()?) == self.identity,
            "held file changed"
        );
        let mut b = Vec::new();
        f.by_ref().take(self.len).read_to_end(&mut b)?;
        ensure!(b.len() as u64 == self.len, "short read");
        self.check()?;
        Ok(b)
    }
    fn verify(&self, budget: &mut Budget, sha: &str) -> Result<()> {
        use sha2::{Digest, Sha256};
        budget.charge(self.len, 1)?;
        self.check()?;
        let f = File::open(&self.path)?;
        ensure!(
            identity(&f.metadata()?) == self.identity,
            "held object changed"
        );
        let mut reader = f.take(self.len);
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut n = 0;
        loop {
            let got = reader.read(&mut buffer)?;
            if got == 0 {
                break;
            }
            n += got as u64;
            hash.update(&buffer[..got]);
        }
        ensure!(
            n == self.len && format!("{:x}", hash.finalize()) == sha,
            "object digest"
        );
        self.check()?;
        Ok(())
    }
}
#[derive(Clone, Default)]
struct Inventory {
    commits: u64,
    objects: u64,
    manifest_bytes: u64,
    object_bytes: u64,
    record_bytes: u64,
    files: BTreeMap<String, Fact>,
    manifests: Vec<Manifest>,
    records: BTreeMap<String, Envelope>,
    manifest_sha256: BTreeMap<Uuid, String>,
}
fn add(v: &mut u64, n: u64, max: u64) -> Result<()> {
    *v = v.checked_add(n).context("inventory overflow")?;
    ensure!(*v <= max, "inventory bound");
    Ok(())
}
fn staged(name: &str) -> bool {
    name.strip_prefix(".stage-")
        .is_some_and(|s| Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s && !u.is_nil()))
}
fn inventory(
    store: &Store,
    generation: Uuid,
    budget: &mut Budget,
    known: &BTreeMap<String, u64>,
) -> Result<Inventory> {
    let root = store.generation(generation);
    let mut inv = Inventory::default();
    for e in fs::read_dir(&root)? {
        budget.charge(0, 1)?;
        let e = e?;
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 entry"))?;
        if ["format.json", "commits", "objects"].contains(&name.as_str()) {
            continue;
        }
        ensure!(staged(&name), "unknown generation entry");
        let f = file(&e.path())?;
        add(&mut inv.commits, 1, 4096)?;
        add(&mut inv.manifest_bytes, f.len, 8 * MIB)?;
    }
    let mut manifest_files = Vec::new();
    for e in fs::read_dir(root.join("commits"))? {
        budget.charge(0, 1)?;
        let e = e?;
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("commit name"))?;
        let f = file(&e.path())?;
        add(&mut inv.commits, 1, 4096)?;
        add(&mut inv.manifest_bytes, f.len, 8 * MIB)?;
        if staged(&name) {
            continue;
        }
        let id = name.strip_suffix(".json").context("commit name")?;
        ensure!(
            Uuid::parse_str(id).is_ok_and(|u| !u.is_nil() && u.to_string() == id),
            "commit name"
        );
        manifest_files.push((id.to_owned(), f));
    }
    for e in fs::read_dir(root.join("objects"))? {
        budget.charge(0, 1)?;
        let e = e?;
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("object name"))?;
        let f = file(&e.path())?;
        add(&mut inv.objects, 1, 8192)?;
        add(&mut inv.object_bytes, f.len, 256 * MIB)?;
        if staged(&name) {
            add(&mut inv.record_bytes, f.len, 32 * MIB)?;
            continue;
        }
        ensure!(valid_digest(&name), "object name");
        inv.files.insert(name, f);
    }
    manifest_files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut refs = BTreeMap::<String, (u64, bool)>::new();
    for (id, f) in manifest_files {
        let bytes = f.read(budget, 8 * MIB)?;
        inspection::strict_json(&bytes)?;
        let m: Manifest = serde_json::from_slice(&bytes)?;
        let n = m.records.len() as u64 + m.media.len() as u64 + m.media_coverage.len() as u64;
        budget.charge(
            0,
            n.checked_mul(n)
                .context("manifest work overflow")?
                .checked_add(1)
                .context("manifest work overflow")?,
        )?;
        m.validate()?;
        ensure!(m.transaction_id.to_string() == id, "manifest identity");
        ensure!(
            m.scope == Scope::LocalTransaction,
            "nonordinary transaction"
        );
        for (r, record) in m
            .records
            .iter()
            .map(|r| (r, true))
            .chain(m.media.iter().map(|r| (r, false)))
        {
            budget.charge(0, 1)?;
            let f = inv
                .files
                .get(&r.sha256)
                .context("missing committed object")?;
            ensure!(r.bytes == f.len, "object size");
            if let Some(prior) = refs.insert(r.sha256.clone(), (r.bytes, record)) {
                ensure!(prior == (r.bytes, record), "inconsistent reference");
            }
        }
        inv.manifest_sha256.insert(m.transaction_id, digest(&bytes));
        inv.manifests.push(m);
    }
    for (sha, f) in &inv.files {
        budget.charge(0, 1)?;
        match refs.get(sha) {
            Some((_, true)) => {
                ensure!(f.len <= MIB, "record bound");
                add(&mut inv.record_bytes, f.len, 32 * MIB)?;
            }
            Some((_, false)) => {}
            None => {
                ensure!(known.get(sha) == Some(&f.len), "unknown uncommitted object");
                add(&mut inv.record_bytes, f.len, 32 * MIB)?;
            }
        }
    }
    // All inventory/declared-length budgets were checked BEFORE object content reads.
    for (sha, f) in &inv.files {
        if refs.get(sha).is_some_and(|(_, r)| *r) || known.contains_key(sha) {
            let b = f.read(budget, MIB)?;
            ensure!(digest(&b) == *sha, "record digest");
            inspection::strict_json(&b)?;
            let e: Envelope = serde_json::from_slice(&b)?;
            budget.charge(0, (e.parents.len() + e.media_descriptors.len()) as u64)?;
            e.validate()?;
            inv.records.insert(sha.clone(), e);
        } else {
            f.verify(budget, sha)?;
        }
    }
    Ok(inv)
}
fn validate_records(
    inv: &Inventory,
    m: &Manifest,
    budget: &mut Budget,
) -> Result<Vec<(Envelope, String)>> {
    let mut out = Vec::new();
    let mut descriptors = BTreeMap::new();
    for r in &m.records {
        budget.charge(0, 1)?;
        let e = inv.records.get(&r.sha256).context("record unavailable")?;
        ensure!(
            m.record_namespaces.get(&r.sha256) == Some(&e.namespace),
            "namespace mismatch"
        );
        for media in &e.media_descriptors {
            budget.charge(0, 1)?;
            if let Some(old) = descriptors.insert(media.sha256.clone(), media.clone()) {
                ensure!(old == *media, "inconsistent media");
            }
        }
        out.push((e.clone(), r.sha256.clone()));
    }
    ensure!(
        descriptors
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            == m.media_coverage.iter().map(Coverage::hash).collect(),
        "coverage descriptors"
    );
    for r in &m.media {
        budget.charge(0, 1)?;
        ensure!(
            descriptors
                .get(&r.sha256)
                .is_some_and(|d| d.bytes == r.bytes),
            "media size"
        );
    }
    Ok(out)
}
fn can_apply(index: &Index, records: &[(Envelope, String)], budget: &mut Budget) -> Result<()> {
    let batch: BTreeMap<_, _> = records
        .iter()
        .map(|(e, h)| (e.revision_id, (e, h)))
        .collect();
    ensure!(batch.len() == records.len(), "duplicate revision");
    let mut ops = BTreeMap::new();
    for (e, h) in records {
        budget.charge(0, 1)?;
        if let Some(old) = index.records.get(&e.revision_id) {
            ensure!(&old.1 == h, "revision collision");
        }
        if let Some(old) = index
            .operations
            .get(&e.operation_id)
            .or_else(|| ops.get(&e.operation_id))
        {
            ensure!(old == h, "operation collision");
        }
        ops.insert(e.operation_id, h.clone());
        let mut stack: Vec<_> = e.parents.iter().copied().collect();
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            budget.charge(0, 1)?;
            ensure!(id != e.revision_id, "cyclic lineage");
            if !seen.insert(id) {
                continue;
            }
            let p = index
                .records
                .get(&id)
                .map(|p| &p.0)
                .or_else(|| batch.get(&id).map(|p| p.0))
                .context("pending dependency")?;
            ensure!(
                p.namespace == e.namespace && p.record_id == e.record_id,
                "foreign parent"
            );
            stack.extend(p.parents.iter().copied());
        }
    }
    Ok(())
}
fn rebuild(inv: &Inventory, budget: &mut Budget) -> Result<Index> {
    let mut index = Index::default();
    let mut pending = Vec::new();
    for m in &inv.manifests {
        pending.push((m.clone(), validate_records(inv, m, budget)?));
    }
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for (m, r) in pending {
            budget.charge(0, 1)?;
            match can_apply(&index, &r, budget) {
                Ok(()) => {
                    let visits = index
                        .records
                        .values()
                        .chain(r.iter())
                        .map(|(e, _)| 2 + e.parents.len() as u64)
                        .sum::<u64>();
                    budget.charge(0, visits)?;
                    Store::apply_index(&mut index, m, r);
                }
                Err(e) => {
                    if budget.visits >= 65536 {
                        return Err(e);
                    }
                    next.push((m, r));
                }
            }
        }
        ensure!(next.len() < before, "unavailable committed history");
        pending = next;
    }
    Ok(index)
}
struct Headers {
    facts: Vec<(Fact, String)>,
    actor: Uuid,
    generation: Uuid,
}
impl Headers {
    fn capture(paths: &StorePaths, budget: &mut Budget) -> Result<Self> {
        let mut facts = Vec::new();
        fn read<T: serde::de::DeserializeOwned>(
            path: &Path,
            limit: u64,
            budget: &mut Budget,
            facts: &mut Vec<(Fact, String)>,
        ) -> Result<T> {
            let f = file(path)?;
            let bytes = f.read(budget, limit)?;
            inspection::strict_json(&bytes)?;
            let value = serde_json::from_slice(&bytes)?;
            facts.push((f, digest(&bytes)));
            Ok(value)
        }
        let identity: Identity = read(&paths.data.join("identity.json"), MIB, budget, &mut facts)?;
        ensure!(
            identity.format == FORMAT && !identity.actor.is_nil(),
            "invalid initial identity"
        );
        for root in [&paths.cache, &paths.credentials] {
            let owner: Identity = read(&root.join("owner.json"), MIB, budget, &mut facts)?;
            ensure!(
                owner.actor == identity.actor && owner.format == FORMAT,
                "foreign initial owner"
            );
        }
        let generation: Uuid = read(&paths.data.join("CURRENT"), 128, budget, &mut facts)?;
        ensure!(!generation.is_nil(), "nil initial generation");
        let format: u32 = read(
            &paths
                .data
                .join("generations")
                .join(generation.to_string())
                .join("format.json"),
            64,
            budget,
            &mut facts,
        )?;
        ensure!(format == FORMAT, "initial generation not ordinary");
        Ok(Self {
            facts,
            actor: identity.actor,
            generation,
        })
    }
    fn check(&self, budget: &mut Budget) -> Result<()> {
        for (fact, sha) in &self.facts {
            let bytes = fact.read(budget, fact.len)?;
            ensure!(digest(&bytes) == *sha, "admitted header content changed");
        }
        Ok(())
    }
}
pub(crate) struct Session {
    pub store: Arc<Store>,
    inventory: Inventory,
    pub budget: Budget,
    headers: Headers,
}
impl Session {
    pub(crate) fn open(paths: StorePaths, known: &BTreeMap<String, u64>) -> Result<Self> {
        Self::open_observed(paths, known, |_, _| Ok(()))
    }
    fn open_observed(
        paths: StorePaths,
        known: &BTreeMap<String, u64>,
        during: impl FnOnce(&Store, Uuid) -> Result<()>,
    ) -> Result<Self> {
        use std::cell::RefCell;
        let budget = RefCell::new(Budget::default());
        let headers = RefCell::new(None::<Headers>);
        let mut saved = None;
        let store = Store::open_existing_for_conversation(
            paths,
            |paths, held| {
                let named = file(&paths.data.join("store.lock"))?;
                ensure!(
                    named.identity == identity(&held.metadata()?),
                    "initial lock changed"
                );
                let h = Headers::capture(paths, &mut budget.borrow_mut())?;
                // The unchanged typed constructor rereads these five headers.
                // Reserve its worst-case bounded reads before they can occur.
                budget.borrow_mut().charge(3 * (MIB + 1) + 129 + 65, 5)?;
                *headers.borrow_mut() = Some(h);
                Ok(())
            },
            |s, g| {
                Self::check_lock(s)?;
                {
                    let h = headers.borrow();
                    let h = h.as_ref().context("initial headers absent")?;
                    ensure!(
                        h.actor == s.actor_id && h.generation == g,
                        "initial admission changed"
                    );
                    h.check(&mut budget.borrow_mut())?;
                }
                let inv = inventory(s, g, &mut budget.borrow_mut(), known)?;
                during(s, g)?;
                let index = rebuild(&inv, &mut budget.borrow_mut())?;
                headers
                    .borrow()
                    .as_ref()
                    .context("initial headers absent")?
                    .check(&mut budget.borrow_mut())?;
                Self::check_lock(s)?;
                saved = Some(inv);
                Ok(index)
            },
        )?;
        let mut session = Self {
            store: Arc::new(store),
            inventory: saved.context("inventory absent")?,
            budget: budget.into_inner(),
            headers: headers.into_inner().context("headers absent")?,
        };
        session.check()?;
        Ok(session)
    }
    fn check_lock(store: &Store) -> Result<()> {
        let named = file(&store.paths.data.join("store.lock"))?;
        let held = store._lease.0.metadata()?;
        ensure!(named.identity == identity(&held), "named lock replaced");
        Ok(())
    }
    pub(crate) fn check(&mut self) -> Result<()> {
        Self::check_lock(&self.store)?;
        self.headers.check(&mut self.budget)?;
        for f in self.inventory.files.values() {
            self.budget.charge(0, 1)?;
            f.check()?;
        }
        Ok(())
    }
    pub(crate) fn generation(&self) -> Result<Uuid> {
        Ok(self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?
            .generation)
    }
    pub(crate) fn snapshot(&mut self, conversation: Uuid) -> Result<Snapshot> {
        let inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?;
        snapshot(&inner, conversation, &mut self.budget)
    }
}
pub(crate) struct Snapshot {
    pub generation: Uuid,
    pub history: Vec<Envelope>,
    pub current: Vec<Envelope>,
    pub manifests: Vec<Manifest>,
    pub raw_hashes: BTreeMap<Uuid, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateRefusal {
    TooLarge,
    Unsupported,
    Conflict,
    Unavailable,
}
impl Session {
    pub(crate) fn candidate_roots(&mut self) -> std::result::Result<Vec<Uuid>, CandidateRefusal> {
        use CandidateRefusal::*;
        let inner = self.store.inner.lock().map_err(|_| Unavailable)?;
        // Boolean type facts only: no copied text, bindings or native identities.
        let mut kinds = BTreeMap::<Uuid, (bool, bool)>::new();
        for (e, sha) in inner.index.records.values() {
            self.budget.charge(0, 1).map_err(|_| Unavailable)?;
            if e.namespace != Namespace::Conversation {
                continue;
            }
            if e.domain_schema_version != 1 {
                return Err(Unsupported);
            }
            if e.kind == Kind::Tombstone {
                continue;
            }
            // The already-admitted raw object length pays for this additional
            // typed decode/clone BEFORE it happens. Never serialize to find size.
            let bytes = self.inventory.files.get(sha).ok_or(Unavailable)?.len;
            self.budget.charge(bytes, 1).map_err(|_| Unavailable)?;
            let root = crate::conversation::cleanup::inventory_value_is_root(e)
                .map_err(|_| Unsupported)?;
            let entry = kinds.entry(e.record_id).or_default();
            if root {
                entry.0 = true;
            } else {
                entry.1 = true;
            }
        }
        let mut candidates = Vec::new();
        for (id, (root, other)) in kinds {
            self.budget.charge(0, 1).map_err(|_| Unavailable)?;
            if !root {
                continue;
            }
            if other {
                return Err(Conflict);
            }
            let heads = inner
                .index
                .heads
                .get(&(Namespace::Conversation, id))
                .ok_or(Conflict)?;
            if heads.len() != 1 {
                return Err(Conflict);
            }
            for revision in heads {
                self.budget.charge(0, 1).map_err(|_| Unavailable)?;
                let (e, _) = inner.index.records.get(revision).ok_or(Unavailable)?;
                if e.kind == Kind::Value {
                    candidates.push(id);
                }
            }
        }
        // All roots/heads were classified first; never expose a truncated prefix.
        if candidates.len() > 16 {
            return Err(TooLarge);
        }
        candidates.sort_by_key(Uuid::to_string);
        Ok(candidates)
    }
}
fn snapshot(inner: &Inner, conversation: Uuid, budget: &mut Budget) -> Result<Snapshot> {
    let id = conversation.to_string();
    let mut keys = BTreeSet::from([(Namespace::Conversation, conversation)]);
    for (e, _) in inner.index.records.values() {
        budget.charge(0, 1)?;
        let p = &e.payload["record"];
        if p["conversation"].as_str() == Some(&id)
            || p["acknowledgment"]["conversation"].as_str() == Some(&id)
        {
            keys.insert((e.namespace, e.record_id));
        }
        if e.namespace == Namespace::Conversation && e.record_id == conversation {
            if let Some(s) = p["binding"].as_str() {
                keys.insert((Namespace::Conversation, Uuid::parse_str(s)?));
            }
        }
    }
    let mut history = Vec::new();
    let mut bytes = 0usize;
    for (e, _) in inner.index.records.values() {
        budget.charge(0, 1)?;
        if keys.contains(&(e.namespace, e.record_id)) {
            ensure!(history.len() < 4096, "target record bound");
            bytes = bytes
                .checked_add(serde_json::to_vec(e)?.len())
                .context("metadata overflow")?;
            ensure!(bytes <= 8 * MIB as usize, "target metadata bound");
            history.push(e.clone());
        }
    }
    history.sort_by_key(|e| {
        (
            serde_json::to_string(&e.namespace).unwrap_or_default(),
            e.record_id,
            e.revision_id,
        )
    });
    let current = history
        .iter()
        .filter(|e| {
            inner
                .index
                .heads
                .get(&(e.namespace, e.record_id))
                .is_some_and(|h| h.contains(&e.revision_id))
        })
        .cloned()
        .collect();
    Ok(Snapshot {
        generation: inner.generation,
        history,
        current,
        manifests: inner.index.manifests.values().cloned().collect(),
        raw_hashes: inner
            .index
            .records
            .iter()
            .map(|(id, (_, sha))| (*id, sha.clone()))
            .collect(),
    })
}
pub(crate) fn history_hash(s: &Snapshot) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&s.history)?))
}

#[derive(Clone)]
pub(crate) struct Plan {
    pub manifest: Manifest,
    pub records: Vec<Envelope>,
    pub objects: BTreeMap<String, Vec<u8>>,
}
impl Plan {
    pub(crate) fn new(intent: &Intent) -> Result<Self> {
        use crate::conversation::{Acknowledgment, Receipt as HistoricalReceipt, Record};
        let operation = crate::conversation_maintenance::uuid(&intent.operation)?;
        let conversation = crate::conversation_maintenance::uuid(&intent.conversation)?;
        let actor = crate::conversation_maintenance::uuid(&intent.actor)?;
        let envelope = |id, role: &str, parents, kind, payload| Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: id,
            revision_id: intent.derived(&format!("{role}-revision")),
            parents,
            operation_id: intent.derived(&format!("{role}-operation")),
            actor_id: actor,
            kind,
            payload,
            media_descriptors: vec![],
        };
        let root = envelope(
            conversation,
            "root",
            BTreeSet::from([crate::conversation_maintenance::uuid(
                &intent.root.revision,
            )?]),
            Kind::Tombstone,
            serde_json::Value::Null,
        );
        let mut records = vec![root.clone()];
        let binding = if let Some(b) = &intent.binding {
            let id = crate::conversation_maintenance::uuid(&b.id)?;
            records.push(envelope(
                id,
                "binding",
                BTreeSet::from([crate::conversation_maintenance::uuid(&b.revision)?]),
                Kind::Tombstone,
                serde_json::Value::Null,
            ));
            Some(id)
        } else {
            None
        };
        let receipt = Record::Receipt(HistoricalReceipt {
            fingerprint: digest(&serde_json::to_vec(&[
                "prepared-conversation-delete-v1",
                &intent.intent_sha256,
            ])?),
            acknowledgment: Acknowledgment {
                operation,
                conversation,
                records: binding.into_iter().collect(),
                applied_root_revision: root.revision_id,
                binding,
            },
            admitted_intent: None,
        });
        let receipt_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:remarkable-buddies:operation:v1:{operation}").as_bytes(),
        );
        records.push(envelope(
            receipt_id,
            "receipt",
            BTreeSet::new(),
            Kind::Value,
            serde_json::to_value(receipt)?,
        ));
        let mut objects = BTreeMap::new();
        let mut refs = Vec::new();
        let mut namespaces = BTreeMap::new();
        for e in &records {
            e.validate()?;
            let b = serde_json::to_vec(e)?;
            let sha = digest(&b);
            refs.push(ObjectRef {
                sha256: sha.clone(),
                bytes: b.len() as u64,
            });
            namespaces.insert(sha.clone(), e.namespace);
            objects.insert(sha, b);
        }
        let manifest = Manifest {
            format: FORMAT,
            transaction_id: intent.derived("transaction"),
            scope: Scope::LocalTransaction,
            records: refs,
            record_namespaces: namespaces,
            media: vec![],
            media_coverage: vec![],
        };
        manifest.validate()?;
        Ok(Self {
            manifest,
            records,
            objects,
        })
    }
    pub(crate) fn known(&self) -> BTreeMap<String, u64> {
        self.objects
            .iter()
            .map(|(h, b)| (h.clone(), b.len() as u64))
            .collect()
    }
    pub(crate) fn receipt(&self, intent: &Intent) -> Receipt {
        Receipt {
            operation: intent.operation.clone(),
            transaction: self.manifest.transaction_id,
            root_revision: intent.derived("root-revision"),
            binding_revision: intent
                .binding
                .as_ref()
                .map(|_| intent.derived("binding-revision")),
        }
    }
}
impl Session {
    pub(crate) fn acknowledged(&mut self, intent: &Intent, plan: &Plan) -> Result<bool> {
        self.check()?;
        let inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?;
        self.budget.charge(0, plan.records.len() as u64)?;
        if let Some(m) = inner.index.manifests.get(&plan.manifest.transaction_id) {
            ensure!(m == &plan.manifest, "changed original transaction");
            ensure!(
                self.inventory
                    .manifest_sha256
                    .get(&plan.manifest.transaction_id)
                    == Some(&digest(&serde_json::to_vec(&plan.manifest)?)),
                "original manifest bytes changed"
            );
            for e in &plan.records {
                ensure!(
                    inner
                        .index
                        .records
                        .get(&e.revision_id)
                        .is_some_and(|(old, sha)| old == e && plan.objects.contains_key(sha)),
                    "original receipt changed"
                );
            }
            let original = inner
                .index
                .records
                .get(&crate::conversation_maintenance::uuid(
                    &intent.root.revision,
                )?)
                .context("original root absent")?;
            ensure!(
                digest(&serde_json::to_vec(&original.0)?) == intent.root.sha256
                    && original.0.kind == Kind::Value
                    && original.0.record_id
                        == crate::conversation_maintenance::uuid(&intent.conversation)?,
                "original root changed"
            );
            let expected_binding = crate::conversation::cleanup::original_root(
                &original.0,
                crate::conversation_maintenance::uuid(&intent.conversation)?,
            )?;
            ensure!(
                expected_binding
                    == intent
                        .binding
                        .as_ref()
                        .map(|b| crate::conversation_maintenance::uuid(&b.id))
                        .transpose()?,
                "original binding relationship changed"
            );
            if let Some(b) = &intent.binding {
                let original = inner
                    .index
                    .records
                    .get(&crate::conversation_maintenance::uuid(&b.revision)?)
                    .context("original binding absent")?;
                ensure!(
                    digest(&serde_json::to_vec(&original.0)?) == b.sha256
                        && original.0.kind == Kind::Value
                        && original.0.record_id == crate::conversation_maintenance::uuid(&b.id)?,
                    "original binding changed"
                );
                crate::conversation::cleanup::original_binding(
                    &original.0,
                    crate::conversation_maintenance::uuid(&intent.conversation)?,
                    crate::conversation_maintenance::uuid(&b.id)?,
                )?;
            }
            return Ok(true);
        }
        // Exact derived records under another/partial transaction establish
        // possible current state, never the original receipt or fresh dispatch.
        for e in &plan.records {
            let expected = digest(&serde_json::to_vec(e)?);
            if let Some((stored, sha)) = inner.index.records.get(&e.revision_id) {
                ensure!(stored == e && *sha == expected, "changed partial record");
            }
            if let Some(sha) = inner.index.operations.get(&e.operation_id) {
                ensure!(*sha == expected, "changed partial operation");
            }
        }
        let receipt = plan.records.last().context("receipt absent")?;
        if let Some(heads) = inner
            .index
            .heads
            .get(&(receipt.namespace, receipt.record_id))
        {
            ensure!(
                *heads == BTreeSet::from([receipt.revision_id]),
                "operation reused"
            );
        }
        Ok(false)
    }
    pub(crate) fn partial(&self, plan: &Plan) -> bool {
        plan.objects
            .keys()
            .any(|h| self.inventory.files.contains_key(h))
    }
    pub(crate) fn current_state(&mut self, intent: &Intent, plan: &Plan) -> Result<&'static str> {
        let inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?;
        let mut old = true;
        let mut desired = true;
        for e in plan.records.iter().filter(|e| e.kind == Kind::Tombstone) {
            self.budget.charge(0, 1)?;
            let hs = inner.index.heads.get(&(e.namespace, e.record_id));
            desired &= hs == Some(&BTreeSet::from([e.revision_id]));
            let original = if e.record_id.to_string() == intent.conversation {
                crate::conversation_maintenance::uuid(&intent.root.revision)?
            } else {
                crate::conversation_maintenance::uuid(
                    &intent.binding.as_ref().context("binding")?.revision,
                )?
            };
            old &= hs == Some(&BTreeSet::from([original]));
        }
        Ok(if old {
            "live-original"
        } else if desired {
            "desired-tombstones"
        } else {
            "changed"
        })
    }
    pub(crate) fn reserve(
        &mut self,
        intent: &Intent,
        plan: &Plan,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let mut prospective = self.inventory.clone();
        let n = plan.records.len() as u64;
        let r = plan.objects.values().map(|v| v.len() as u64).sum::<u64>();
        let m = serde_json::to_vec(&plan.manifest)?.len() as u64;
        add(&mut prospective.commits, 2, 4096)?;
        add(&mut prospective.objects, n + 1, 8192)?;
        add(&mut prospective.manifest_bytes, 2 * m, 8 * MIB)?;
        add(&mut prospective.object_bytes, 2 * r, 256 * MIB)?;
        add(&mut prospective.record_bytes, 2 * r, 32 * MIB)?;
        let mut history = snapshot.history.clone();
        history.extend(plan.records.clone());
        ensure!(
            history.len() <= 4096 && serde_json::to_vec(&history)?.len() <= 8 * MIB as usize,
            "target publication headroom"
        );
        let inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?;
        ensure!(
            !inner
                .index
                .manifests
                .contains_key(&plan.manifest.transaction_id),
            "transaction collision"
        );
        let receipt = plan.records.last().context("receipt absent")?;
        ensure!(
            !inner
                .index
                .heads
                .contains_key(&(receipt.namespace, receipt.record_id)),
            "operation already used"
        );
        for e in &plan.records {
            ensure!(
                !inner.index.records.contains_key(&e.revision_id)
                    && !inner.index.operations.contains_key(&e.operation_id),
                "derived identity collision"
            );
        }
        drop(inner);
        for (h, b) in &plan.objects {
            prospective
                .records
                .insert(h.clone(), serde_json::from_slice(b)?);
        }
        prospective.manifests.push(plan.manifest.clone());
        prospective.manifests.sort_by_key(|m| m.transaction_id);
        // Conservative prospective reopen: retain all original work, charge new
        // content/receipt reads, and simulate every bounded dependency/index visit.
        let mut future = self.budget.clone();
        future.charge(2 * r + 2 * m, 2 * n + 2)?;
        rebuild(&prospective, &mut future)?;
        ensure!(
            crate::conversation_maintenance::uuid(&intent.generation)? == snapshot.generation,
            "generation changed"
        );
        Ok(())
    }
    pub(crate) fn publish(
        &mut self,
        intent: &Intent,
        plan: &Plan,
        attempted: &mut bool,
    ) -> Result<()> {
        self.check()?;
        let mut inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?;
        ensure!(
            inner.generation == crate::conversation_maintenance::uuid(&intent.generation)?
                && self.store.actor_id == crate::conversation_maintenance::uuid(&intent.actor)?,
            "generation/actor changed"
        );
        // Any in-process publication since admission invalidates physical headroom,
        // even when it is unrelated to the selected conversation.
        ensure!(
            inner.index.manifests.len() == self.inventory.manifests.len()
                && self.inventory.manifests.iter().all(|m| inner
                    .index
                    .manifests
                    .get(&m.transaction_id)
                    == Some(m)),
            "admitted history changed"
        );
        let target = snapshot(
            &inner,
            crate::conversation_maintenance::uuid(&intent.conversation)?,
            &mut self.budget,
        )?;
        ensure!(
            history_hash(&target)? == intent.target_history_sha256,
            "target history changed"
        );
        for e in plan.records.iter().filter(|e| e.kind == Kind::Tombstone) {
            ensure!(
                inner.index.heads.get(&(e.namespace, e.record_id)) == Some(&e.parents),
                "target heads changed"
            );
        }
        ensure!(
            inner.mode_handles > 0
                && inner.legacy_sync_handles == 0
                && inner.legacy_publisher_handles == 0,
            "maintenance worker exclusion"
        );
        let encoded: Vec<_> = plan
            .records
            .iter()
            .map(|e| Ok((e.clone(), digest(&serde_json::to_vec(e)?))))
            .collect::<Result<_>>()?;
        can_apply(&inner.index, &encoded, &mut self.budget)?;
        let index_work = inner
            .index
            .records
            .values()
            .chain(encoded.iter())
            .map(|(e, _)| 2 + e.parents.len() as u64)
            .sum::<u64>();
        // The unchanged publisher verifies and rereads new record objects. Charge
        // both passes plus index traversal BEFORE its first filesystem write.
        self.budget.charge(
            2 * plan.objects.values().map(|b| b.len() as u64).sum::<u64>(),
            index_work + plan.records.len() as u64,
        )?;
        *attempted = true;
        self.store.publish_locked(
            &mut inner,
            plan.manifest.clone(),
            plan.objects.clone(),
            true,
        )?;
        ensure!(
            inner.index.manifests.get(&plan.manifest.transaction_id) == Some(&plan.manifest),
            "publication unverified"
        );
        let root = self.store.generation(inner.generation);
        let manifest = file(
            &root
                .join("commits")
                .join(format!("{}.json", plan.manifest.transaction_id)),
        )?
        .read(&mut self.budget, 8 * MIB)?;
        ensure!(
            manifest == serde_json::to_vec(&plan.manifest)?,
            "durable manifest readback changed"
        );
        for (sha, bytes) in &plan.objects {
            let actual = file(&root.join("objects").join(sha))?.read(&mut self.budget, MIB)?;
            ensure!(&actual == bytes, "durable record readback changed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cumulative_work_is_sticky_and_overflow_is_not_a_reset() {
        let mut b = Budget::default();
        b.charge(512 * MIB, 65536).unwrap();
        assert!(b.charge(1, 0).is_err());
        assert!(b.charge(0, 0).is_err());
        assert_eq!((b.bytes, b.visits), (512 * MIB, 65536));
        let mut b = Budget::default();
        assert!(b.charge(u64::MAX, 1).is_err());
        assert!(b.charge(0, 1).is_err());
    }
}

#[cfg(test)]
mod shared_reference_tests {
    use super::*;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            if std::thread::panicking() {
                return;
            }
            let p = self.0.canonicalize().unwrap();
            assert!(
                p.starts_with(std::env::temp_dir().canonicalize().unwrap())
                    && p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("buddy-conversation-work-v1-")
            );
            fs::remove_dir_all(p).unwrap();
        }
    }
    #[test]
    fn shared_sixteen_mib_object_in_thousand_manifests_refuses_within_one_budget() {
        let temp = Temp(
            std::env::temp_dir().join(format!("buddy-conversation-work-v1-{}", Uuid::new_v4())),
        );
        let paths = StorePaths {
            data: temp.0.join("data"),
            cache: temp.0.join("cache"),
            credentials: temp.0.join("credentials"),
        };
        let store = Store::open(paths.clone()).unwrap();
        let actor = store.actor_id;
        let generation = store.inner.lock().unwrap().generation;
        let root = store.generation(generation);
        drop(store);
        let media = vec![0x5a; 16 * 1024 * 1024];
        let media_sha = digest(&media);
        fs::write(root.join("objects").join(&media_sha), &media).unwrap();
        for _ in 0..1000 {
            let e = Envelope {
                envelope_version: 1,
                namespace: Namespace::SubjectMemory,
                domain_schema_version: 1,
                record_id: Uuid::new_v4(),
                revision_id: Uuid::new_v4(),
                parents: BTreeSet::new(),
                operation_id: Uuid::new_v4(),
                actor_id: actor,
                kind: Kind::Value,
                payload: serde_json::json!({"synthetic":"repeated-reference recovery only"}),
                media_descriptors: vec![Media {
                    sha256: media_sha.clone(),
                    bytes: media.len() as u64,
                    media_type: "application/octet-stream".into(),
                }],
            };
            let b = serde_json::to_vec(&e).unwrap();
            let sha = digest(&b);
            fs::write(root.join("objects").join(&sha), &b).unwrap();
            let m = Manifest {
                format: 1,
                transaction_id: Uuid::new_v4(),
                scope: Scope::LocalTransaction,
                records: vec![ObjectRef {
                    sha256: sha.clone(),
                    bytes: b.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(sha, Namespace::SubjectMemory)]),
                media: vec![ObjectRef {
                    sha256: media_sha.clone(),
                    bytes: media.len() as u64,
                }],
                media_coverage: vec![Coverage::Included {
                    sha256: media_sha.clone(),
                }],
            };
            m.validate().unwrap();
            fs::write(
                root.join("commits")
                    .join(format!("{}.json", m.transaction_id)),
                serde_json::to_vec(&m).unwrap(),
            )
            .unwrap();
        }
        let mut budget = Budget::default();
        let result = Store::open_existing_for_conversation(
            paths.clone(),
            |_, _| Ok(()),
            |s, g| {
                let inv = inventory(s, g, &mut budget, &BTreeMap::new())?;
                rebuild(&inv, &mut budget)
            },
        );
        assert!(result.is_err());
        assert!(budget.exhausted);
        assert!(budget.bytes <= 512 * MIB && budget.visits <= 65536);
        assert!(
            budget.bytes >= 16 * MIB && budget.bytes < 32 * MIB,
            "verified reuse must not rehash media per manifest"
        );
        let lock = File::open(paths.data.join("store.lock")).unwrap();
        assert!(lock.try_lock().is_ok());
    }
}

#[cfg(test)]
mod header_race_tests {
    use super::*;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            if std::thread::panicking() {
                return;
            }
            let p = self.0.canonicalize().unwrap();
            assert!(
                p.starts_with(std::env::temp_dir().canonicalize().unwrap())
                    && p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("buddy-cleanup-headers-v1-")
            );
            fs::remove_dir_all(p).unwrap();
        }
    }
    fn fixture() -> (Temp, StorePaths) {
        let t =
            Temp(std::env::temp_dir().join(format!("buddy-cleanup-headers-v1-{}", Uuid::new_v4())));
        let p = StorePaths {
            data: t.0.join("data"),
            cache: t.0.join("cache"),
            credentials: t.0.join("credentials"),
        };
        drop(Store::open(p.clone()).unwrap());
        (t, p)
    }
    fn change(paths: &StorePaths, g: Uuid, kind: usize) -> PathBuf {
        let path = match kind {
            0 => paths.data.join("CURRENT"),
            1 => paths.data.join("identity.json"),
            2 => paths.cache.join("owner.json"),
            3 => paths.credentials.join("owner.json"),
            _ => paths
                .data
                .join("generations")
                .join(g.to_string())
                .join("format.json"),
        };
        let old = file(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let next = match kind {
            0 => serde_json::to_vec(&Uuid::new_v4()).unwrap(),
            4 => b"2".to_vec(),
            _ => serde_json::to_vec(&Identity {
                format: FORMAT,
                actor: Uuid::new_v4(),
            })
            .unwrap(),
        };
        assert_eq!(next.len(), bytes.len());
        fs::write(&path, &next).unwrap();
        let f = OpenOptions::new().write(true).open(&path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(old.modified.unwrap()))
            .unwrap();
        old.check().unwrap();
        assert_ne!(digest(&bytes), digest(&next));
        path
    }
    #[test]
    fn initial_header_bytes_survive_recovery_or_admission_refuses() {
        for kind in 0..5 {
            let (_t, p) = fixture();
            let mut touched = None;
            let result = Session::open_observed(p.clone(), &BTreeMap::new(), |s, g| {
                touched = Some(change(&s.paths, g, kind));
                Ok(())
            });
            assert!(
                result.is_err(),
                "changed header{kind} became a new baseline"
            );
            assert!(touched.unwrap().exists());
            let lock = File::open(p.data.join("store.lock")).unwrap();
            assert!(lock.try_lock().is_ok());
        }
    }
    #[test]
    fn later_same_length_same_metadata_header_rewrite_is_not_accepted() {
        for kind in 0..5 {
            let (_t, p) = fixture();
            let mut session = Session::open(p.clone(), &BTreeMap::new()).unwrap();
            let g = session.generation().unwrap();
            let path = change(&p, g, kind);
            let changed = fs::read(&path).unwrap();
            assert!(session.check().is_err());
            assert_eq!(fs::read(path).unwrap(), changed);
        }
    }
}

#[cfg(test)]
mod reservation_boundary_tests {
    use super::*;
    use crate::{
        conversation::{BindingReceipt, ExpectedHeads, Ledger, SourceObservation},
        conversation_maintenance::{Binding, Expected, Summary},
    };
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            if std::thread::panicking() {
                return;
            }
            let p = self.0.canonicalize().unwrap();
            assert!(
                p.starts_with(std::env::temp_dir().canonicalize().unwrap())
                    && p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("buddy-cleanup-reservation-v1-")
            );
            fs::remove_dir_all(p).unwrap();
        }
    }
    fn fixture(
        bound: bool,
    ) -> (
        Temp,
        Session,
        Snapshot,
        Intent,
        Plan,
        sync::mode::OrdinaryModeLease,
    ) {
        let t = Temp(
            std::env::temp_dir().join(format!("buddy-cleanup-reservation-v1-{}", Uuid::new_v4())),
        );
        let paths = StorePaths {
            data: t.0.join("data"),
            cache: t.0.join("cache"),
            credentials: t.0.join("credentials"),
        };
        let store = Arc::new(Store::open(paths.clone()).unwrap());
        let ledger = Ledger::new(store);
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        if bound {
            let document = Uuid::new_v4();
            let page = Uuid::new_v4();
            let target = Uuid::new_v4();
            let operation = Uuid::new_v4();
            let source = SourceObservation {
                document,
                page,
                session: "synthetic".into(),
                visit: "synthetic".into(),
                content_revision: None,
                order_revision: None,
                capability_revision: "synthetic".into(),
                evidence_procedure: "synthetic".into(),
            };
            let claim = BindingReceipt {
                acquisition: Default::default(),
                operation,
                conversation,
                request_fingerprint: "synthetic".into(),
                adapter_fingerprint: "synthetic".into(),
                source,
                intended_target: Some(target),
                observed_document: document,
                observed_page: target,
                expected_order: vec![page],
                observed_order: vec![page, target],
                persisted_revision: "synthetic".into(),
                observed_ms: 1,
                evidence_origin: "synthetic".into(),
            };
            ledger
                .bind(
                    operation,
                    ledger
                        .expected(Namespace::Conversation, conversation)
                        .unwrap(),
                    ExpectedHeads::default(),
                    claim,
                )
                .unwrap();
        }
        drop(ledger);
        let mut session = Session::open(paths, &BTreeMap::new()).unwrap();
        let snapshot = session.snapshot(conversation).unwrap();
        let root = snapshot
            .current
            .iter()
            .find(|e| e.record_id == conversation)
            .unwrap();
        let binding = root.payload["record"]["binding"].as_str().map(|id| {
            let e = snapshot
                .current
                .iter()
                .find(|e| e.record_id.to_string() == id)
                .unwrap();
            Binding {
                id: id.into(),
                revision: e.revision_id.to_string(),
                sha256: digest(&serde_json::to_vec(e).unwrap()),
            }
        });
        let mode = session.store.sync_mode_snapshot().unwrap();
        let guard = session.store.ordinary_mode_lease(&mode.token).unwrap();
        let mode_sha = serde_json::to_value(&mode.token).unwrap()["sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut i = Intent {
            intent_version: 1,
            operation: Uuid::new_v4().to_string(),
            conversation: conversation.to_string(),
            config_path_sha256: "a".repeat(64),
            config_bytes_sha256: "b".repeat(64),
            roots_sha256: "c".repeat(64),
            actor: session.store.actor_id.to_string(),
            generation: snapshot.generation.to_string(),
            mode_sha256: mode_sha,
            target_history_sha256: history_hash(&snapshot).unwrap(),
            root: Expected {
                revision: root.revision_id.to_string(),
                sha256: digest(&serde_json::to_vec(root).unwrap()),
            },
            binding,
            summary: Summary::default(),
            intent_sha256: String::new(),
        };
        i.intent_sha256 = i.hash().unwrap();
        let plan = Plan::new(&i).unwrap();
        (t, session, snapshot, i, plan, guard)
    }
    #[test]
    fn every_inventory_reservation_boundary_refuses_before_mutation_or_fits() {
        // Inject numeric inventory boundaries, not fake physical hundreds-of-MiB
        // files. Real inventory parsing and committed reopen are tested separately.
        for bound in [false, true] {
            for kind in 0..5 {
                for extra in [0, 1, 2] {
                    let (_t, mut session, snapshot, i, plan, _guard) = fixture(bound);
                    let n = plan.records.len() as u64;
                    let r = plan.objects.values().map(|b| b.len() as u64).sum::<u64>();
                    let m = serde_json::to_vec(&plan.manifest).unwrap().len() as u64;
                    let (limit, delta) = match kind {
                        0 => (4096, 2),
                        1 => (8192, n + 1),
                        2 => (8 * MIB, 2 * m),
                        3 => (256 * MIB, 2 * r),
                        _ => (32 * MIB, 2 * r),
                    };
                    let used = if extra == 2 {
                        limit
                    } else {
                        limit - delta + extra
                    };
                    match kind {
                        0 => session.inventory.commits = used,
                        1 => session.inventory.objects = used,
                        2 => session.inventory.manifest_bytes = used,
                        3 => session.inventory.object_bytes = used,
                        _ => session.inventory.record_bytes = used,
                    };
                    let before = session.store.inner.lock().unwrap().index.manifests.len();
                    assert_eq!(
                        session.reserve(&i, &plan, &snapshot).is_ok(),
                        extra == 0,
                        "bound{bound}/kind{kind}/extra{extra}"
                    );
                    assert_eq!(
                        before,
                        session.store.inner.lock().unwrap().index.manifests.len()
                    );
                    assert!(!session
                        .store
                        .generation(snapshot.generation)
                        .join("commits")
                        .join(format!("{}.json", plan.manifest.transaction_id))
                        .exists());
                }
            }
        }
    }
    #[test]
    fn target_count_and_exact_encoded_byte_headroom_are_enforced() {
        for bound in [false, true] {
            let (_t, mut session, mut snapshot, i, plan, _guard) = fixture(bound);
            let sample = snapshot.history[0].clone();
            snapshot
                .history
                .resize(4096 - plan.records.len(), sample.clone());
            assert!(session.reserve(&i, &plan, &snapshot).is_ok());
            snapshot.history.push(sample);
            assert!(session.reserve(&i, &plan, &snapshot).is_err());
        }
        for extra in [0, 1] {
            let (_t, mut session, mut snapshot, i, plan, _guard) = fixture(false);
            let mut padding = snapshot.history[0].clone();
            padding.payload = serde_json::Value::String(String::new());
            let mut prospective = snapshot.history.clone();
            prospective.push(padding.clone());
            prospective.extend(plan.records.clone());
            let baseline = serde_json::to_vec(&prospective).unwrap().len();
            padding.payload =
                serde_json::Value::String("x".repeat(8 * MIB as usize - baseline + extra));
            snapshot.history.push(padding);
            assert_eq!(session.reserve(&i, &plan, &snapshot).is_ok(), extra == 0);
        }
    }
    #[test]
    fn active_legacy_publisher_handle_is_not_external_maintenance_authority() {
        let (_t, mut session, snapshot, i, plan, _guard) = fixture(false);
        session.reserve(&i, &plan, &snapshot).unwrap();
        let publisher = session.store.legacy_publisher_lease().unwrap();
        let before = session.store.inner.lock().unwrap().index.manifests.len();
        let mut attempted = false;
        assert!(session.publish(&i, &plan, &mut attempted).is_err());
        assert!(!attempted);
        assert_eq!(
            before,
            session.store.inner.lock().unwrap().index.manifests.len()
        );
        drop(publisher);
    }
}
