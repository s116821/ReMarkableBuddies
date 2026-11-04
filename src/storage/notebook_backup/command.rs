use super::*;
use crate::storage::maintenance::{ExistingCustody, Refusal};
use std::io::Read;

const UNIQUE_BYTES: u64 = 1024 * 1024 * 1024;
const WORK_BYTES: u64 = 8 * UNIQUE_BYTES;
const MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const REFERENCES: usize = 16_384;

fn budget() -> selection::SelectedReadBudget {
    // Both archive validation passes may revisit every manifest reference.
    selection::SelectedReadBudget::combined_bytes((REFERENCES * 4) as u64, WORK_BYTES)
}

pub(super) fn regular_if_present(path: &Path) -> Result<bool> {
    files::safe_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(meta.is_file(), "notebook archive requires regular files");
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn directory(path: &Path) -> Result<()> {
    files::safe_path(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "notebook archive directory required"
    );
    Ok(())
}

fn empty_if_present(path: &Path) -> Result<()> {
    files::safe_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_dir() && fs::read_dir(path)?.next().transpose()?.is_none(),
                "selected notebook custody unsupported"
            );
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn check_destination(paths: &StorePaths, destination: &Path) -> Result<()> {
    use crate::{restore_maintenance::overlap, storage::inspection::canonical_with_missing_tail};

    paths.validate()?;
    files::safe_path(destination)?;
    let parent = destination.parent().context("archive parent absent")?;
    directory(parent)?;
    let destination = parent
        .canonicalize()?
        .join(destination.file_name().context("archive name absent")?);
    let canonical =
        |path: &Path| canonical_with_missing_tail(path).context("owned root unavailable");
    let roots = [
        canonical(&paths.data)?,
        canonical(&paths.cache)?,
        canonical(&paths.credentials)?,
    ];
    for (index, root) in roots.iter().enumerate() {
        for other in roots[index + 1..].iter().chain([&destination]) {
            ensure!(
                !overlap(root, other).map_err(|_| anyhow::anyhow!("path comparison failed"))?,
                "notebook archive paths overlap"
            );
        }
    }
    Ok(())
}

pub(crate) struct ExportSource {
    custody: ExistingCustody,
    budget: selection::SelectedReadBudget,
}

impl ExportSource {
    pub(crate) fn open(paths: StorePaths) -> std::result::Result<Self, Refusal> {
        let mut budget = budget();
        // Reserve the bounded Config::load before charging actual owned content.
        budget
            .charge(0, 0, MAX_RECORD as u64, 0)
            .map_err(|_| Refusal::Unsupported)?;
        let custody = ExistingCustody::open(paths, |_, _| Ok(()), Some(&mut budget))?;
        Ok(Self { custody, budget })
    }

    pub(crate) fn prepare(&mut self) -> Result<PreparedExport> {
        let ExistingCustody {
            paths,
            actor,
            generation,
            ..
        } = &self.custody;
        let root = paths.data.join("generations").join(generation.to_string());
        for path in [&root, &root.join("commits"), &root.join("objects")] {
            directory(path)?;
        }
        for name in [
            notebook_generation::MARKER,
            feedback_generation::MARKER,
            "selection-feature.json",
        ] {
            regular_if_present(&root.join(name))?;
        }
        // Reserve upper bounds for the fixed existing readiness validator:
        // current, repeated format and notebook reads, three features, contract
        // and capabilities. This covers even growth up to each legacy cap.
        self.budget.charge(0, 0, 8 * MAX_RECORD as u64, 0)?;
        notebook_generation::validate_ready(paths, *actor, *generation)?;
        ensure!(
            sync::mode::ordinary_local_existing(paths, *actor, &mut self.budget)?,
            "notebook export requires local mode"
        );
        empty_if_present(&root.join("selections"))?;
        empty_if_present(&root.join("selection-history"))?;
        let manifests = manifests(&root, &mut self.budget)?;
        let backup = NotebookBackup::build(
            paths,
            &root,
            *actor,
            *generation,
            manifests,
            Some(&mut self.budget),
        )?;
        archive_limits(&backup, UNIQUE_BYTES)?;
        let bytes = backup.encode()?;
        backup.inspect_with_budget(&root, Some(&mut self.budget))?;
        Ok(PreparedExport {
            root,
            backup,
            bytes,
        })
    }

    pub(crate) fn export(
        &mut self,
        prepared: PreparedExport,
        destination: &Path,
    ) -> Result<(BackupInspection, Witness)> {
        let PreparedExport {
            root,
            backup,
            bytes,
        } = prepared;
        check_destination(&self.custody.paths, destination)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(destination)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(destination)?;
        fs::create_dir(destination.join("objects"))?;
        for item in &backup.objects {
            self.budget.charge(0, 0, 0, item.bytes)?;
            let source = root.join("objects").join(&item.sha256);
            ensure!(regular_if_present(&source)?, "notebook object unavailable");
            let mut input = File::open(source)?;
            ensure!(
                input.metadata()?.len() == item.bytes,
                "notebook object size changed"
            );
            let output = destination.join("objects").join(&item.sha256);
            ensure!(
                !regular_if_present(&output)?,
                "archive object already exists"
            );
            files::copy_verified(&output, (&mut input).take(item.bytes), item)?;
            ensure!(
                input.metadata()?.len() == item.bytes,
                "notebook object grew"
            );
        }
        files::sync_dir(&destination.join("objects"))?;
        files::publish_archive_marker(destination, &bytes)?;
        let (report, witness) = inspect_with_budget(destination, &mut self.budget)?;
        ensure!(witness.marker == digest(&bytes), "committed marker changed");
        Ok((report, witness))
    }

    pub(crate) fn verify(&mut self, witness: &Witness) -> Result<()> {
        witness.verify_with_budget(&mut self.budget)
    }
}

pub(crate) struct PreparedExport {
    root: PathBuf,
    backup: NotebookBackup,
    bytes: Vec<u8>,
}

fn manifests(root: &Path, budget: &mut selection::SelectedReadBudget) -> Result<Vec<Manifest>> {
    let mut manifests = BTreeMap::new();
    let mut remaining = MANIFEST_BYTES;
    let mut references = 0;
    for (index, entry) in fs::read_dir(root.join("commits"))?.enumerate() {
        ensure!(index < MAX_ITEMS, "notebook commit inventory bound");
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid commit filename")?;
        if let Some(stage) = name.strip_prefix(".stage-") {
            let id = Uuid::parse_str(stage)?;
            ensure!(
                stage == id.to_string() && regular_if_present(&path)?,
                "invalid commit staging"
            );
            continue;
        }
        let bytes =
            files::read_with_budget(&path, remaining.min(MAX_METADATA as u64), Some(budget))?;
        remaining -= bytes.len() as u64;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        references += manifest.records.len() + manifest.media.len();
        ensure!(references <= REFERENCES, "notebook archive reference bound");
        ensure!(
            name == format!("{}.json", manifest.transaction_id)
                && matches!(manifest.scope, Scope::LocalTransaction)
                && manifests
                    .insert(manifest.transaction_id, manifest)
                    .is_none(),
            "invalid ordinary notebook commit"
        );
    }
    Ok(manifests.into_values().collect())
}

fn archive_limits(backup: &NotebookBackup, unique_bytes: u64) -> Result<()> {
    ensure!(
        backup.manifests.len() <= MAX_ITEMS && backup.objects.len() <= MAX_ITEMS,
        "notebook archive inventory bound"
    );
    let mut bytes = 0u64;
    for object in &backup.objects {
        bytes = bytes
            .checked_add(object.bytes)
            .context("notebook archive size overflow")?;
        ensure!(bytes <= unique_bytes, "notebook archive capacity");
    }
    let mut references = 0usize;
    for manifest in &backup.manifests {
        manifest.validate()?;
        references += manifest.records.len() + manifest.media.len();
        ensure!(references <= REFERENCES, "notebook archive reference bound");
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: std::time::SystemTime,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    id: (u64, u64),
}

fn stamp(path: &Path) -> Result<Stamp> {
    files::safe_path(path)?;
    let meta = fs::symlink_metadata(path)?;
    Ok(Stamp {
        len: meta.len(),
        modified: meta.modified()?,
        created: meta.created().ok(),
        #[cfg(unix)]
        id: {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        },
    })
}

fn inventory(root: &Path, names: &BTreeSet<String>) -> Result<BTreeMap<PathBuf, Stamp>> {
    let objects = root.join("objects");
    directory(root)?;
    directory(&objects)?;
    let mut inventory = BTreeMap::new();
    let allowed = BTreeSet::from([root.join("backup.json"), objects.clone()]);
    let mut present = BTreeSet::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        ensure!(allowed.contains(&path), "unexpected archive root entry");
        present.insert(path);
    }
    ensure!(present == allowed, "incomplete archive root");
    for path in [root.to_owned(), objects.clone(), root.join("backup.json")] {
        inventory.insert(path.clone(), stamp(&path)?);
    }
    let mut present = BTreeSet::new();
    for entry in fs::read_dir(&objects)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid object name")?;
        ensure!(
            names.contains(name) && regular_if_present(&path)?,
            "unexpected archive object"
        );
        present.insert(name.to_owned());
        inventory.insert(path.clone(), stamp(&path)?);
    }
    ensure!(&present == names, "missing archive object");
    Ok(inventory)
}

