//! Archive custody only. Original identities/pins never become live authority.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectionArchive {
    pub origin_actor: Uuid,
    pub origin_generation: Uuid,
    pub feature: ObjectRef,
    pub heads: Vec<ObjectRef>,
}

pub(crate) struct ArchivedSelections {
    pub archive: SelectionArchive,
    pub metadata: BTreeMap<String, Vec<u8>>,
    pub manifests: BTreeMap<Uuid, Manifest>,
    pub receipts: usize,
}

impl ArchivedSelections {
    fn add(&mut self, bytes: Vec<u8>) -> Result<ObjectRef> {
        let reference = ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        if let Some(prior) = self.metadata.get(&reference.sha256) {
            ensure!(prior == &bytes, "archive metadata collision");
        } else {
            self.metadata.insert(reference.sha256.clone(), bytes);
        }
        ensure!(
            self.metadata.len() <= MAX_ITEMS
                && self.metadata.values().map(Vec::len).sum::<usize>() <= MAX_METADATA,
            "archive selected metadata exceeds bound"
        );
        Ok(reference)
    }
    fn include_chain(
        &mut self,
        root: &Path,
        directory: &Path,
        head: SelectionTransaction,
    ) -> Result<()> {
        for transaction in publication::history_directory(
            directory,
            self.archive.origin_generation,
            &head.scope.clone(),
            head,
        )? {
            publication::validate_closure(root, &transaction)?;
            self.receipts += 1;
            ensure!(
                self.receipts <= MAX_ITEMS,
                "archive receipt count exceeds bound"
            );
            for manifest in std::iter::once(&transaction.selected).chain(&transaction.retained) {
                if let Some(prior) = self
                    .manifests
                    .insert(manifest.transaction_id, manifest.clone())
                {
                    ensure!(prior == *manifest, "archive manifest identity collision");
                }
            }
            if let Some(hash) = transaction.previous_sha256 {
                let bytes = files::read(&directory.join(&hash), MAX_METADATA as u64)?;
                ensure!(
                    self.add(bytes)?.sha256 == hash,
                    "archive history integrity failure"
                );
            }
        }
        Ok(())
    }
}

pub(crate) fn collect(root: &Path, actor: Uuid, generation: Uuid) -> Result<ArchivedSelections> {
    validate_generation(root, actor, generation)?;
    ensure!(
        has_selected(root)?,
        "selected backup requires selected metadata"
    );
    let mut collected = ArchivedSelections {
        archive: SelectionArchive {
            origin_actor: actor,
            origin_generation: generation,
            feature: ObjectRef {
                sha256: String::new(),
                bytes: 0,
            },
            heads: vec![],
        },
        metadata: BTreeMap::new(),
        manifests: BTreeMap::new(),
        receipts: 0,
    };
    collected.archive.feature = collected.add(files::read(
        &root.join("selection-feature.json"),
        MAX_RECORD as u64,
    )?)?;
    let mut scopes = BTreeSet::new();
    for entry in fs::read_dir(root.join("selections"))? {
        let path = entry?.path();
        let bytes = files::read(&path, MAX_METADATA as u64)?;
        let head: SelectionTransaction =
            serde_json::from_slice(&bytes).context("unsupported archived selection")?;
        let filename = head.scope.filename()?;
        ensure!(
            path.file_name().and_then(|n| n.to_str()) == Some(filename.as_str())
                && scopes.insert(filename),
            "archive selection identity mismatch"
        );
        let reference = collected.add(bytes)?;
        collected.archive.heads.push(reference);
        collected.include_chain(root, &root.join("selection-history"), head)?;
    }
    collected
        .archive
        .heads
        .sort_by(|a, b| a.sha256.cmp(&b.sha256));
    Ok(collected)
}

impl SelectionArchive {
    pub(crate) fn validate(&self, root: &Path) -> Result<ArchivedSelections> {
        ensure!(
            !self.origin_actor.is_nil()
                && !self.origin_generation.is_nil()
                && !self.heads.is_empty()
                && self.heads.len() <= MAX_ITEMS,
            "invalid archive selection origin/bounds"
        );
        let read = |reference: &ObjectRef, limit: usize| -> Result<Vec<u8>> {
            ensure!(
                valid_digest(&reference.sha256) && reference.bytes <= limit as u64,
                "archive metadata reference exceeds bound"
            );
            let bytes = files::read(&root.join("objects").join(&reference.sha256), limit as u64)?;
            ensure!(
                bytes.len() as u64 == reference.bytes && digest(&bytes) == reference.sha256,
                "archive metadata integrity failure"
            );
            Ok(bytes)
        };
        let feature_bytes = read(&self.feature, MAX_RECORD)?;
        let feature: SelectionFeature =
            serde_json::from_slice(&feature_bytes).context("unsupported archived feature")?;
        ensure!(
            feature
                == SelectionFeature {
                    format: FORMAT,
                    actor: self.origin_actor,
                    generation: self.origin_generation
                },
            "foreign or unsupported archived feature"
        );
        let mut collected = ArchivedSelections {
            archive: self.clone(),
            metadata: BTreeMap::new(),
            manifests: BTreeMap::new(),
            receipts: 0,
        };
        collected.add(feature_bytes)?;
        let mut scopes = BTreeSet::new();
        for reference in &self.heads {
            let bytes = read(reference, MAX_METADATA)?;
            let head: SelectionTransaction =
                serde_json::from_slice(&bytes).context("unsupported archived selection")?;
            ensure!(
                scopes.insert(head.scope.filename()?),
                "duplicate archived selection scope"
            );
            collected.add(bytes)?;
            collected.include_chain(root, &root.join("objects"), head)?;
        }
        Ok(collected)
    }
}
