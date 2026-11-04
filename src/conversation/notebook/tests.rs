use super::*;
use crate::retrieval::{NormalizedDocument, SourceUnit};
use crate::storage::StorePaths;
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("notebook-source-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn input() -> ValidatedNotebook {
    let doc = NormalizedDocument {
        document: Uuid::from_u128(2),
        format: SourceFormat::Notebook,
        source_sha256: "a".repeat(64),
        extractor_revision: "archive-fixture-v1".into(),
        units: vec![
            SourceUnit {
                page: Some(1),
                section: None,
                text: "selected text é\n".into(),
            },
            SourceUnit {
                page: Some(2),
                section: None,
                text: "unselected retained data".into(),
            },
        ],
    };
    let mut bytes = serde_json::to_vec(&doc).unwrap();
    bytes.push(b'\n');
    ValidatedNotebook::from_bytes(&bytes, &digest(&bytes)).unwrap()
}
fn archive_envelope(ledger: &Ledger, archive: &ArchivedNormalizedSource) -> Envelope {
    ledger
        .envelope(
            Namespace::Source,
            archive.id,
            ExpectedHeads::default(),
            Record::ArchivedNormalizedSource(Box::new(archive.clone())),
            vec![archive.media.clone()],
        )
        .unwrap()
}

#[test]
fn archive_reopen_context_and_retained_media_keep_original_raw_provenance() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::prepare_notebook(fixture.paths()).unwrap());
    let ledger = Ledger::new(store.clone());
    let conversation = Uuid::new_v4();
    let turn_id = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let source = input();
    let archive = ArchivedNormalizedSource::from_notebook(
        conversation,
        turn_id,
        &source,
        TextScope::Page { page: 1 },
        Some("outline".into()),
    )
    .unwrap();
    let (prior, mut root) = ledger.live_root(conversation, None).unwrap();
    root.next_sequence = 1;
    root.updated_ms = 2;
    let root = ledger
        .envelope(
            Namespace::Conversation,
            conversation,
            ExpectedHeads(BTreeSet::from([prior.revision_id])),
            Record::Root(root),
            vec![],
        )
        .unwrap();
    let turn = ledger
        .envelope(
            Namespace::Conversation,
            turn_id,
            ExpectedHeads::default(),
            Record::Turn(Turn {
                id: turn_id,
                conversation,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::User,
                mode: Mode::Writer,
                outcome: Outcome::Interpreted,
                text: Some("generate notes".into()),
                sources: vec![archive.id],
                correction_of: None,
                created_ms: 2,
                updated_ms: 2,
                completion: None,
            }),
            vec![],
        )
        .unwrap();
    store
        .commit(
            vec![root, turn, archive_envelope(&ledger, &archive)],
            BTreeMap::from([(
                archive.media.sha256.clone(),
                source.canonical_bytes().to_vec(),
            )]),
        )
        .unwrap();
    drop(ledger);
    drop(store);
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let ledger = Ledger::new(store.clone());
    let loaded = archive.load(&store).unwrap();
    assert_eq!(loaded.raw_sha256(), source.raw_sha256());
    assert_eq!(loaded.raw_bytes(), source.raw_bytes());
    assert_ne!(loaded.raw_bytes(), loaded.canonical_bytes().len());
    assert_eq!(loaded.document().units.len(), 2);
    assert_eq!(
        loaded
            .coverage(&archive.selection, 2)
            .unwrap()
            .selected_units(),
        1
    );
    let context = ledger
        .context(
            conversation,
            &ContextBudget {
                max_text_bytes: 4096,
                max_turns: 10,
                provider_token_limit: Some(4096),
            },
            |_| Ok(10),
        )
        .unwrap();
    assert_eq!(context.archived_sources, vec![archive.clone()]);
    assert!(
        context.sources.is_empty()
            && context.captures.is_empty()
            && context.missing_media.is_empty()
    );
    let media = ledger.retained_media(conversation).unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].media, archive.media);
    assert!(media[0].available);
    assert_eq!(media[0].retained_revision_references, 1);
    let backup = fixture.0.join("notebook-backup");
    let report = store.export_notebook_backup(&backup).unwrap();
    assert_eq!(report.format, 4);
    assert!(report.complete_media && report.manifests > 0 && report.objects > 0);
    assert_eq!(Store::inspect_notebook_backup(&backup).unwrap(), report);
    assert!(
        Store::inspect_backup(&backup).is_err(),
        "legacy backup reader must refuse"
    );
    let before = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    store.restore_notebook_backup(&backup).unwrap();
    assert_ne!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        before
    );
    assert_eq!(
        archive.load(&store).unwrap().raw_sha256(),
        source.raw_sha256()
    );
    assert_eq!(
        ledger.retained_media(conversation).unwrap()[0].retained_revision_references,
        1
    );
    let foreign = Fixture::new();
    let foreign_store = Store::prepare_notebook(foreign.paths()).unwrap();
    let foreign_current = fs::read(foreign.paths().data.join("CURRENT")).unwrap();
    assert!(foreign_store.restore_notebook_backup(&backup).is_err());
    assert_eq!(
        fs::read(foreign.paths().data.join("CURRENT")).unwrap(),
        foreign_current
    );
}

