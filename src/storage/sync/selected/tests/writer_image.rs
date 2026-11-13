use super::super::corrections as route;
use super::corrections::{confirm, observed, png, CorrectionFixture};
use super::*;
use crate::{
    conversation::{
        fixture_defer_pending_for_worker, resident_capture::tests::capture_for,
        selected_reader_prepare::ResidentPrepared, SelectedWriterImage,
    },
    llm::ModelHandoff,
    writer::{
        self,
        selected::{run_image, Selection, WriterImageRequest},
    },
    LLMEngine,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::json;
use std::path::{Path, PathBuf};

struct Model<'a> {
    writer: &'a SelectedAdmission,
    store: &'a Store,
    input: &'a WriterImageRequest,
    before: Option<(usize, &'a dyn Fn())>,
    after: Option<(usize, &'a dyn Fn())>,
    calls: Vec<Vec<String>>,
    current: Vec<String>,
}

impl LLMEngine for Model<'_> {
    fn guarded_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, value: &str) {
        self.current.push(value.to_owned());
    }
    fn clear_content(&mut self) {
        self.current.clear();
    }
    fn execute(&mut self) -> Result<String> {
        panic!("unguarded selected Writer")
    }
    fn execute_with_handoff(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut ModelHandoff<'_>,
    ) -> Result<String> {
        progress()?;
        let ordinal = self.calls.len();
        assert!(self
            .store
            .selected_receipt(self.writer.scope(), self.input.preparation())?
            .is_some());
        if let Some((_, change)) = self.before.filter(|(stage, _)| *stage == ordinal) {
            change();
        }
        handoff(&mut || {
            self.calls.push(self.current.clone());
            Ok(())
        })?;
        if let Some((_, change)) = self.after.filter(|(stage, _)| *stage == ordinal) {
            change();
        }
        if ordinal == 0 {
            Ok(r#"{"pages":[{"page":1,"status":"transcribed","text":"x = 7"}]}"#.into())
        } else {
            assert!(self
                .store
                .selected_receipt(self.writer.scope(), self.input.operation)?
                .is_some());
            Ok("# Draft\n\nx = 7".into())
        }
    }
}

struct WriterFixture {
    correction: CorrectionFixture,
    writer: SelectedAdmission,
    scope: SelectionScope,
    path: PathBuf,
    root: Uuid,
    image: SelectedWriterImage,
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Source,
    Preference,
}

impl WriterFixture {
    fn new(handwriting: bool) -> Self {
        let f = CorrectionFixture::new();
        let identity = catalog::Identity {
            group: f.binding.group,
            profile: f.owner.profile,
            binding_sha256: f.binding.digest.clone(),
        };
        let document_owner = catalog::Owner::Document {
            document: f.document,
        };
        let scope = identity.aggregate_scope(&document_owner).unwrap();
        let mut items = 32;
        let genesis = f
            .engine(&f.a)
            .refresh_root_catalog(&mut items)
            .unwrap()
            .unwrap()
            .root_sha;
        let (token, _, _) = fixture_defer_pending_for_worker(
            f.a.clone(),
            scope.clone(),
            f.document,
            genesis_base(&genesis, &scope.key_sha256),
        );
        super::recovery::register_document(f.a.clone(), &identity, document_owner, &genesis);
        let capture =
            ResidentPrepared::new(&f.a, &token, &capture_for(f.document), Uuid::new_v4(), 2)
                .unwrap();
        let writer = SelectedAdmission::new(f.a.clone(), scope.clone()).unwrap();
        let (_, capture) = SelectedAdmission::with_gates(&[&writer], |store| {
            capture.publish_locked(store, &token, &mut route::allowance())
        })
        .unwrap();
        let path = f.storage.0.join("selected-recognition-writer.json");
        files::atomic_json(
        &path,
        &json!({"paths":f.a.paths, "handwriting":{"enabled":handwriting,"profile":f.owner.profile}}),
    )
    .unwrap();
        preference_command::set_fixture_preference(
            f.a.clone(),
            &path,
            f.binding.clone(),
            &f.wire,
            json!({"product_settings":{"writer_enabled":true}}),
        )
        .unwrap();
        Self {
            correction: f,
            writer,
            scope,
            path,
            root: capture.root.record_id,
            image: SelectedWriterImage {
                source: capture.capture,
                turn: capture.user,
                ordinal: 0,
                page: 1,
            },
        }
    }

    fn policy(&self) -> preference_consumer::RequestPreferences {
        preference_consumer::RequestPreferences::writer_image_from_binding(
            self.correction.a.clone(),
            self.path.clone(),
            self.correction.binding.clone(),
            |_| panic!("selected Writer opened transport"),
        )
        .unwrap()
    }

    fn selection(&self) -> Selection {
        let snapshot = self
            .correction
            .a
            .selected_snapshot(&self.scope, MAX_ITEMS)
            .unwrap()
            .unwrap();
        let root = crate::conversation::SelectedDomainProjection::from_snapshot(&snapshot)
            .unwrap()
            .current_reference(Namespace::Conversation, self.root)
            .unwrap();
        Selection {
            accepted: snapshot.token,
            root,
            refine: None,
        }
    }

    fn request(&self) -> WriterImageRequest {
        WriterImageRequest {
            operation: Uuid::new_v4(),
            request: "Make a note".into(),
            image: self.image.clone(),
            guidance: None,
            model_profile: "fixture".into(),
        }
    }

    fn change(&self, kind: Change) {
        match kind {
            Change::Source => {
                let snapshot = self
                    .correction
                    .a
                    .selected_snapshot(&self.scope, MAX_ITEMS)
                    .unwrap()
                    .unwrap();
                let projection =
                    crate::conversation::SelectedDomainProjection::from_snapshot(&snapshot)
                        .unwrap();
                let mut capture = projection
                    .head(Namespace::Source, self.image.source.record_id)
                    .unwrap()
                    .0
                    .clone();
                capture.parents = BTreeSet::from([capture.revision_id]);
                capture.revision_id = Uuid::new_v4();
                capture.operation_id = Uuid::new_v4();
                SelectedAdmission::with_gates(&[&self.writer], |store| {
                    store.commit_selected(
                        &snapshot.token,
                        Uuid::new_v4(),
                        vec![capture],
                        BTreeMap::new(),
                    )?;
                    Ok(())
                })
                .unwrap();
            }
            Change::Preference => {
                let before = self.selection().accepted;
                preference_command::set_fixture_preference(
                    self.correction.a.clone(),
                    &self.path,
                    self.correction.binding.clone(),
                    &self.correction.wire,
                    json!({"product_settings":{"writer_enabled":false}}),
                )
                .unwrap();
                assert_eq!(before, self.selection().accepted);
            }
        }
    }

    fn has_receipt(&self, operation: Uuid) -> bool {
        self.correction
            .a
            .selected_receipt(&self.scope, operation)
            .unwrap()
            .is_some()
    }

    fn model<'a>(&'a self, input: &'a WriterImageRequest) -> Model<'a> {
        Model {
            writer: &self.writer,
            store: &self.correction.a,
            input,
            before: None,
            after: None,
            calls: Vec::new(),
            current: Vec::new(),
        }
    }
}

#[test]
fn selected_writer_image_stale_source_and_preference_refuse_submission_or_result() {
    for (kind, stage, after, calls) in [
        (Change::Source, 0, false, 0),
        (Change::Source, 0, true, 1),
        (Change::Preference, 1, false, 1),
        (Change::Preference, 1, true, 2),
    ] {
        let fixture = WriterFixture::new(false);
        let selection = fixture.selection();
        let input = fixture.request();
        let policy = fixture.policy();
        let entered = std::cell::Cell::new(false);
        let change = || {
            assert!(!entered.replace(true));
            fixture.change(kind);
        };
        let interlock = (stage, &change as &dyn Fn());
        let mut model = fixture.model(&input);
        model.before = (!after).then_some(interlock);
        model.after = after.then_some(interlock);
        let result = run_image(
            &fixture.writer,
            &selection,
            &input,
            &mut model,
            policy,
            None,
            &mut || false,
        );
        assert!(entered.get(), "{kind:?} did not reach stage {stage}");
        assert_eq!(
            model.calls.len(),
            calls,
            "{kind:?} changed at stage {stage}"
        );
        assert!(result.is_err() || result.unwrap().publication.is_none());
        assert!(fixture.has_receipt(input.preparation()));
        assert_eq!(fixture.has_receipt(input.operation), stage == 1);
        assert!(!fixture.has_receipt(writer::stage(input.operation, "finish")));
    }
}

#[test]
fn selected_writer_image_preparation_fault_recovers_only_exact_history() {
    use crate::conversation::IntentSelectionEvidence;
    use crate::writer::selected_command::{self, Operation};

    for fault in [
        Fault::BeforeActivation,
        Fault::AfterCommit,
        Fault::AfterActivation,
    ] {
        let fixture = WriterFixture::new(false);
        let selection = fixture.selection();
        let input = fixture.request();
        let before = fixture
            .correction
            .a
            .selected_snapshot_with_budget(&fixture.scope, MAX_ITEMS, &mut route::allowance())
            .unwrap()
            .unwrap();
        let envelope = json!({
            "wire_version":1,
            "expected":{
                "selection":IntentSelectionEvidence::from_token(&selection.accepted),
                "raw_head_sha256":before.accepted_history[0].sha256,
            },
            "root":selection.root,
            "request":input,
        });
        let policy = fixture.policy();
        let mut model = fixture.model(&input);
        fixture.correction.a.set_fault(fault).unwrap();
        let result = run_image(
            &fixture.writer,
            &selection,
            &input,
            &mut model,
            policy,
            None,
            &mut || false,
        );
        assert!(
            format!("{:#}", result.err().expect("faulted preparation continued"))
                .contains("injected storage interruption")
        );
        assert!(model.calls.is_empty(), "{fault:?}");
        let durable = fault != Fault::BeforeActivation;
        assert_eq!(
            fixture.has_receipt(input.preparation()),
            durable,
            "{fault:?}"
        );
        assert!(!fixture.has_receipt(input.operation));
        if !durable {
            assert_eq!(fixture.selection().accepted, selection.accepted);
            continue;
        }
        assert_ne!(fixture.selection().accepted, selection.accepted);
        assert!(run_image(
            &fixture.writer,
            &fixture.selection(),
            &input,
            &mut model,
            fixture.policy(),
            None,
            &mut || false,
        )
        .is_err());
        assert!(model.calls.is_empty());
        let WriterFixture {
            correction,
            writer,
            path,
            scope,
            ..
        } = fixture;
        let CorrectionFixture {
            a, wire, storage, ..
        } = correction;
        let before_network = wire.requests().len();
        let paths = a.paths.clone();
        let before_state = std::fs::read(paths.data.join("sync/state.json")).unwrap();
        let selected_before = a
            .selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token;
        let credential = paths.credentials.join("drive.json");
        let quarantined = paths.credentials.join("fixture-drive-unused.json");
        assert!(credential.is_file());
        std::fs::rename(&credential, &quarantined).unwrap();
        let mut config: serde_json::Value = files::json(&path, MAX_METADATA as u64).unwrap();
        config["model_credential"] = json!("missing-key");
        files::atomic_json(&path, &config).unwrap();
        drop(writer);
        drop(a);

        let command = |value: &serde_json::Value| {
            let bytes = serde_json::to_vec(value).unwrap();
            let mut report = selected_command::run(
                Operation::GenerateImage,
                Path::new("-"),
                Some(path.clone()),
                &mut bytes.as_slice(),
            );
            let mut bytes = Vec::new();
            report.write_to(&mut bytes).unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };
        let report = command(&envelope);
        assert_eq!(
            report["status"], "historical-pending",
            "{fault:?}: {report}"
        );
        assert_eq!(report["data"]["disposition"], "historical");
        assert_eq!(report["native_authority"], false);
        assert!(report["data"]["handles"].is_null());
        assert_eq!(
            report["data"]["preparation"]["operation"],
            json!(input.preparation())
        );
        let mut altered = envelope.clone();
        altered["request"]["request"] = json!("Altered instruction");
        assert_eq!(command(&altered)["status"], "unavailable");
        let mut altered = envelope;
        altered["expected"]["raw_head_sha256"] = json!(digest(b"different predecessor"));
        assert_eq!(command(&altered)["status"], "unavailable");
        assert_eq!(wire.requests().len(), before_network);
        assert!(!credential.exists() && quarantined.exists());
        assert_eq!(
            std::fs::read(paths.data.join("sync/state.json")).unwrap(),
            before_state
        );
        let reopened = Store::open(paths).unwrap();
        assert_eq!(
            reopened
                .selected_snapshot(&scope, MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            selected_before
        );
        assert!(reopened
            .selected_receipt(&scope, input.operation)
            .unwrap()
            .is_none());
        drop(reopened);
        drop(storage);
    }
}

#[test]
fn selected_handwriting_writer_normalizes_only_current_images_and_never_replays() {
    let fixture = WriterFixture::new(true);
    let f = &fixture.correction;
    let writer = &fixture.writer;
    let scope = &fixture.scope;
    let selection = || fixture.selection();
    let policy = || fixture.policy();
    let image = fixture.image.clone();
    let target_png = match writer
        .capture_images(&selection().accepted, image.source.record_id)
        .unwrap()
    {
        crate::conversation::SelectedCaptureImages::Resident(capture) => capture.images[0].clone(),
        _ => panic!("expected resident fixture capture"),
    };
    let mut target_correction = confirm(f.owner, Uuid::new_v4());
    target_correction["sample_png_base64"] = json!(STANDARD.encode(&target_png));
    let owner = HandwritingOwner::new(f.owner.profile, Uuid::new_v4());
    let first = confirm(owner, Uuid::new_v4());
    let mut revision = Uuid::nil();
    for index in 0..6 {
        match index {
            0 => {
                let target_revision = observed(f.command(&f.a, &target_correction, true));
                f.drive(&mut f.engine(&f.a), target_revision);
            }
            1 => revision = observed(f.command(&f.a, &first, true)),
            2 => {
                let mut sender = f.engine(&f.a);
                let key = owner.domain().unwrap().key_sha256().to_owned();
                for _ in 0..16 {
                    sender.step().unwrap();
                    if sender.view().unwrap().unwrap().entries.contains_key(&key) {
                        break;
                    }
                }
                let mut items = 32;
                assert!(sender
                    .refresh_root_catalog(&mut items)
                    .unwrap()
                    .unwrap()
                    .entries
                    .contains_key(&key));
            }
            _ => {}
        }
        let selection = selection();
        let input = WriterImageRequest {
            operation: Uuid::new_v4(),
            request: "Make a note".into(),
            image: image.clone(),
            guidance: None,
            model_profile: "fixture".into(),
        };
        let next = std::cell::Cell::new(None);
        let previous = revision;
        let replace = || {
            let mut update = first.clone();
            update["command"] = json!("update");
            update["operation"] = json!(Uuid::new_v4());
            update["expected_heads"] = json!([previous]);
            update["corrected_text"] = json!(format!("replacement {index}"));
            next.set(Some(observed(f.command(&f.a, &update, false))));
        };
        let mut model = Model {
            writer: &writer,
            store: &f.a,
            input: &input,
            calls: Vec::new(),
            current: Vec::new(),
            before: (index == 3).then_some((0, &replace as &dyn Fn())),
            after: match index {
                4 => Some((0, &replace as &dyn Fn())),
                5 => Some((1, &replace as &dyn Fn())),
                _ => None,
            },
        };
        let report = run_image(
            &writer,
            &selection,
            &input,
            &mut model,
            policy(),
            None,
            &mut || false,
        );
        assert_eq!(model.calls.len(), [2, 0, 2, 0, 1, 2][index]);
        assert_eq!(
            f.a.selected_receipt(&scope, input.preparation())
                .unwrap()
                .is_some(),
            index != 1
        );
        if index == 0 || index == 2 {
            let report = report.unwrap();
            assert_eq!(report.report.status, writer::Status::Generated);
            assert_eq!(model.calls[0].len(), if index == 0 { 1 } else { 2 });
            if index == 2 {
                assert!(model.calls[0][0].ends_with(&STANDARD.encode(png())));
            }
            assert!(model.calls[0]
                .last()
                .unwrap()
                .ends_with(&STANDARD.encode(&target_png)));
            assert!(model.calls[1].is_empty());
        } else {
            assert!(report.is_err() || report.unwrap().publication.is_none());
            assert_eq!(
                f.a.selected_receipt(&scope, input.operation)
                    .unwrap()
                    .is_some(),
                index == 5
            );
            assert!(f
                .a
                .selected_receipt(&scope, writer::stage(input.operation, "finish"))
                .unwrap()
                .is_none());
            if index == 1 {
                continue;
            }
            revision = next.get().unwrap();
            let before = model.calls.len();
            let snapshot = f.a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
            let root = crate::conversation::SelectedDomainProjection::from_snapshot(&snapshot)
                .unwrap()
                .current_reference(Namespace::Conversation, selection.root.record_id)
                .unwrap();
            let latest = Selection {
                accepted: snapshot.token,
                root,
                refine: None,
            };
            assert!(run_image(
                &writer,
                &latest,
                &input,
                &mut model,
                policy(),
                None,
                &mut || false
            )
            .is_err());
            assert_eq!(model.calls.len(), before);
        }
    }
    let selection = selection();
    let input = WriterImageRequest {
        operation: Uuid::new_v4(),
        request: "Make a note".into(),
        image: SelectedWriterImage { page: 2, ..image },
        guidance: None,
        model_profile: "fixture".into(),
    };
    let mut model = Model {
        writer: &writer,
        store: &f.a,
        input: &input,
        calls: Vec::new(),
        current: Vec::new(),
        before: None,
        after: None,
    };
    let error = run_image(
        &writer,
        &selection,
        &input,
        &mut model,
        policy(),
        None,
        &mut || false,
    )
    .err()
    .expect("wrong page was admitted");
    assert!(error.to_string().contains("capture page differs"));
    assert!(model.calls.is_empty());
    assert!(f
        .a
        .selected_receipt(&scope, input.preparation())
        .unwrap()
        .is_none());
}
