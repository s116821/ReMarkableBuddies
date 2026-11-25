//! Saved inputs for an admitted ordinary notebook. No pixels or publication permit.
use super::{open_at, Asset, Directory, SourceProgress, INSTALLED_ROOT, TABLE_BYTES};
use crate::{
    conversation::{Ledger, SourceObservation},
    device::native_page::ordered_pages,
};
use anyhow::{ensure, Context, Result};
use nix::libc;
use remarkable_open_sdk::{native_adapter::ReaderFacts, Uuid};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    io,
    os::unix::fs::FileExt,
    path::Path,
    time::Instant,
};

const TEMPLATES: &str = "/usr/share/remarkable/templates";
const MAX_PAGES: usize = 256;
const ASSET_BYTES: usize = 16 * 1024 * 1024;
const TOTAL_BYTES: usize = 64 * 1024 * 1024;
const SCENE_HEADER: &[u8; 43] = b"reMarkable .lines file, version=6          ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedNotebookSource {
    document: Uuid,
    original_pages: Vec<Uuid>,
    revision: String,
}

impl SavedNotebookSource {
    pub fn document(&self) -> Uuid {
        self.document
    }

    pub fn original_pages(&self) -> &[Uuid] {
        &self.original_pages
    }

    /// Identity of the persisted inputs, independent of render output.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Supply the receiver's independent saved-content field. Native order,
    /// rendered images and effect admission still come from their own guards.
    pub fn bind_content_revision(&self, observation: &mut SourceObservation) -> Result<()> {
        ensure!(
            observation.document.as_bytes() == self.document.as_bytes()
                && self
                    .original_pages
                    .iter()
                    .any(|page| { observation.page.as_bytes() == page.as_bytes() }),
            "Saved notebook revision does not match the observed original page"
        );
        observation.content_revision = Some(self.revision.clone());
        Ok(())
    }
}

/// No branch of this result supplies full-page images, including blank images.
#[derive(Debug)]
pub enum NotebookSourceAcquisition {
    SavedInputs(SavedNotebookSource),
    Unavailable(anyhow::Error),
}

/// The exact source bytes read for an original page's saved revision.
pub struct SavedNotebookPage<'a> {
    pub page: Uuid,
    pub scene: &'a [u8],
    pub template_stem: &'a str,
    pub template_svg: Option<&'a [u8]>,
    pub template_png: Option<&'a [u8]>,
}

pub type SavedNotebookPageConsumer<'a> =
    dyn for<'page> FnMut(SavedNotebookPage<'page>) -> Result<()> + 'a;