pub(crate) struct Witness {
    root: PathBuf,
    marker: String,
    names: BTreeSet<String>,
    inventory: BTreeMap<PathBuf, Stamp>,
}

impl Witness {
    fn verify_with_budget(&self, budget: &mut selection::SelectedReadBudget) -> Result<()> {
        let marker = files::read_with_budget(
            &self.root.join("backup.json"),
            MAX_METADATA as u64,
            Some(budget),
        )?;
        ensure!(
            digest(&marker) == self.marker && inventory(&self.root, &self.names)? == self.inventory,
            "notebook archive changed"
        );
        Ok(())
    }
}

pub(crate) struct Detached {
    witness: Witness,
    budget: selection::SelectedReadBudget,
}

impl Detached {
    pub(crate) fn inspect(path: &Path) -> Result<(BackupInspection, Self)> {
        let mut budget = budget();
        let (report, witness) = inspect_with_budget(path, &mut budget)?;
        Ok((report, Self { witness, budget }))
    }

    pub(crate) fn verify(&mut self) -> Result<()> {
        self.witness.verify_with_budget(&mut self.budget)
    }
}

fn inspect_with_budget(
    path: &Path,
    budget: &mut selection::SelectedReadBudget,
) -> Result<(BackupInspection, Witness)> {
    directory(path)?;
    let marker =
        files::read_with_budget(&path.join("backup.json"), MAX_METADATA as u64, Some(budget))?;
    let backup = serde_json::from_slice::<NotebookBackup>(&marker)?.admit_format()?;
    archive_limits(&backup, UNIQUE_BYTES)?;
    let names = backup
        .objects
        .iter()
        .map(|object| object.sha256.clone())
        .collect();
    let witness = Witness {
        root: path.to_owned(),
        marker: digest(&marker),
        inventory: inventory(path, &names)?,
        names,
    };
    let report = backup.inspect_with_budget(path, Some(budget))?;
    witness.verify_with_budget(budget)?;
    Ok((report, witness))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notebook_archive_marker_reads_charge_the_same_reduced_budget() {
        let root = std::env::temp_dir().join(format!("archive-budget-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let store = Store::prepare_notebook(paths).unwrap();
        let archive = root.join("archive");
        store.export_notebook_backup(&archive).unwrap();
        drop(store);
        let size = fs::metadata(archive.join("backup.json")).unwrap().len();
        let mut exact = selection::SelectedReadBudget::combined_bytes(4, 2 * size);
        let mut short = selection::SelectedReadBudget::combined_bytes(4, 2 * size - 1);
        assert!(inspect_with_budget(&archive, &mut exact).is_ok());
        assert_eq!(exact.usage().metadata_bytes, 2 * size);
        assert!(inspect_with_budget(&archive, &mut short).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn notebook_archive_combined_work_and_unique_capacity_have_exact_edges() {
        let mut budget = selection::SelectedReadBudget::combined_bytes(4, 12);
        budget.charge(0, 1, 5, 0).unwrap();
        budget.charge(0, 1, 0, 7).unwrap();
        assert!(budget.charge(0, 0, 1, 0).is_err());
        assert!(budget.exhausted());

        let backup = NotebookBackup {
            format: 4,
            origin_actor: Uuid::new_v4(),
            origin_generation: Uuid::new_v4(),
            notebook_feature: 1,
            feedback_feature: false,
            selected_feature: false,
            manifests: vec![],
            objects: vec![ObjectRef {
                sha256: digest(b"fixture"),
                bytes: 12,
            }],
            complete_media: true,
            config_snapshot: None,
        };
        archive_limits(&backup, 12).unwrap();
        assert!(archive_limits(&backup, 11).is_err());
    }
}
