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
        private_write(
            f.templates.join("templates.json"),
            br#"{"templates":[{"filename":"blank","name":"Blank"}]}"#,
        )
        .unwrap();
        private_write(f.templates.join("blank.svg"), "<svg/>").unwrap();
        let scene = include_bytes!("../../../../../tests/fixtures/native-history/applied.rm");
        private_write(f.scene(f.first), scene).unwrap();
        private_write(f.scene(f.last), scene).unwrap();
        f.table(false);
        f
    }

    fn scene(&self, page: Uuid) -> PathBuf {
        self.root.join(format!("{}/{page}.rm", self.document))
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
            self.root.join(format!("{}.content", self.document)),
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
        let facts = ReaderFacts::new(
            PageKey {
                document: self.document,
                page: self.first,
            },
            order,
        )
        .unwrap();
        acquire(
            &self.root,
            &self.templates,
            (unsafe { libc::geteuid() }, None),
            true,
            &facts,
            &excluded,
            &mut SourceProgress::new(Instant::now() + Duration::from_secs(5), &mut || Ok(())),
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

    let scene = fs::read(f.scene(f.last))?;
    let mut changed = scene.clone();
    *changed.last_mut().unwrap() ^= 1;
    fs::write(f.scene(f.last), changed)?;
    assert_ne!(initial, f.get(inserted.clone(), excluded.clone())?);
    fs::write(f.scene(f.last), scene)?;

    fs::write(f.templates.join("blank.svg"), "<svg>changed</svg>")?;
    assert_ne!(initial, f.get(inserted.clone(), excluded.clone())?);
    fs::remove_file(f.scene(f.last))?;
    let e = f.get(inserted, excluded).unwrap_err();
    assert!(e.to_string().contains("blank has not been proven"));
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
