//! New bounded existing-only recovery and conditional conversation transaction.
//! Legacy recovery/publication callers are unchanged.
use super::*;
use crate::{
    config::inspection,
    conversation_maintenance::{Intent, Receipt},
};
use std::{fs::Metadata, io::Read};
const MIB: u64 = 1024 * 1024;
#[derive(Clone, Default, Debug)]
pub(crate) struct Budget {
    pub bytes: u64,
    pub visits: u64,
    exhausted: bool,
}
impl Budget {
    pub(crate) fn charge(&mut self, bytes: u64, visits: u64) -> Result<()> {
        ensure!(!self.exhausted, "conversation recovery budget exhausted");
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
                anyhow::bail!("conversation recovery work exhausted")
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
pub(crate) struct Session {
    pub store: Arc<Store>,
    inventory: Inventory,
    pub budget: Budget,
    headers: Vec<(Fact, String)>,
}
impl Session {
    pub(crate) fn open(paths: StorePaths, known: &BTreeMap<String, u64>) -> Result<Self> {
        let mut budget = Budget::default();
        let mut saved = None;
        let store = Store::open_existing_for_conversation(paths, |s, g| {
            Self::check_lock(s)?;
            let inv = inventory(s, g, &mut budget, known)?;
            let index = rebuild(&inv, &mut budget)?;
            saved = Some(inv);
            Ok(index)
        })?;
        let store = Arc::new(store);
        let mut headers = Vec::new();
        let generation = store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("mutex"))?
            .generation;
        for p in [
            store.paths.data.join("identity.json"),
            store.paths.data.join("CURRENT"),
            store.generation(generation).join("format.json"),
            store.paths.cache.join("owner.json"),
            store.paths.credentials.join("owner.json"),
        ] {
            let f = file(&p)?;
            let bytes = f.read(&mut budget, MIB)?;
            headers.push((f, digest(&bytes)));
        }
        let mut session = Self {
            store,
            inventory: saved.context("inventory absent")?,
            budget,
            headers,
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
        for (f, _) in &self.headers {
            self.budget.charge(0, 1)?;
            f.check()?;
        }
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
        for e in &plan.records {
            ensure!(
                !inner.index.records.contains_key(&e.revision_id)
                    && !inner.index.operations.contains_key(&e.operation_id),
                "partial/changed original receipt"
            );
        }
        let receipt = plan.records.last().context("receipt absent")?;
        ensure!(
            !inner
                .index
                .heads
                .contains_key(&(receipt.namespace, receipt.record_id)),
            "operation reused"
        );
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
        let result = Store::open_existing_for_conversation(paths.clone(), |s, g| {
            let inv = inventory(s, g, &mut budget, &BTreeMap::new())?;
            rebuild(&inv, &mut budget)
        });
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