/// The service supplies fresh Reader facts and its ordinary ownership ledger.
/// Missing scene data is never evidence of a blank.
pub fn acquire_saved_notebook(
    facts: &ReaderFacts,
    ledger: &Ledger,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> NotebookSourceAcquisition {
    acquire_installed(facts, ledger, deadline, progress, None)
}

/// Visit the exact bounded inputs in original order while their handles are pinned.
/// The consumer must discard any outputs if acquisition returns `Unavailable`.
pub fn acquire_saved_notebook_with_pages(
    facts: &ReaderFacts,
    ledger: &Ledger,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
    consume: &mut SavedNotebookPageConsumer<'_>,
) -> NotebookSourceAcquisition {
    acquire_installed(facts, ledger, deadline, progress, Some(consume))
}

fn acquire_installed(
    facts: &ReaderFacts,
    ledger: &Ledger,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
    consume: Option<&mut SavedNotebookPageConsumer<'_>>,
) -> NotebookSourceAcquisition {
    let result = (|| {
        let mut progress = SourceProgress::new(deadline, progress);
        ensure!(
            facts.order().len() <= MAX_PAGES,
            "Saved notebook page limit"
        );
        let document = uuid::Uuid::from_bytes(*facts.page().document.as_bytes());
        let pages: Vec<_> = facts
            .order()
            .iter()
            .map(|page| uuid::Uuid::from_bytes(*page.as_bytes()))
            .collect();
        progress.boundary()?;
        let before = ledger.notebook_page_associations(document, &pages, || progress.poll())?;
        let excluded = before
            .iter()
            .map(|association| {
                Uuid::parse(&association.receipt().observed_page.to_string())
                    .context("Invalid Buddy page identity")
            })
            .collect::<Result<HashSet<_>>>()?;
        let paths = SourcePaths {
            root: Path::new(INSTALLED_ROOT),
            templates: Path::new(TEMPLATES),
            owner: (0, Some(0)),
            fixture: false,
        };
        #[cfg(all(test, feature = "installed-resident-output"))]
        let fixture = FIXTURE_PATHS.with(|paths| paths.borrow().clone());
        #[cfg(all(test, feature = "installed-resident-output"))]
        let paths = fixture
            .as_ref()
            .map_or(paths, |(root, templates)| SourcePaths {
                root,
                templates,
                owner: (unsafe { libc::geteuid() }, None),
                fixture: true,
            });
        let source = acquire(paths, facts, &excluded, &mut progress, consume)?;
        let after = ledger.notebook_page_associations(document, &pages, || progress.poll())?;
        ensure!(before == after, "Notebook Buddy ownership changed");
        progress.check_time()?;
        Ok(source)
    })();
    match result {
        Ok(source) => NotebookSourceAcquisition::SavedInputs(source),
        Err(error) => NotebookSourceAcquisition::Unavailable(error),
    }
}

#[cfg(all(test, feature = "installed-resident-output"))]
thread_local! {
    static FIXTURE_PATHS: std::cell::RefCell<Option<(std::path::PathBuf, std::path::PathBuf)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, feature = "installed-resident-output"))]
pub(crate) fn with_fixture_paths<T>(root: &Path, templates: &Path, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FIXTURE_PATHS.with(|paths| paths.borrow_mut().take());
        }
    }

    FIXTURE_PATHS.with(|paths| {
        let mut paths = paths.borrow_mut();
        assert!(paths.is_none());
        *paths = Some((root.to_owned(), templates.to_owned()));
    });
    let _reset = Reset;
    run()
}

fn exists(dir: &Directory, name: &str) -> Result<bool> {
    match open_at(dir.leaf(), OsStr::new(name), libc::O_PATH) {
        Ok(_) => Ok(true),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

struct Inputs<'a, 'p> {
    assets: Vec<(usize, Asset)>,
    missing: Vec<(usize, String)>,
    bytes: usize,
    reads: usize,
    work: usize,
    progress: &'p mut SourceProgress<'a>,
}

impl Inputs<'_, '_> {
    fn read(
        &mut self,
        dirs: &[Directory],
        dir: usize,
        name: String,
        max: usize,
        raw: Option<&mut Vec<u8>>,
    ) -> Result<String> {
        self.progress.poll()?;
        let asset = Asset::open(&dirs[dir], name, max, &mut self.work)?;
        let digest = asset.read(raw, &mut self.reads, &mut self.bytes, self.progress)?;
        self.assets.push((dir, asset));
        Ok(digest)
    }

    fn scene(
        &mut self,
        dirs: &[Directory],
        name: String,
        raw: Option<&mut Vec<u8>>,
    ) -> Result<String> {
        let digest = self
            .read(dirs, 1, name, ASSET_BYTES, raw)
            .context("Original saved scene unavailable; blank has not been proven")?;
        let (_, asset) = self.assets.last().context("Saved scene pin unavailable")?;
        let mut header = [0; SCENE_HEADER.len()];
        ensure!(
            asset.stamp.size > SCENE_HEADER.len() as u64
                && asset.file.read_exact_at(&mut header, 0).is_ok()
                && &header == SCENE_HEADER,
            "Unsupported saved scene; blank has not been proven"
        );
        Ok(digest)
    }

    fn template_asset(
        &mut self,
        dirs: &[Directory],
        name: String,
        retain: bool,
    ) -> Result<Option<TemplateAsset>> {
        let dir = 2;
        self.progress.poll()?;
        if exists(&dirs[dir], &name)? {
            let mut bytes = retain.then(Vec::new);
            let digest = self.read(dirs, dir, name, ASSET_BYTES, bytes.as_mut())?;
            return Ok(Some(TemplateAsset { digest, bytes }));
        }
        self.missing.push((dir, name));
        Ok(None)
    }

    fn verify(&mut self, dirs: &[Directory]) -> Result<()> {
        self.progress.boundary()?;
        let current = dirs
            .iter()
            .map(|dir| dir.current(&mut self.work))
            .collect::<Result<Vec<_>>>()?;
        for (dir, asset) in &self.assets {
            self.progress.poll()?;
            asset.current(&current[*dir], &mut self.work)?;
        }
        for (dir, name) in &self.missing {
            self.progress.poll()?;
            ensure!(
                !exists(&current[*dir], name)?,
                "Saved notebook input appeared during read"
            );
        }
        self.progress.check_time()
    }
}

struct TemplateAsset {
    digest: String,
    bytes: Option<Vec<u8>>,
}

struct TemplateInputs {
    svg: Option<TemplateAsset>,
    png: Option<TemplateAsset>,
}

struct SourcePaths<'a> {
    root: &'a Path,
    templates: &'a Path,
    owner: (u32, Option<u32>),
    fixture: bool,
}

#[derive(Serialize)]
struct Original<'a> {
    id: &'a str,
    page_layout: &'a Value,
    scene_sha256: &'a str,
    template_svg_sha256: Option<&'a str>,
    template_png_sha256: Option<&'a str>,
}

