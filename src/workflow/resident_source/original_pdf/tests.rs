use super::*;
use crate::storage::digest;
use remarkable_open_sdk::{native_adapter::ReaderFacts, PageKey, Uuid as SdkUuid};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    facts: ReaderFacts,
    document: Uuid,
}

impl Fixture {
    fn new() -> Self {
        Self::for_document(Uuid::from_u128(1))
    }

    fn for_document(document: Uuid) -> Self {
        // Use a private child of the fixture temp root and own only this child.
        let root = std::env::temp_dir().join(format!("reader-original-pdf-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let id = |number| SdkUuid::parse(&Uuid::from_u128(number).to_string()).unwrap();
        let facts = ReaderFacts::new(
            PageKey {
                document: SdkUuid::parse(&document.to_string()).unwrap(),
                page: id(3),
            },
            vec![id(2), id(3)],
        )
        .unwrap();
        let fixture = Self {
            root,
            facts,
            document,
        };
        fixture.table(0, 1);
        fs::write(fixture.pdf_path(), b"%PDF-1.5\noriginal source bytes\n").unwrap();
        fixture
    }

    fn pdf_path(&self) -> PathBuf {
        self.root.join(format!("{}.pdf", self.document))
    }

    fn table_path(&self) -> PathBuf {
        self.root.join(format!("{}.content", self.document))
    }

    fn table(&self, first: u32, second: u32) {
        let table = json!({
            "fileType": "pdf",
            "cPages": {
                "original": { "value": 2 },
                "pages": [
                    { "id": Uuid::from_u128(3), "idx": { "value": "b" }, "redir": { "value": second } },
                    { "id": Uuid::from_u128(2), "idx": { "value": "a" }, "redir": { "value": first } },
                ],
            }
        });
        fs::write(self.table_path(), serde_json::to_vec(&table).unwrap()).unwrap();
    }

    fn observe(&self) -> Result<ObservedOriginalPdf> {
        let uid = fs::metadata(&self.root)?.uid();
        ObservedOriginalPdf::observe_at(
            &self.root,
            (uid, None),
            true,
            &self.facts,
            self.document,
            Instant::now() + Duration::from_secs(5),
            &mut || Ok(()),
        )
    }
}

#[cfg(feature = "installed-resident-output")]
pub(crate) fn assert_ordinary_pdf_reuse(
    executable: &Path,
    initial: &[u8],
    revised: &[u8],
    oversized: &[u8],
) -> Result<()> {
    let observe = |fixture: &Fixture| -> Result<ObservedOriginalPdf> {
        OrdinaryPdfSource::pin_fixture(&fixture.root, &fixture.facts)?
            .read(
                &fixture.facts,
                Instant::now() + Duration::from_secs(25),
                &mut || Ok(()),
            )?
            .context("Fixture page did not admit an original PDF")
    };
    let query = |fixture: &Fixture, cached, question: &str, page: u32| {
        let deadline = Instant::now() + Duration::from_secs(30);
        let (capture, cached) = observe(fixture)?.complete_ordinary(
            &fixture.facts,
            executable,
            deadline,
            cached,
            &mut || Ok(()),
        )?;
        let key = capture.index.key().clone();
        let request = capture.index.fixture_request();
        let found =
            capture
                .index
                .query(&key, question, 1, 4096, false, deadline, &mut || Ok(()))?;
        assert_eq!(
            found.passages.first().and_then(|passage| passage.page),
            Some(page)
        );
        Ok::<_, anyhow::Error>((request, key, cached))
    };

    let fixture = Fixture::new();
    fs::write(fixture.pdf_path(), initial)?;
    let ((first, reused), key, cache) = query(&fixture, None, "alpha acceleration", 2)?;
    assert!(!reused && cache.is_some());
    let ((second, reused), again, cache) = query(&fixture, cache, "unrelated first page", 1)?;
    assert!(reused);
    assert_ne!(first, second);
    assert_eq!(key, again);

    fs::write(fixture.pdf_path(), revised)?;
    let ((_, reused), changed, cache) = query(&fixture, cache, "beta velocity", 2)?;
    assert!(!reused);
    assert_ne!(key.source_sha256, changed.source_sha256);

    // Replacing the inode with identical bytes still requires cold admission.
    let replacement = fixture.root.join("replacement.pdf");
    fs::write(&replacement, revised)?;
    fs::rename(replacement, fixture.pdf_path())?;
    let ((_, reused), same, cache) = query(&fixture, cache, "beta velocity", 2)?;
    assert!(!reused);
    assert_eq!(same, changed);

    let other = Fixture::for_document(Uuid::new_v4());
    fs::write(other.pdf_path(), revised)?;
    let ((_, reused), other_key, cache) = query(&other, cache, "beta velocity", 2)?;
    assert!(!reused);
    assert_ne!(other_key.document, changed.document);
    let ((_, reused), _, cache) = query(&fixture, cache, "beta velocity", 2)?;
    assert!(!reused);

    // Fail the full source recheck before a cached worker can receive input.
    let source = observe(&fixture)?;
    fixture.table(1, 0);
    assert!(source
        .complete_ordinary(
            &fixture.facts,
            executable,
            Instant::now() + Duration::from_secs(30),
            cache,
            &mut || Ok(()),
        )
        .is_err());
    fixture.table(0, 1);
    let ((_, reused), _, cache) = query(&fixture, None, "beta velocity", 2)?;
    assert!(!reused && cache.is_some());

    // Valid normalized bytes above the retention bound go through the cold index.
    fs::write(other.pdf_path(), oversized)?;
    let mut table: serde_json::Value = serde_json::from_slice(&fs::read(other.table_path())?)?;
    table["cPages"]["original"]["value"] = json!(7);
    fs::write(other.table_path(), serde_json::to_vec(&table)?)?;
    let ((large, reused), large_key, cache) = query(&other, cache, "velocity marker", 7)?;
    assert!(!reused && cache.is_none());
    let ((second_large, reused), key, cache) = query(&other, cache, "marker", 7)?;
    assert!(!reused && cache.is_none());
    assert_ne!(large, second_large);
    assert_eq!(key, large_key);
    Ok(())
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn original_pdf_uses_exact_order_and_nonzero_redirect_and_brackets_bytes() {
    let fixture = Fixture::new();
    let observed = fixture.observe().unwrap();
    assert_eq!(observed.pin.original_page(), 2);
    let mut admitted = observed
        .recheck_capture(&fixture.facts, &fixture.facts, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        admitted.pdf_digest,
        digest(&fs::read(fixture.pdf_path()).unwrap())
    );
    admitted
        .pin
        .check(Instant::now() + Duration::from_secs(5))
        .unwrap();
    fs::write(fixture.pdf_path(), b"%PDF-1.5\nchanged source bytes!\n").unwrap();
    assert!(admitted
        .pin
        .check(Instant::now() + Duration::from_secs(5))
        .is_err());
}

#[test]
fn original_pdf_refuses_foreign_capture_duplicate_redirect_and_link() {
    let fixture = Fixture::new();
    let observed = fixture.observe().unwrap();
    let other = ReaderFacts::new(
        PageKey {
            document: fixture.facts.page().document,
            page: fixture.facts.order()[0],
        },
        fixture.facts.order().to_vec(),
    )
    .unwrap();
    assert!(observed
        .recheck_capture(&fixture.facts, &other, &mut || Ok(()))
        .is_err());
    fixture.table(1, 1);
    assert!(fixture.observe().is_err());
    fixture.table(0, 1);

    let replacement = fixture.root.join("replacement.pdf");
    fs::rename(fixture.pdf_path(), &replacement).unwrap();
    symlink(&replacement, fixture.pdf_path()).unwrap();
    assert!(fixture.observe().is_err());
}

#[test]
fn original_pdf_refuses_same_bytes_at_replaced_inode_and_bounded_cancellation() {
    let fixture = Fixture::new();
    let observed = fixture.observe().unwrap();
    let replacement = fixture.root.join("replacement.pdf");
    fs::copy(fixture.pdf_path(), &replacement).unwrap();
    fs::rename(&replacement, fixture.pdf_path()).unwrap();
    assert!(observed
        .recheck_capture(&fixture.facts, &fixture.facts, &mut || Ok(()))
        .is_err());

    let observed = fixture.observe().unwrap();
    let mut callbacks = 0;
    assert!(observed
        .recheck_capture(&fixture.facts, &fixture.facts, &mut || {
            callbacks += 1;
            bail!("scripted cancellation")
        })
        .is_err());
    assert_eq!(callbacks, 1);
}

#[test]
fn original_pdf_refuses_changed_page_table_source_size_and_expired_pin() {
    let fixture = Fixture::new();
    let observed = fixture.observe().unwrap();
    fixture.table(1, 0);
    assert!(observed
        .recheck_capture(&fixture.facts, &fixture.facts, &mut || Ok(()))
        .is_err());

    fs::OpenOptions::new()
        .write(true)
        .open(fixture.pdf_path())
        .unwrap()
        .set_len((PDF_BYTES + 1) as u64)
        .unwrap();
    assert!(fixture.observe().is_err());

    fs::write(fixture.pdf_path(), b"%PDF-1.5\noriginal source bytes\n").unwrap();
    let mut observed = fixture.observe().unwrap();
    observed.pin.samples_left = 1;
    let deadline = Instant::now() + Duration::from_secs(5);
    observed.pin.check(deadline).unwrap();
    assert!(observed.pin.check(deadline).is_err());
}

#[test]
fn original_pdf_large_reads_use_cadence_and_still_propagate_cancellation() {
    let fixture = Fixture::new();
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.pdf_path())
        .unwrap()
        .set_len(PDF_BYTES as u64)
        .unwrap();
    let mut callbacks = 0;
    let mut progress = || {
        callbacks += 1;
        ensure!(callbacks <= 34, "source consumed its progress allowance");
        Ok(())
    };
    let observed = ObservedOriginalPdf::observe_at(
        &fixture.root,
        (fs::metadata(&fixture.root).unwrap().uid(), None),
        true,
        &fixture.facts,
        fixture.document,
        Instant::now() + Duration::from_secs(5),
        &mut progress,
    )
    .unwrap();
    let admitted = observed
        .recheck_capture(&fixture.facts, &fixture.facts, &mut progress)
        .unwrap();
    assert!(callbacks >= 4);
    assert_eq!(
        admitted.source_bytes_left + 2 * fs::metadata(fixture.table_path()).unwrap().len() as usize,
        2 * TABLE_BYTES
    );

    let observed = fixture.observe().unwrap();
    let mut cancel = || bail!("scripted cadence cancellation");
    let mut progress = SourceProgress::new(Instant::now() + Duration::from_secs(5), &mut cancel);
    // The first callback is due. The read must refuse before consuming a chunk.
    let mut reads = SOURCE_READS;
    let mut bytes = SOURCE_BYTES;
    let error = observed
        .pin
        .pdf
        .read(None, &mut reads, &mut bytes, &mut progress)
        .unwrap_err();
    assert!(error.to_string().contains("scripted cadence cancellation"));
    assert_eq!(reads, SOURCE_READS);
}
