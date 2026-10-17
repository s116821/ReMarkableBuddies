//! Explicit, code-registered generation migrations. No inferred domain conversion.
use super::*;

pub trait GenerationMigration {
    fn source_version(&self) -> u32;
    fn target_version(&self) -> u32;
    /// Transform only the staged generation. Never touch CURRENT or user roots.
    fn transform(&self, staged: &Path) -> Result<()>;
    fn validate(&self, staged: &Path) -> Result<()>;
}
#[derive(Default)]
pub struct MigrationRegistry {
    entries: BTreeMap<(u32, u32), Box<dyn GenerationMigration>>,
}
impl MigrationRegistry {
    pub fn register(&mut self, migration: impl GenerationMigration + 'static) -> Result<()> {
        let key = (migration.source_version(), migration.target_version());
        ensure!(
            key.0 > 0 && key.1 > key.0 && !self.entries.contains_key(&key),
            "invalid or duplicate migration registration"
        );
        self.entries.insert(key, Box::new(migration));
        Ok(())
    }
}
impl Store {
    /// The shipped v1 registry is empty. Future releases register explicit readers
    /// and adapters; a version number alone never authorizes rewriting data.
    pub fn migrate(&self, registry: &MigrationRegistry, target: u32) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex unavailable"))?;
        let source = self.generation(inner.generation);
        let from: u32 = files::json(&source.join("format.json"), 64)?;
        let migration = registry
            .entries
            .get(&(from, target))
            .context("unsupported migration path")?;
        ensure!(
            inner.index.unavailable == 0,
            "cannot migrate unavailable committed data"
        );
        let next = Uuid::new_v4();
        Self::create_generation(&self.paths.data, next)?;
        let staged = self.generation(next);
        for manifest in inner.index.manifests.values() {
            for item in manifest.records.iter().chain(&manifest.media) {
                let path = source.join("objects").join(&item.sha256);
                files::safe_path(&path)?;
                files::copy_verified(
                    &staged.join("objects").join(&item.sha256),
                    File::open(path)?,
                    item,
                )?;
            }
            files::atomic_json(
                &staged
                    .join("commits")
                    .join(format!("{}.json", manifest.transaction_id)),
                manifest,
            )?;
        }
        migration.transform(&staged)?;
        migration.validate(&staged)?;
        let rebuilt = self.recover(next)?;
        ensure!(
            rebuilt.unavailable == 0,
            "migrated generation failed integrity validation"
        );
        files::atomic_json(&staged.join("format.json"), &target)?;
        Self::trip(&mut inner, Fault::BeforeActivation)?;
        files::atomic_json(&self.paths.data.join("CURRENT"), &next)?;
        inner.generation = next;
        inner.index = rebuilt;
        Self::trip(&mut inner, Fault::AfterActivation)?;
        Ok(())
    }
}
