use super::*;
use remarkable_open_sdk::PageKey;
use serde_json::json;
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf, time::Duration};

fn private_write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes.as_ref())
}

// Reduced from the pinned public stock registry. Assets below are synthetic.
const STOCK_REGISTRY: &str = r#"{"templates":[
    {"filename":"Blank","name":"Blank"},
    {"filename":"Blank","name":"Blank","landscape":true},
    {"filename":"P Lines medium","name":"Lined medium"},
    {"filename":"LS Lines medium","name":"Lined medium","landscape":true}
]}"#;
const BLANK_SVG: &str = "<svg/>";
const PORTRAIT_SVG: &str = "<svg><path id='portrait'/></svg>";
const LANDSCAPE_PNG: &[u8] = b"bounded landscape template fixture bytes";

struct Fixture {
    dir: PathBuf,
    root: PathBuf,
    templates: PathBuf,
    document: Uuid,
    first: Uuid,
    last: Uuid,
    output: Uuid,
}

fn id(n: u128) -> Uuid {
    Uuid::parse(&uuid::Uuid::from_u128(n).to_string()).unwrap()
}

impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let dir = std::env::temp_dir().join(format!("saved-notebook-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let f = Self {
            root: dir.join("store"),
            templates: dir.join("templates"),
            dir,
            document: id(1),
            first: id(2),
            last: id(3),
            output: id(4),
        };
        for path in [&f.root, &f.root.join(f.document.to_string()), &f.templates] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        private_write(f.templates.join("templates.json"), STOCK_REGISTRY).unwrap();
        private_write(f.templates.join("Blank.svg"), BLANK_SVG).unwrap();
        private_write(f.templates.join("P Lines medium.svg"), PORTRAIT_SVG).unwrap();
        private_write(f.templates.join("LS Lines medium.png"), LANDSCAPE_PNG).unwrap();
        let scene = include_bytes!("../../../../../tests/fixtures/native-history/applied.rm");
        private_write(f.scene(f.first), scene).unwrap();
        private_write(f.scene(f.last), scene).unwrap();
        f.table(false);
        f
    }

    fn scene(&self, page: Uuid) -> PathBuf {
        self.root.join(format!("{}/{page}.rm", self.document))
    }

    fn table_path(&self) -> PathBuf {
        self.root.join(format!("{}.content", self.document))
    }

    fn page_template(&self, page: Uuid, template: Option<&str>) -> Result<()> {
        let path = self.table_path();
        let mut table: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let pages = table["cPages"]["pages"].as_array_mut().unwrap();
        let row = pages
            .iter_mut()
            .find(|row| row["id"] == page.to_string())
            .unwrap();
        let fields = row.as_object_mut().unwrap();
        match template {
            Some(template) => {
                fields.insert("template".into(), json!({"value":template}));
            }
            None => {
                fields.remove("template");
            }
        }
        private_write(path, serde_json::to_vec(&table)?)?;
        Ok(())
    }

    fn table(&self, insert: bool) {
        let row = |page: Uuid, key: &str| {
            json!({
                "id": page.to_string(), "idx": {"value": key}, "template": {"value":"Blank"},
            })
        };
        let mut pages = vec![row(self.last, "c"), row(self.first, "a")];
        if insert {
            pages.push(row(self.output, "b"));
        }
        private_write(
            self.table_path(),
            json!({
                "fileType":"notebook", "formatVersion":2, "pageCount": pages.len(),
                "orientation":"portrait",
                "cPages":{"pages":pages,"lastOpened":{"value":self.first.to_string()}},
            })
            .to_string(),
        )
        .unwrap();
    }

    fn get(&self, order: Vec<Uuid>, excluded: HashSet<Uuid>) -> Result<SavedNotebookSource> {
        self.get_with_pages(order, excluded, None)
    }

    fn get_with_pages(
        &self,
        order: Vec<Uuid>,
        excluded: HashSet<Uuid>,
        consume: Option<&mut SavedNotebookPageConsumer<'_>>,
    ) -> Result<SavedNotebookSource> {
        let facts = ReaderFacts::new(
            PageKey {
                document: self.document,
                page: self.first,
            },
            order,
        )
        .unwrap();
        acquire(
            SourcePaths {
                root: &self.root,
                templates: &self.templates,
                owner: (unsafe { libc::geteuid() }, None),
                fixture: true,
            },
            &facts,
            &excluded,
            &mut SourceProgress::new(Instant::now() + Duration::from_secs(5), &mut || Ok(())),
            consume,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn saved_notebook_stays_current_only_for_the_same_ordered_source_inputs() -> Result<()> {
    let f = Fixture::new();
    let order = vec![f.first, f.last];
    let initial = f.get(order.clone(), HashSet::new())?;
    assert_eq!(initial.original_pages(), order);
    let mut observation = SourceObservation {
        document: uuid::Uuid::from_bytes(*f.document.as_bytes()),
        page: uuid::Uuid::from_bytes(*f.first.as_bytes()),
        session: "fixture".into(),
        visit: "fixture-visit".into(),
        content_revision: None,
        order_revision: Some("independent order witness".into()),
        capability_revision: "fixture-profile".into(),
        evidence_procedure: "fixture".into(),
    };
    initial.bind_content_revision(&mut observation)?;
    assert_eq!(
        observation.content_revision.as_deref(),
        Some(initial.revision())
    );
    assert_eq!(
        observation.order_revision.as_deref(),
        Some("independent order witness")
    );
    observation.page = uuid::Uuid::from_bytes(*f.output.as_bytes());
    assert!(initial.bind_content_revision(&mut observation).is_err());
    assert!(f.get(vec![f.last, f.first], HashSet::new()).is_err());

    f.table(true);
    let inserted = vec![f.first, f.output, f.last];
    assert!(f.get(inserted.clone(), HashSet::new()).is_err());
    let excluded = HashSet::from([f.output]);
    assert_eq!(initial, f.get(inserted.clone(), excluded.clone())?);
    fs::write(f.scene(f.output), b"confirmed Buddy output").unwrap();
    assert_eq!(initial, f.get(inserted.clone(), excluded.clone())?);

    let pagedata = f.root.join(format!("{}.pagedata", f.document));
    private_write(&pagedata, "Blank\nBlank\nBlank\n")?;
    assert_eq!(initial, f.get(inserted.clone(), excluded.clone())?);
    private_write(&pagedata, "Blank\nP Lines medium\nBlank\n")?;
    f.page_template(f.output, Some("P Lines medium"))?;
    private_write(
        f.templates.join("P Lines medium.svg"),
        "<svg><path id='changed'/></svg>",
    )?;
    assert_eq!(initial, f.get(inserted.clone(), excluded.clone())?);
    f.page_template(f.last, Some("P Lines medium"))?;
    assert_ne!(initial, f.get(inserted.clone(), excluded.clone())?);
    f.page_template(f.last, None)?;
    let error = f.get(inserted.clone(), excluded.clone()).unwrap_err();
    assert!(error
        .to_string()
        .contains("Missing explicit original template assignment"));
    f.page_template(f.last, Some("Blank"))?;
    assert_eq!(initial, f.get(inserted.clone(), excluded.clone())?);

    let scene = fs::read(f.scene(f.last))?;
    let mut changed = scene.clone();
    *changed.last_mut().unwrap() ^= 1;
    fs::write(f.scene(f.last), changed)?;
    assert_ne!(initial, f.get(inserted.clone(), excluded.clone())?);
    fs::write(f.scene(f.last), scene)?;

    fs::write(f.templates.join("Blank.svg"), "<svg>changed</svg>")?;
    assert_ne!(initial, f.get(inserted.clone(), excluded.clone())?);
    fs::remove_file(f.scene(f.last))?;
    let e = f.get(inserted, excluded).unwrap_err();
    assert!(e.to_string().contains("blank has not been proven"));
    Ok(())
}

#[test]
fn saved_notebook_resolves_stock_filenames_and_lends_exact_pinned_bytes() -> Result<()> {
    let f = Fixture::new();
    let order = vec![f.first, f.last];
    let blank = f.get(order.clone(), HashSet::new())?;
    // Orientation and other presentation fields on duplicate registry rows
    // must not affect the revision when the assigned files stay the same.
    let mut registry: Value = serde_json::from_str(STOCK_REGISTRY)?;
    let entries = registry["templates"].as_array_mut().unwrap();
    entries[1]["name"] = json!("Alternate presentation");
    entries.reverse();
    private_write(
        f.templates.join("templates.json"),
        serde_json::to_vec(&registry)?,
    )?;
    assert_eq!(blank, f.get(order.clone(), HashSet::new())?);

    f.table(true);
    f.page_template(f.first, Some("P Lines medium"))?;
    f.page_template(f.last, Some("LS Lines medium"))?;
    let order = vec![f.first, f.output, f.last];
    let exclude = HashSet::from([f.output]);
    let scenes = [fs::read(f.scene(f.first))?, fs::read(f.scene(f.last))?];
    let mut seen = Vec::new();
    let rendered = f.get_with_pages(
        order.clone(),
        exclude.clone(),
        Some(&mut |page: SavedNotebookPage<'_>| {
            let first = seen.is_empty();
            assert_eq!(page.page, if first { f.first } else { f.last });
            assert_eq!(page.scene, scenes[usize::from(!first)].as_slice());
            if first {
                assert_eq!(page.template_stem, "P Lines medium");
                assert_eq!(page.template_svg, Some(PORTRAIT_SVG.as_bytes()));
                assert_eq!(page.template_png, None);
            } else {
                assert_eq!(page.template_stem, "LS Lines medium");
                assert_eq!(page.template_svg, None);
                assert_eq!(page.template_png, Some(LANDSCAPE_PNG));
            }
            seen.push(page.page);
            Ok(())
        }),
    )?;
    assert_eq!(seen, [f.first, f.last]);
    assert_eq!(rendered, f.get(order.clone(), exclude.clone())?);
    private_write(
        f.templates.join("P Lines medium.svg"),
        PORTRAIT_SVG.replace("portrait", "Portrait"),
    )?;
    assert_ne!(rendered, f.get(order.clone(), exclude.clone())?);
    for (stem, message) in [
        ("Lined medium", "Unregistered stock template asset stem"),
        ("../Blank", "Invalid stock template asset stem"),
    ] {
        f.page_template(f.first, Some(stem))?;
        let error = f.get(order.clone(), exclude.clone()).unwrap_err();
        assert!(error.to_string().contains(message));
    }
    Ok(())
}

#[test]
fn saved_notebook_rejects_a_scene_changed_while_its_bytes_are_borrowed() -> Result<()> {
    let f = Fixture::new();
    let error = f
        .get_with_pages(
            vec![f.first, f.last],
            HashSet::new(),
            Some(&mut |page: SavedNotebookPage<'_>| {
                if page.page == f.first {
                    // Edit the input after the consumer gets the original bytes.
                    let mut changed = page.scene.to_vec();
                    changed.push(0);
                    private_write(f.scene(f.first), changed)?;
                }
                Ok(())
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("Original document file changed"));
    Ok(())
}

#[test]
fn saved_notebook_refuses_unresolved_backing_and_indirection() -> Result<()> {
    let f = Fixture::new();
    let order = vec![f.first, f.last];
    f.get(order.clone(), HashSet::new())?;
    let pdf = f.root.join(format!("{}.pdf", f.document));
    fs::write(&pdf, b"unexpected backing")?;
    assert!(f.get(order.clone(), HashSet::new()).is_err());
    fs::remove_file(pdf)?;
    fs::remove_file(f.scene(f.last))?;
    std::os::unix::fs::symlink(f.scene(f.first), f.scene(f.last))?;
    assert!(f.get(order, HashSet::new()).is_err());
    Ok(())
}