#[test]
fn legacy_generation_refuses_schema3_before_objects_or_manifest_publication() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(fixture.paths()).unwrap());
    let ledger = Ledger::new(store.clone());
    let archive = ArchivedNormalizedSource::from_notebook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        &input(),
        TextScope::SuppliedNotebook,
        None,
    )
    .unwrap();
    let envelope = archive_envelope(&ledger, &archive);
    let before = store
        .snapshot_revisions_matching(&[Namespace::Source], 100, |_| true)
        .unwrap();
    assert!(store
        .commit(
            vec![envelope],
            BTreeMap::from([(
                archive.media.sha256.clone(),
                input().canonical_bytes().to_vec()
            )])
        )
        .is_err());
    assert_eq!(
        store
            .snapshot_revisions_matching(&[Namespace::Source], 100, |_| true)
            .unwrap()
            .len(),
        before.len()
    );
    assert!(store
        .heads(Namespace::Source, archive.id)
        .unwrap()
        .is_empty());
}

#[test]
fn archive_wrong_schema_kind_namespace_and_descriptors_refuse() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::prepare_notebook(fixture.paths()).unwrap());
    let ledger = Ledger::new(store);
    let archive = ArchivedNormalizedSource::from_notebook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        &input(),
        TextScope::SuppliedNotebook,
        None,
    )
    .unwrap();
    let envelope = archive_envelope(&ledger, &archive);
    assert!(Ledger::decode(&envelope).is_ok());
    let mut wrong = envelope.clone();
    wrong.domain_schema_version = 2;
    assert!(Ledger::decode(&wrong).is_err());
    let mut wrong = envelope.clone();
    wrong.namespace = Namespace::Conversation;
    assert!(Ledger::decode(&wrong).is_err());
    let mut wrong = envelope.clone();
    wrong.kind = Kind::Tombstone;
    assert!(Ledger::decode(&wrong).is_err());
    let mut wrong = envelope.clone();
    wrong.media_descriptors.clear();
    assert!(Ledger::decode(&wrong).is_err());
    let mut wrong = archive;
    wrong.guidance = Some("changed".into());
    assert!(wrong.validate().is_err());
}

#[test]
fn missing_or_semantically_wrong_json_never_becomes_a_committed_archive() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::prepare_notebook(fixture.paths()).unwrap());
    let ledger = Ledger::new(store.clone());
    let archive = ArchivedNormalizedSource::from_notebook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        &input(),
        TextScope::SuppliedNotebook,
        None,
    )
    .unwrap();
    assert!(store
        .commit(vec![archive_envelope(&ledger, &archive)], BTreeMap::new())
        .is_err());
    assert!(archive.validate_bytes(b"{}").is_err());
    assert!(archive.load(&store).is_err());
    assert!(store
        .heads(Namespace::Source, archive.id)
        .unwrap()
        .is_empty());
}
