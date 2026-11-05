use super::*;
use crate::{
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::{
        sync::mode::SyncModeChoice, Manifest, Namespace, ObjectRef, Scope, StorePaths, FORMAT,
    },
    writer, LLMEngine,
};
use std::{collections::BTreeMap, io};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let p = self.0.canonicalize().unwrap();
        assert_eq!(
            p.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("writer-caller-"));
        std::fs::remove_dir_all(p).unwrap();
    }
}
struct Model;
impl LLMEngine for Model {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, _: &str) {
        panic!("text only")
    }
    fn clear_content(&mut self) {}
    fn execute(&mut self) -> Result<String> {
        Ok("# Draft\n\nx = 7".into())
    }
}
fn published() -> (Fixture, Arc<Store>, Report, Uuid, Uuid) {
    let f = Fixture(std::env::temp_dir().join(format!("writer-caller-{}", Uuid::new_v4())));
    let store = Arc::new(
        Store::open(StorePaths {
            data: f.0.join("data"),
            cache: f.0.join("cache"),
            credentials: f.0.join("credentials"),
        })
        .unwrap(),
    );
    let ledger = Ledger::new(store.clone());
    let conversation = Uuid::new_v4();
    let document = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
    let scope = SelectionScope {
        group: Uuid::new_v4(),
        key_sha256: crate::memory::catalog::document_key(document).unwrap(),
        binding_sha256: digest(b"caller fixture"),
    };
    let root = store
        .heads(Namespace::Conversation, conversation)
        .unwrap()
        .pop()
        .unwrap();
    let raw = serde_json::to_vec(&root).unwrap();
    let hash = digest(&raw);
    let selected = Manifest {
        format: FORMAT,
        transaction_id: Uuid::new_v4(),
        scope: Scope::SelectedRecords,
        records: vec![ObjectRef {
            sha256: hash.clone(),
            bytes: raw.len() as u64,
        }],
        record_namespaces: BTreeMap::from([(hash.clone(), Namespace::Conversation)]),
        media: vec![],
        media_coverage: vec![],
    };
    let initialized = store
        .initialize_selected(
            &scope,
            storage::selection::SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: digest(b"base"),
                selected,
                retained: vec![],
            },
            BTreeMap::from([(hash, raw)]),
        )
        .unwrap();
    let before = store.sync_mode_snapshot().unwrap();
    store
        .set_sync_mode(
            &before.token,
            Uuid::new_v4(),
            SyncModeChoice::SharedGroup {
                group: scope.group,
                binding_sha256: scope.binding_sha256.clone(),
                root_id: "fixture-root".into(),
            },
        )
        .unwrap();
    let writer = SelectedAdmission::new(store.clone(), scope.clone()).unwrap();
    let p = writer
        .with_current(&initialized.token, |s| {
            SelectedDomainProjection::from_snapshot(s)
        })
        .unwrap();
    let selection = selected::Selection {
        accepted: initialized.token,
        root: p
            .current_reference(Namespace::Conversation, conversation)
            .unwrap(),
        refine: None,
    };
    let operation = Uuid::new_v4();
    let req = writer::Request::Generate {
        conversation,
        operation,
        request: "Keep x exact".into(),
        document: NormalizedDocument {
            document,
            format: SourceFormat::Notebook,
            source_sha256: digest(b"source"),
            extractor_revision: "fixture".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "x = 7".into(),
            }],
        },
        selection: TextScope::SuppliedNotebook,
        guidance: None,
    };
    let result = selected::run(
        &writer,
        &selection,
        &mut Model,
        &req,
        &crate::config::Personalization::default()
            .snapshot()
            .unwrap(),
        None,
        &mut || false,
    );
    let SelectedWriterPublication::Published {
        publication,
        acknowledgment,
    } = result.publication.unwrap()
    else {
        panic!("fresh")
    };
    let mut report = Report::new(Operation::Generate, "generated");
    report.data = Some(
        serde_json::json!({"draft":acknowledgment.written,"receipt":acknowledgment.receipt,"disposition":"published","handles":null}),
    );
    report.mode = Some(shared(&store, &scope).unwrap());
    attach_current(&mut report, writer, publication.token, conversation).unwrap();
    (f, store, report, conversation, operation)
}
#[test]
fn committed_writer_report_replacement_preserves_original_draft_as_historical() {
    let (_f, store, mut report, conversation, _) = published();
    let guard = report.guard.as_ref().unwrap();
    let snapshot = store
        .selected_snapshot(guard.token.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let p = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
    let mut root = p
        .head(Namespace::Conversation, conversation)
        .unwrap()
        .0
        .clone();
    root.parents = [root.revision_id].into();
    root.revision_id = Uuid::new_v4();
    root.operation_id = Uuid::new_v4();
    store
        .commit_selected(&snapshot.token, Uuid::new_v4(), vec![root], BTreeMap::new())
        .unwrap();
    let mut output = Vec::new();
    report.write_to(&mut output).unwrap();
    let frame: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(frame["status"], "historical");
    assert_eq!(frame["data"]["draft"]["text"], "# Draft\n\nx = 7");
    assert!(frame["data"]["handles"].is_null());
    assert_eq!(frame["native_authority"], false);
}
struct Broken {
    bytes: Vec<u8>,
}
impl Write for Broken {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if !self.bytes.is_empty() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"));
        }
        let size = b.len().min(7);
        self.bytes.extend_from_slice(&b[..size]);
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn selected_writer_output_failure_does_not_emit_replacement_or_replay_commit() {
    let (_f, store, mut report, _, operation) = published();
    let before = serde_json::to_vec(&store.manifests().unwrap()).unwrap();
    let mut output = Broken { bytes: vec![] };
    assert!(report.write_to(&mut output).is_err());
    assert_eq!(output.bytes.len(), 7);
    assert!(report.write_to(&mut Vec::new()).is_err());
    assert_eq!(
        serde_json::to_vec(&store.manifests().unwrap()).unwrap(),
        before
    );
    assert!(store
        .selected_receipt(
            report.guard.as_ref().unwrap().token.scope(),
            writer::stage(operation, "finish")
        )
        .unwrap()
        .is_some());
}
#[test]
fn selected_writer_tampered_current_report_refuses_before_any_output() {
    let (_f, _store, mut report, _, _) = published();
    report.data.as_mut().unwrap()["draft"]["text"] = serde_json::json!("tampered");
    let mut out = Vec::new();
    assert!(report.write_to(&mut out).is_err());
    assert!(out.is_empty());
}