fn acquire(
    paths: SourcePaths<'_>,
    facts: &ReaderFacts,
    excluded: &HashSet<Uuid>,
    progress: &mut SourceProgress<'_>,
    mut consume: Option<&mut SavedNotebookPageConsumer<'_>>,
) -> Result<SavedNotebookSource> {
    let document = facts.page().document;
    ensure!(
        facts.order().len() <= MAX_PAGES,
        "Saved notebook page limit"
    );
    ensure!(
        !excluded.contains(&facts.page().page)
            && excluded.iter().all(|page| facts.order().contains(page)),
        "Invalid Buddy exclusion for the observed source"
    );
    let mut inputs = Inputs {
        assets: Vec::new(),
        missing: Vec::new(),
        bytes: TOTAL_BYTES,
        reads: 8192,
        work: 16384,
        progress,
    };
    inputs.progress.boundary()?;
    let dirs = [
        Directory::open(paths.root, paths.owner, paths.fixture, &mut inputs.work)?,
        Directory::open(
            &paths.root.join(document.to_string()),
            paths.owner,
            paths.fixture,
            &mut inputs.work,
        )?,
        Directory::open(
            paths.templates,
            paths.owner,
            paths.fixture,
            &mut inputs.work,
        )?,
    ];
    let mut content = Vec::new();
    inputs.read(
        &dirs,
        0,
        format!("{document}.content"),
        TABLE_BYTES,
        Some(&mut content),
    )?;
    let mut layout = crate::config::inspection::strict_json(&content)?;
    ensure!(
        layout["fileType"] == "notebook" && layout["formatVersion"] == 2,
        "Unsupported saved notebook document"
    );
    ensure!(
        layout.get("fontName").is_none_or(|font| font == ""),
        "Saved notebook uses an unresolved font asset"
    );
    let order = ordered_pages(&layout)?;
    ensure!(
        order.len() == facts.order().len()
            && order
                .iter()
                .zip(facts.order())
                .all(|(page, id)| page.id == id.to_string() && page.redirect.is_none()),
        "Saved notebook order or backing differs"
    );
    for extension in ["pdf", "epub"] {
        let name = format!("{document}.{extension}");
        ensure!(
            !exists(&dirs[0], &name)?,
            "Saved notebook backing is unsupported"
        );
        inputs.missing.push((0, name));
    }
    let mut registry_bytes = Vec::new();
    inputs.read(
        &dirs,
        2,
        "templates.json".into(),
        TABLE_BYTES,
        Some(&mut registry_bytes),
    )?;
    let registry = crate::config::inspection::strict_json(&registry_bytes)?;
    let registry = registry["templates"]
        .as_array()
        .context("Missing stock template registry")?;
    let mut page_rows = BTreeMap::new();
    let source_rows = layout["cPages"]
        .as_object_mut()
        .context("Missing saved page table")?
        .remove("pages")
        .context("Missing saved page rows")?;
    for row in source_rows.as_array().context("Missing saved page rows")? {
        if row["deleted"]["value"] != true {
            let id = row["id"].as_str().context("Missing saved page ID")?;
            ensure!(
                page_rows.insert(id, row.clone()).is_none(),
                "Duplicate saved page"
            );
        }
    }
    layout["cPages"]
        .as_object_mut()
        .expect("validated page table")
        .remove("lastOpened");
    let fields = layout
        .as_object_mut()
        .context("Invalid saved notebook layout")?;
    for field in ["pageCount", "sizeInBytes", "pageTags", "tags"] {
        fields.remove(field);
    }

    let mut template_assets = BTreeMap::new();
    let mut originals = Vec::new();
    for page in facts.order().iter().filter(|page| !excluded.contains(page)) {
        let id = page.to_string();
        let mut page_layout = page_rows
            .remove(id.as_str())
            .context("Missing original row")?;
        let template = page_layout["template"]["value"]
            .as_str()
            .filter(|template| !template.is_empty())
            // This producer only admits explicit cPages assignments.
            .context("Missing explicit original template assignment")?
            .to_owned();
        if !template_assets.contains_key(&template) {
            ensure!(
                template.len() < 128
                    && template
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_ -".contains(&c)),
                "Invalid stock template asset stem"
            );
            ensure!(
                registry.iter().any(|entry| entry["filename"] == template),
                "Unregistered stock template asset stem"
            );
            let retain = consume.is_some();
            let svg = inputs.template_asset(&dirs, format!("{template}.svg"), retain)?;
            let png = inputs.template_asset(&dirs, format!("{template}.png"), retain)?;
            ensure!(
                svg.is_some() || png.is_some(),
                "Stock template assets absent"
            );
            template_assets.insert(template.clone(), TemplateInputs { svg, png });
        }
        page_layout
            .as_object_mut()
            .context("Invalid original page layout")?
            .remove("idx");
        let mut scene = consume.is_some().then(Vec::new);
        let scene_sha256 = inputs.scene(&dirs, format!("{id}.rm"), scene.as_mut())?;
        if let (Some(consume), Some(scene)) = (consume.as_mut(), scene.as_deref()) {
            let assets = &template_assets[&template];
            inputs.progress.poll()?;
            consume(SavedNotebookPage {
                page: *page,
                scene,
                template_stem: &template,
                template_svg: assets.svg.as_ref().and_then(|asset| asset.bytes.as_deref()),
                template_png: assets.png.as_ref().and_then(|asset| asset.bytes.as_deref()),
            })?;
            inputs.progress.poll()?;
        }
        originals.push((id, page_layout, scene_sha256, template));
    }
    inputs.verify(&dirs)?;
    let exact = originals
        .iter()
        .map(|(id, layout, digest, template)| {
            let assets = &template_assets[template];
            Original {
                id,
                page_layout: layout,
                scene_sha256: digest,
                template_svg_sha256: assets.svg.as_ref().map(|asset| asset.digest.as_str()),
                template_png_sha256: assets.png.as_ref().map(|asset| asset.digest.as_str()),
            }
        })
        .collect::<Vec<_>>();
    let revision = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            "saved-notebook-source-v1",
            document.to_string(),
            layout,
            exact,
        ))?)
    );
    Ok(SavedNotebookSource {
        document,
        original_pages: facts
            .order()
            .iter()
            .copied()
            .filter(|page| !excluded.contains(page))
            .collect(),
        revision,
    })
}

#[cfg(test)]
mod tests;
