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

/// The service supplies fresh Reader facts and its ordinary ownership ledger.
/// Missing scene data is never evidence of a blank.
pub fn acquire_saved_notebook(
    facts: &ReaderFacts,
    ledger: &Ledger,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
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
        let source = acquire(
            Path::new(INSTALLED_ROOT),
            Path::new(TEMPLATES),
            (0, Some(0)),
            false,
            facts,
            &excluded,
            &mut progress,
        )?;
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

    fn scene(&mut self, dirs: &[Directory], name: String) -> Result<String> {
        let digest = self
            .read(dirs, 1, name, ASSET_BYTES, None)
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

    fn optional(
        &mut self,
        dirs: &[Directory],
        dir: usize,
        name: String,
        max: usize,
    ) -> Result<Option<String>> {
        self.progress.poll()?;
        if exists(&dirs[dir], &name)? {
            return self.read(dirs, dir, name, max, None).map(Some);
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

#[derive(Serialize)]
struct Original<'a> {
    id: &'a str,
    page_layout: &'a Value,
    scene_sha256: &'a str,
    template: &'a [Option<String>],
}

fn acquire(
    root: &Path,
    templates: &Path,
    owner: (u32, Option<u32>),
    fixture: bool,
    facts: &ReaderFacts,
    excluded: &HashSet<Uuid>,
    progress: &mut SourceProgress<'_>,
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
        Directory::open(root, owner, fixture, &mut inputs.work)?,
        Directory::open(
            &root.join(document.to_string()),
            owner,
            fixture,
            &mut inputs.work,
        )?,
        Directory::open(templates, owner, fixture, &mut inputs.work)?,
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
    let page_data = inputs.optional(&dirs, 0, format!("{document}.pagedata"), TABLE_BYTES)?;

    let mut registry_bytes = Vec::new();
    let registry_sha256 = inputs.read(
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
            .context("Missing original template assignment")?
            .to_owned();
        if !template_assets.contains_key(&template) {
            let mut entries = registry.iter().filter(|entry| entry["name"] == template);
            let stem = entries
                .next()
                .and_then(|entry| entry["filename"].as_str())
                .context("Unresolved stock template asset")?;
            ensure!(
                entries.next().is_none()
                    && !stem.is_empty()
                    && stem.len() < 128
                    && stem
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_ -".contains(&c)),
                "Ambiguous or invalid stock template"
            );
            let mut assets = Vec::new();
            for extension in ["svg", "png"] {
                assets.push(inputs.optional(
                    &dirs,
                    2,
                    format!("{stem}.{extension}"),
                    ASSET_BYTES,
                )?);
            }
            ensure!(
                assets.iter().any(Option::is_some),
                "Stock template assets absent"
            );
            template_assets.insert(template.clone(), assets);
        }
        page_layout
            .as_object_mut()
            .context("Invalid original page layout")?
            .remove("idx");
        let scene_sha256 = inputs.scene(&dirs, format!("{id}.rm"))?;
        originals.push((id, page_layout, scene_sha256, template));
    }
    inputs.verify(&dirs)?;
    let exact = originals
        .iter()
        .map(|(id, layout, digest, template)| Original {
            id,
            page_layout: layout,
            scene_sha256: digest,
            template: &template_assets[template],
        })
        .collect::<Vec<_>>();
    let revision = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            "saved-notebook-source-v1",
            document.to_string(),
            layout,
            page_data,
            registry_sha256,
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
