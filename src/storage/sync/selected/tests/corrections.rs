use super::super::catalog::current_generation;
use super::super::corrections as route;
use super::*;
use crate::handwriting::{self, Failure, MutationOutcome};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::io::Cursor;

struct CorrectionFixture {
    a: Arc<Store>,
    b: Arc<Store>,
    wire: Wire,
    binding: Binding,
    owner: HandwritingOwner,
    scope: SelectionScope,
    document: Uuid,
    storage: Fixture,
}

impl CorrectionFixture {
    fn new() -> Self {
        let storage = Fixture::new();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let digest = wire
            .client(storage.0.join("provider/drive.json"), group)
            .binding_digest();
        let profile = Uuid::new_v4();
        let owner = HandwritingOwner::new(profile, Uuid::new_v4());
        let scope = owner
            .domain()
            .unwrap()
            .scope(group, digest.clone())
            .unwrap();
        let document = Uuid::new_v4();
        let seed_owner = catalog::Owner::Document { document };
        let identity = catalog::Identity {
            group,
            profile,
            binding_sha256: digest.clone(),
        };
        let seed_scope = identity.aggregate_scope(&seed_owner).unwrap();
        existing_group(
            &wire,
            group,
            digest.clone(),
            profile,
            &seed_scope,
            seed_owner,
        );
        let a = storage.store("a");
        let b = storage.store("b");
        let a = compound(a, group, &digest);
        let b = compound(b, group, &digest);
        choose(&a, group, &digest);
        choose(&b, group, &digest);
        Self {
            a,
            b,
            wire,
            owner,
            scope,
            document,
            binding: Binding {
                group,
                root_id: "shared-root".into(),
                digest,
            },
            storage,
        }
    }

    fn command(
        &self,
        store: &Arc<Store>,
        request: &Value,
        allow_catalog: bool,
    ) -> handwriting::Report {
        let input =
            handwriting::parse(&mut Cursor::new(serde_json::to_vec(request).unwrap())).unwrap();
        route::command::admitted(
            store.clone(),
            SyncPolicy::default(),
            self.binding.clone(),
            input,
            || {
                assert!(allow_catalog, "established operation opened a transport");
                Ok(self.wire.client(
                    store.paths.credentials.join("drive.json"),
                    self.binding.group,
                ))
            },
        )
    }

    fn engine(&self, store: &Arc<Store>) -> Engine {
        fixture_engine(&self.wire, store.clone(), self.binding.group, 32)
    }

    fn capture_profile(
        &self,
        store: &Arc<Store>,
        targets: &BTreeSet<String>,
        reads: &mut crate::storage::selection::SelectedReadBudget,
    ) -> Result<handwriting::SelectedCapture> {
        route::SelectedContextOwner::capture_profile(
            store,
            &self.binding,
            self.owner.profile,
            targets,
            &[],
            reads,
            |_, _| Ok(()),
        )
    }

    fn drive(&self, engine: &mut Engine, revision: Uuid) {
        for _ in 0..24 {
            engine.step().unwrap();
            let Some(progress) = engine.checkpoint.scopes.get(&self.scope.key_sha256) else {
                continue;
            };
            let Some(applied) = &progress.applied else {
                continue;
            };
            if progress.pending.is_some() || progress.receive.is_some() {
                continue;
            }
            let snapshot = engine
                .store
                .selected_snapshot(&self.scope, MAX_ITEMS)
                .unwrap()
                .unwrap();
            let projection = route::project(
                &engine.store,
                self.owner,
                &snapshot.transaction.selected,
                &mut route::allowance(),
            )
            .unwrap();
            if projection.head() == revision
                && applied.membership
                    == receiving::membership(&snapshot.transaction.selected).unwrap()
            {
                return;
            }
        }
        panic!("correction {revision} did not settle");
    }
}

#[test]
fn selected_handwriting_worker_distinguishes_omitted_and_missing_samples() {
    let f = CorrectionFixture::new();
    let revision = observed(f.command(&f.a, &confirm(f.owner, Uuid::new_v4()), true));
    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, revision);
    let selected =
        f.a.selected_snapshot(&f.scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .transaction
            .selected;
    let sample = &selected.media[0];
    let sample_id = &sender.checkpoint.uploads[&sample.sha256].id;
    f.wire.remove_fixture_object(sample_id);

    let mut receiver = f.engine(&f.b);
    let error = (0..12)
        .find_map(|_| receiver.step().err())
        .expect("missing included sample must prevent activation");
    assert!(
        error
            .to_string()
            .contains("promised shared object not visible"),
        "{error:#}"
    );
    assert!(f
        .b
        .selected_snapshot(&f.scope, MAX_ITEMS)
        .unwrap()
        .is_none());

    // Give a fresh receiver the explicit omission at the fixed slot. A
    // coincidentally cached copy of the exact bytes still grants no sample.
    let mut omitted = selected.clone();
    omitted.media.clear();
    omitted.media_coverage = vec![Coverage::Omitted {
        sha256: sample.sha256.clone(),
        reason: crate::storage::Omission::UnavailableAtSource,
    }];
    let recorded = sender.checkpoint.scopes[&f.scope.key_sha256]
        .chain
        .last()
        .unwrap();
    let mut commit: Commit = serde_json::from_slice(&recorded.bytes).unwrap();
    let bytes = serde_json::to_vec(&omitted).unwrap();
    commit.manifest_id = "omitted-correction-manifest".into();
    commit.manifest = ObjectRef {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
    };
    f.wire.object(&commit.manifest_id, f.binding.group, bytes);
    f.wire.slot(
        &SlotBinding {
            group: f.binding.group,
            scope_sha256: f.scope.key_sha256.clone(),
            slot_id: commit.slot.clone(),
        },
        "omitted-correction-descriptor",
        serde_json::to_vec(&commit).unwrap(),
    );
    let peer = compound(
        f.storage.store("omitted"),
        f.binding.group,
        &f.binding.digest,
    );
    choose(&peer, f.binding.group, &f.binding.digest);
    let object_path = peer
        .paths
        .data
        .join("generations")
        .join(current_generation(&peer).unwrap().to_string())
        .join("objects")
        .join(&sample.sha256);
    files::object(&object_path, &sample.sha256, &png()).unwrap();
    let mut receiver = f.engine(&peer);
    f.drive(&mut receiver, revision);
    assert_eq!(receiver.step().unwrap(), Status::IncompleteMedia);
    let request = json!({"handwriting_version": 1, "command": "inspect",
        "profile": f.owner.profile, "correction": f.owner.correction});
    let report = f.command(&peer, &request, false);
    assert_eq!(report.status, handwriting::Status::Partial);
    assert_eq!(
        report.facts[0].sample_availability,
        handwriting::Availability::Unavailable
    );
    assert_eq!(files::read(&object_path, sample.bytes).unwrap(), png());
    assert!(peer.read_object(sample).is_err());
    let query = f.command(&peer, &profile_query(f.owner.profile, None), true);
    assert_eq!(query.status, handwriting::Status::Partial);
    assert_eq!(query.diagnostics.missing_media, 1);
    assert!(query.facts.is_empty());
    let automatic = f
        .capture_profile(&peer, &BTreeSet::new(), &mut route::allowance())
        .unwrap();
    assert_eq!(automatic.selection.status(), "partial-empty");
    assert_eq!(automatic.selection.skipped_media, 1);
    let request = [handwriting::ExampleSelection {
        correction: f.owner.correction,
        revision,
    }];
    assert_eq!(
        handwriting::ConfirmedContext::capture_selected(
            peer,
            f.owner.profile,
            &request,
            &BTreeSet::new(),
            &mut route::allowance(),
        )
        .err(),
        Some(handwriting::ContextFailure::Unavailable),
    );
}

#[test]
fn selected_handwriting_automatic_capture_distinguishes_pending_local_from_empty() {
    let f = CorrectionFixture::new();
    let mut engine = f.engine(&f.a);
    let targets = BTreeSet::new();
    assert!(f
        .capture_profile(&f.a, &targets, &mut route::allowance())
        .is_err());
    let mut items = 32;
    engine.refresh_root_catalog(&mut items).unwrap().unwrap();
    let mut reads = route::allowance();
    let empty = f.capture_profile(&f.a, &targets, &mut reads).unwrap();
    assert_eq!(empty.selection.status(), "complete-empty");
    empty
        .context
        .with_selected_phase(&[], &mut reads, |store| {
            assert!(matches!(
                store.selected_checkpoint.try_write(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            Ok(())
        })
        .unwrap();
    assert!(f.a.selected_checkpoint.try_write().is_ok());
    observed(f.command(&f.a, &confirm(f.owner, Uuid::new_v4()), true));
    assert!(empty
        .context
        .with_selected_phase::<()>(&[], &mut reads, |_| panic!("new local scope admitted"))
        .is_err());
    let pending = f.capture_profile(&f.a, &targets, &mut reads).unwrap();
    assert_eq!(pending.selection.status(), "partial-empty");
    assert_eq!(pending.selection.unsupported, 1);
}

#[test]
fn selected_handwriting_checkpoint_rejects_a_stale_engine_write() {
    let f = CorrectionFixture::new();
    let mut engine = f.engine(&f.a);
    let mut stale = f.engine(&f.a);
    let mut items = 32;
    let view = engine.refresh_root_catalog(&mut items).unwrap().unwrap();
    let before = std::fs::read(f.a.paths.data.join("sync/state.json")).unwrap();
    let error = stale.save().unwrap_err();
    assert!(error.to_string().contains("advanced before save"));
    assert_eq!(
        std::fs::read(f.a.paths.data.join("sync/state.json")).unwrap(),
        before
    );
    stale.step().unwrap();
    assert_eq!(
        stale.checkpoint.observed_catalog_tail.as_ref(),
        Some(&view.base)
    );
}

#[test]
fn selected_handwriting_automatic_ranks_metadata_before_capped_sample_reads() {
    let f = CorrectionFixture::new();
    let mut expected = Vec::new();
    for confirmed_ms in 1..=5 {
        let owner = HandwritingOwner::new(f.owner.profile, Uuid::new_v4());
        let mut request = confirm(owner, Uuid::new_v4());
        request["confirmed_ms"] = json!(confirmed_ms);
        expected.push(observed(f.command(&f.a, &request, true)));
    }
    let mut engine = f.engine(&f.a);
    for _ in 0..20 {
        engine.step().unwrap();
        if engine.view().unwrap().unwrap().entries.len() == 6 {
            break;
        }
    }
    let mut items = 32;
    engine.refresh_root_catalog(&mut items).unwrap().unwrap();
    let mut reads = route::allowance();
    let targets = BTreeSet::from([digest(&png())]);
    let empty = f.capture_profile(&f.a, &targets, &mut reads).unwrap();
    assert_eq!(empty.selection.status(), "complete-empty");
    assert_eq!(empty.selection.excluded_targets, 5);
    assert_eq!(reads.usage().media_bytes, 0);
    let ranked = f
        .capture_profile(&f.a, &BTreeSet::new(), &mut reads)
        .unwrap();
    assert_eq!(ranked.selection.status(), "partial");
    assert_eq!(ranked.selection.checked, 3);
    assert_eq!(ranked.selection.unexamined, 2);
    assert_eq!(
        ranked
            .context
            .identity()
            .examples
            .iter()
            .map(|example| example.revision)
            .collect::<Vec<_>>(),
        expected.into_iter().rev().take(3).collect::<Vec<_>>()
    );
}

#[test]
fn selected_handwriting_reader_uses_same_examples_and_refuses_stale_submission_and_result() {
    use crate::conversation::{
        fixture_defer_pending_for_worker, resident_capture::tests::capture_for,
        selected_reader_prepare::ResidentPrepared, Ledger, Outcome, Record,
        SelectedDomainProjection,
    };
    use crate::storage::sync::selected::{ReaderPhase, ReaderPreferences};
    use crate::workflow::{orchestrator::Orchestrator, Workflow};
    use crate::{llm::ModelHandoff, LLMEngine};

    #[derive(Clone, Copy)]
    enum Change {
        None,
        BeforeVerification,
        BeforeResult,
    }

    struct ScriptedReader<'a> {
        change: Change,
        replace: &'a dyn Fn(),
        payload: Vec<String>,
        proof: &'a std::cell::RefCell<Vec<Vec<String>>>,
    }
    impl LLMEngine for ScriptedReader<'_> {
        fn guarded_timeout(&self) -> Option<Duration> {
            Some(Duration::from_secs(1))
        }
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, image: &str) {
            self.payload.push(image.into());
        }
        fn clear_content(&mut self) {
            self.payload.clear();
        }
        fn execute(&mut self) -> Result<String> {
            panic!("unguarded selected Reader submission")
        }
        fn execute_with_handoff(
            &mut self,
            progress: &mut dyn FnMut() -> Result<()>,
            handoff: &mut ModelHandoff<'_>,
        ) -> Result<String> {
            progress()?;
            let stage = self.proof.borrow().len();
            handoff(&mut || {
                self.proof.borrow_mut().push(self.payload.clone());
                Ok(())
            })?;
            if matches!(
                (self.change, stage),
                (Change::BeforeVerification, 0) | (Change::BeforeResult, 1)
            ) {
                (self.replace)();
            }
            Ok(if stage == 0 {
                "QUESTION: What is x?\nQUESTION_BOX: 10,10,100,100\nSELECTION_CENTER: 100,100\n---\nANSWER: x = 7"
            } else {
                "TRANSCRIPTION: What is x?"
            }.into())
        }
    }

    let f = CorrectionFixture::new();
    let identity = catalog::Identity {
        group: f.binding.group,
        profile: f.owner.profile,
        binding_sha256: f.binding.digest.clone(),
    };
    let document_owner = catalog::Owner::Document {
        document: f.document,
    };
    let document_scope = identity.aggregate_scope(&document_owner).unwrap();
    let mut items = 32;
    let genesis = f
        .engine(&f.a)
        .refresh_root_catalog(&mut items)
        .unwrap()
        .unwrap()
        .root_sha;
    let _ = fixture_defer_pending_for_worker(
        f.a.clone(),
        document_scope.clone(),
        f.document,
        genesis_base(&genesis, &document_scope.key_sha256),
    );
    super::recovery::register_document(f.a.clone(), &identity, document_owner, &genesis);
    let first = confirm(f.owner, Uuid::new_v4());
    let mut revision = observed(f.command(&f.a, &first, true));
    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, revision);
    drop(sender);
    let path = f.storage.0.join("selected-recognition-reader.json");
    files::atomic_json(
        &path,
        &json!({
            "paths": f.a.paths,
            "reader": {"selection": document_scope},
            "product_settings": {"reader_enabled": true},
            "handwriting": {"enabled": true, "profile": f.owner.profile},
        }),
    )
    .unwrap();
    preference_command::set_fixture_preference(
        f.a.clone(),
        &path,
        f.binding.clone(),
        &f.wire,
        json!({"product_settings": {"reader_enabled": true, "writer_enabled": false}}),
    )
    .unwrap();
    let reader = Arc::new(SelectedAdmission::new(f.a.clone(), document_scope.clone()).unwrap());
    let requests_before = f.wire.requests().len();
    for (i, change) in [
        Change::None,
        Change::BeforeVerification,
        Change::BeforeResult,
    ]
    .into_iter()
    .enumerate()
    {
        let (policy, token) = ReaderPreferences::from_binding(
            f.a.clone(),
            path.clone(),
            &reader,
            Instant::now(),
            f.binding.clone(),
            |_| panic!("already selected Reader opened transport"),
        )
        .unwrap_or_else(|error| panic!("Reader preference admission failed: {error:#}"));
        let prepared =
            ResidentPrepared::new(&f.a, &token, &capture_for(f.document), Uuid::new_v4(), 1)
                .unwrap();
        let (token, acknowledgment) = {
            let mut reads = policy.reads(ReaderPhase::Acquisition).unwrap();
            let [catalog, preferences] = policy.gates();
            SelectedAdmission::with_gates(&[&reader, catalog, preferences], |store| {
                policy.verify_locked(store, &reader, &token, &mut reads)?;
                prepared.publish_locked(store, &token, &mut reads)
            })
            .unwrap()
        };
        let root = acknowledgment.root.record_id;
        let mut update = first.clone();
        update["command"] = json!("update");
        update["operation"] = json!(Uuid::new_v4());
        update["expected_heads"] = json!([revision]);
        update["confirmed_ms"] = json!(i + 2);
        update["corrected_text"] = json!(format!("10 N {i}"));
        let replacement = std::cell::Cell::new(None);
        let replace = || replacement.set(Some(observed(f.command(&f.a, &update, false))));
        let proof = std::cell::RefCell::new(Vec::new());
        let model = ScriptedReader {
            change,
            replace: &replace,
            payload: Vec::new(),
            proof: &proof,
        };
        let workflow = Workflow::selected_owned_reader_fixture(reader.clone());
        let mut receiver = Orchestrator::new(workflow, model, Ledger::new(f.a.clone()));
        let result = receiver.receive_resident_prepared(token, acknowledgment, policy);
        drop(receiver);
        let proof = proof.into_inner();
        let fixture_image = STANDARD.encode(png());
        assert!(proof.iter().all(|batch| batch
            .first()
            .is_some_and(|image| image.ends_with(&fixture_image))));
        let snapshot =
            f.a.selected_snapshot(&document_scope, MAX_ITEMS)
                .unwrap()
                .unwrap();
        let generated = SelectedDomainProjection::from_snapshot(&snapshot)
            .unwrap()
            .inspect(root)
            .unwrap()
            .iter()
            .any(
                |record| matches!(record, Record::Turn(turn) if turn.outcome == Outcome::Generated),
            );
        if matches!(change, Change::None) {
            result.unwrap();
            assert_eq!(proof.len(), 2);
            assert_eq!(proof[0], proof[1]);
            assert!(generated);
        } else {
            assert_eq!(
                proof.len(),
                if matches!(change, Change::BeforeResult) {
                    2
                } else {
                    1
                }
            );
            assert!(!generated);
            if matches!(change, Change::BeforeResult) {
                assert!(format!("{:#}", result.unwrap_err()).contains("example changed"));
            }
            revision = replacement.get().unwrap();
            assert!(!snapshot.selected_records.iter().any(|record| matches!(Ledger::decode(record),
                Ok(Record::Turn(turn)) if turn.conversation == root && turn.outcome == Outcome::Interpreted)));
        }
    }
    assert_eq!(f.wire.requests().len(), requests_before);
}

fn png() -> Vec<u8> {
    let image =
        image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 1, image::Luma([0])));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn confirm(owner: HandwritingOwner, operation: Uuid) -> Value {
    json!({
        "handwriting_version": 1, "command": "confirm", "profile": owner.profile,
        "correction": owner.correction, "operation": operation, "expected_heads": [],
        "confirmation": "user-confirmed", "original_text": "1O",
        "corrected_text": "10", "confirmed_ms": 1,
        "provenance": {"kind": "caller-confirmed-sample", "description": "Fixture sample", "source": null},
        "sample_png_base64": STANDARD.encode(png()),
    })
}

fn profile_query(profile: Uuid, text: Option<&str>) -> Value {
    json!({"handwriting_version": 1, "command": "query", "profile": profile,
        "query_text": text, "max_matches": 16, "max_encoded_bytes": 65536})
}

fn observed(report: handwriting::Report) -> Uuid {
    assert_eq!(report.failure, None);
    assert_eq!(
        report.diagnostics.mutation_outcome,
        MutationOutcome::Observed
    );
    report.facts[0].revision
}

#[test]
fn selected_handwriting_context_checks_store_catalog_and_correction_each_phase() {
    use handwriting::{ConfirmedContext, ContextFailure, ExampleSelection};

    let f = CorrectionFixture::new();
    let first = confirm(f.owner, Uuid::new_v4());
    let revision = observed(f.command(&f.a, &first, true));
    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, revision);
    let mut receiver = f.engine(&f.b);
    f.drive(&mut receiver, revision);
    let requests = f.wire.requests().len();
    let mut selection = [ExampleSelection {
        correction: f.owner.correction,
        revision,
    }];
    let mut reads = route::allowance();
    let mut targets = BTreeSet::from([digest(&png())]);
    assert_eq!(
        ConfirmedContext::capture_selected(
            f.b.clone(),
            f.owner.profile,
            &selection,
            &targets,
            &mut reads
        )
        .err(),
        Some(ContextFailure::Unavailable),
    );
    targets.clear();
    let captured = ConfirmedContext::capture_selected(
        f.b.clone(),
        f.owner.profile,
        &selection,
        &targets,
        &mut reads,
    )
    .unwrap();
    assert_eq!(captured.identity().examples[0].revision, revision);
    assert_eq!(captured.examples[0].png, png());
    let document = SelectionScope {
        key_sha256: digest(b"fixture document"),
        ..f.scope.clone()
    };
    let gate = SelectedAdmission::new(f.b.clone(), document.clone()).unwrap();
    let foreign = SelectedAdmission::new(f.a.clone(), document).unwrap();
    let used = reads.usage();
    captured
        .with_selected_phase(&[&gate], &mut reads, |store| {
            assert_eq!(store.actor_id, f.b.actor_id);
            Ok(())
        })
        .unwrap();
    assert!(reads.usage().media_bytes > used.media_bytes);
    assert!(captured
        .with_selected_phase::<()>(&[&foreign], &mut reads, |_| panic!("foreign gate admitted"))
        .is_err());
    let mut exhausted = crate::storage::selection::SelectedReadBudget::new(0, 0, 0, 0);
    assert!(captured
        .with_selected_phase::<()>(&[&gate], &mut exhausted, |_| panic!("budget bypassed"))
        .is_err());
    assert!(exhausted.exhausted());
    assert_eq!(f.wire.requests().len(), requests);

    let mut updated = first;
    updated["command"] = json!("update");
    updated["operation"] = json!(Uuid::new_v4());
    updated["expected_heads"] = json!([revision]);
    updated["confirmed_ms"] = json!(2);
    updated["corrected_text"] = json!("10 N");
    let updated = observed(f.command(&f.b, &updated, false));
    let error = captured
        .with_selected_phase::<()>(&[&gate], &mut reads, |_| {
            panic!("stale correction admitted")
        })
        .unwrap_err();
    assert!(error.to_string().contains("example changed"), "{error:#}");
    assert_eq!(
        ConfirmedContext::capture_selected(
            f.b.clone(),
            f.owner.profile,
            &selection,
            &targets,
            &mut reads
        )
        .err(),
        Some(ContextFailure::Stale),
    );
    selection[0].revision = updated;
    let captured = ConfirmedContext::capture_selected(
        f.b.clone(),
        f.owner.profile,
        &selection,
        &targets,
        &mut reads,
    )
    .unwrap();
    let sibling = HandwritingOwner::new(f.owner.profile, Uuid::new_v4());
    observed(f.command(&f.a, &confirm(sibling, Uuid::new_v4()), true));
    let sibling_scope = sibling
        .domain()
        .unwrap()
        .scope(f.binding.group, f.binding.digest.clone())
        .unwrap();
    let mut sender = f.engine(&f.a);
    for _ in 0..12 {
        sender.step().unwrap();
        if sender
            .view()
            .unwrap()
            .unwrap()
            .entries
            .contains_key(&sibling_scope.key_sha256)
        {
            break;
        }
    }
    let mut receiver = f.engine(&f.b);
    let mut items = 32;
    receiver.refresh_root_catalog(&mut items).unwrap().unwrap();
    assert!(receiver
        .view()
        .unwrap()
        .unwrap()
        .entries
        .contains_key(&sibling_scope.key_sha256));
    let error = captured
        .with_selected_phase::<()>(&[&gate], &mut reads, |_| panic!("stale catalog admitted"))
        .unwrap_err();
    assert!(error.to_string().contains("catalog changed"), "{error:#}");
}

#[test]
fn selected_handwriting_profile_query_applies_match_and_report_limits_across_scopes() {
    let f = CorrectionFixture::new();
    let second = HandwritingOwner::new(f.owner.profile, Uuid::new_v4());
    let second_scope = second
        .domain()
        .unwrap()
        .scope(f.binding.group, f.binding.digest.clone())
        .unwrap();
    observed(f.command(&f.a, &confirm(f.owner, Uuid::new_v4()), true));
    observed(f.command(&f.a, &confirm(second, Uuid::new_v4()), true));
    let mut sender = f.engine(&f.a);
    let mut receiver = f.engine(&f.b);
    for _ in 0..24 {
        sender.step().unwrap();
        receiver.step().unwrap();
        if [&f.scope, &second_scope].into_iter().all(|scope| {
            receiver
                .checkpoint
                .scopes
                .get(&scope.key_sha256)
                .is_some_and(|p| p.applied.is_some())
        }) {
            break;
        }
    }
    let mut query = profile_query(f.owner.profile, Some("1O"));
    let both = f.command(&f.b, &query, true);
    assert_eq!(both.status, handwriting::Status::Complete);
    assert_eq!(both.diagnostics.matched, 2);
    assert_eq!(both.facts.len(), 2);
    assert_ne!(both.facts[0].record, both.facts[1].record);
    query["max_matches"] = json!(1);
    let capped = f.command(&f.b, &query, true);
    assert_eq!(capped.status, handwriting::Status::Partial);
    assert_eq!(capped.diagnostics.omitted, 1);
    assert_eq!(capped.facts[0].record, both.facts[0].record);
    query["max_encoded_bytes"] = json!(512);
    let bounded = f.command(&f.b, &query, true);
    assert_eq!(bounded.status, handwriting::Status::Partial);
    assert!(bounded.facts.is_empty());
    assert!(serde_json::to_vec(&bounded).unwrap().len() < 512);

    let replaced =
        f.b.selected_snapshot(&second_scope, MAX_ITEMS)
            .unwrap()
            .unwrap();
    f.b.activate_selected(
        &replaced.token,
        crate::storage::selection::SelectionChange {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"unverified replacement descriptor"),
            selected: replaced.transaction.selected,
            retained: replaced.transaction.retained,
        },
        BTreeMap::new(),
    )
    .unwrap();
    let after = f.command(&f.b, &profile_query(f.owner.profile, Some("1O")), true);
    assert_eq!(after.status, handwriting::Status::Partial);
    assert_eq!(after.diagnostics.unsupported, 1);
    assert_eq!(after.facts.len(), 1);
    let inspect = json!({"handwriting_version": 1, "command": "inspect",
        "profile": second.profile, "correction": second.correction});
    assert_eq!(
        f.command(&f.b, &inspect, false).failure,
        Some(Failure::StoreUnavailable)
    );
}

#[test]
fn selected_handwriting_command_transfers_exact_bytes_and_keeps_original_replay() {
    let mut f = CorrectionFixture::new();
    let operation = Uuid::new_v4();
    let query = profile_query(f.owner.profile, None);
    assert_eq!(
        f.command(&f.a, &query, true).status,
        handwriting::Status::CompleteEmpty
    );
    let first = confirm(f.owner, operation);
    let revision = observed(f.command(&f.a, &first, true));
    let pending = f.command(&f.a, &query, true);
    assert_eq!(pending.status, handwriting::Status::Partial);
    assert!(pending.facts.is_empty());
    let replay = f.command(&f.a, &first, false);
    assert!(replay.historical);
    assert_eq!(observed(replay), revision);
    let mut changed = first.clone();
    changed["corrected_text"] = json!("changed");
    assert_eq!(
        f.command(&f.a, &changed, false).failure,
        Some(Failure::OperationConflict)
    );

    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, revision);
    let mut receiver = f.engine(&f.b);
    f.b.set_fault(Fault::AfterActivation).unwrap();
    let mut receipt = None;
    for _ in 0..12 {
        if receiver.step().is_err() {
            receipt = receiver.checkpoint.scopes[&f.scope.key_sha256]
                .receive
                .as_ref()
                .map(|r| r.operation);
            break;
        }
    }
    let receipt = receipt.expect("receive interrupted after actual Store activation");
    assert!(f.b.selected_receipt(&f.scope, receipt).unwrap().is_some());
    drop(receiver);
    let paths = f.b.paths.clone();
    drop(f.b);
    f.b = Arc::new(Store::open_existing_for_mode(paths).unwrap());
    let before = f.wire.slot_posts();
    let mut receiver = f.engine(&f.b);
    f.drive(&mut receiver, revision);
    assert_eq!(
        receiver.checkpoint.scopes[&f.scope.key_sha256]
            .applied
            .as_ref()
            .unwrap()
            .operation,
        receipt
    );
    assert_eq!(f.wire.slot_posts(), before);
    assert!(f.b.selected_receipt(&f.scope, operation).unwrap().is_none());
    assert!(f.b.selected_attempt_inventory().unwrap().is_empty());
    let sent = f.a.selected_snapshot(&f.scope, MAX_ITEMS).unwrap().unwrap();
    let received = f.b.selected_snapshot(&f.scope, MAX_ITEMS).unwrap().unwrap();
    assert!(shared::same_membership(
        &sent.transaction.selected,
        &received.transaction.selected
    ));
    let sample = &received.transaction.selected.media[0];
    assert_eq!(f.b.read_object(sample).unwrap(), png());
    let matches = f.command(&f.b, &profile_query(f.owner.profile, Some("1o")), true);
    assert_eq!(matches.status, handwriting::Status::Complete);
    assert_eq!(matches.facts[0].revision, revision);
    let no_match = f.command(&f.b, &profile_query(f.owner.profile, Some("other")), true);
    assert_eq!(no_match.status, handwriting::Status::CompleteEmpty);
    let mut query = json!({"handwriting_version": 1, "command": "status",
        "profile": f.owner.profile, "correction": f.owner.correction, "operation": operation});
    assert_eq!(
        f.command(&f.b, &query, false).status,
        handwriting::Status::CompleteEmpty
    );
    query["operation"] = json!(receipt);
    assert_eq!(
        f.command(&f.b, &query, false).failure,
        Some(Failure::OperationConflict)
    );

    drop(sender);
    drop(receiver);
    let mut winning_update = first.clone();
    winning_update["command"] = json!("update");
    winning_update["expected_heads"] = json!([revision]);
    winning_update["operation"] = json!(Uuid::new_v4());
    winning_update["corrected_text"] = json!("10 N");
    let mut losing_update = winning_update.clone();
    losing_update["operation"] = json!(Uuid::new_v4());
    losing_update["corrected_text"] = json!("10 newtons");
    let winner = observed(f.command(&f.a, &winning_update, false));
    let loser = observed(f.command(&f.b, &losing_update, false));
    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, winner);
    let mut receiver = f.engine(&f.b);
    f.drive(&mut receiver, winner);
    let received = f.b.selected_snapshot(&f.scope, MAX_ITEMS).unwrap().unwrap();
    assert!(received.transaction.retained.iter().any(|manifest| {
        let projection = route::project(&f.b, f.owner, manifest, &mut route::allowance()).unwrap();
        projection.head() == loser && projection.sample() == Some(png().as_slice())
    }));
    drop(sender);
    drop(receiver);

    let forget = json!({"handwriting_version": 1, "command": "forget", "profile": f.owner.profile,
        "correction": f.owner.correction, "operation": Uuid::new_v4(), "expected_heads": [winner]});
    let forgotten = observed(f.command(&f.a, &forget, false));
    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, forgotten);
    let mut receiver = f.engine(&f.b);
    f.drive(&mut receiver, forgotten);
    let snapshot = f.b.selected_snapshot(&f.scope, MAX_ITEMS).unwrap().unwrap();
    assert_eq!(
        snapshot.transaction.selected.media,
        std::slice::from_ref(sample)
    );
    let inspect = json!({"handwriting_version": 1, "command": "inspect",
        "profile": f.owner.profile, "correction": f.owner.correction});
    assert_eq!(
        f.command(&f.b, &inspect, false).facts[0].state,
        handwriting::FactState::Tombstone
    );
    assert_eq!(
        f.command(&f.b, &profile_query(f.owner.profile, None), true)
            .status,
        handwriting::Status::CompleteEmpty
    );
    let mut rollback = first;
    rollback["command"] = json!("update");
    rollback["operation"] = json!(Uuid::new_v4());
    rollback["expected_heads"] = json!([forgotten]);
    rollback["confirmed_ms"] = json!(3);
    let restored = observed(f.command(&f.a, &rollback, false));
    assert_ne!(restored, revision);
    let mut sender = f.engine(&f.a);
    f.drive(&mut sender, restored);
    f.drive(&mut receiver, restored);
    let fact = &f.command(&f.b, &inspect, false).facts[0];
    assert_eq!(fact.value.as_ref().unwrap().corrected_text, "10");
    assert_eq!(
        fact.sample_availability,
        handwriting::Availability::Available
    );
}

#[test]
fn selected_handwriting_competing_correction_commands_reuse_one_local_operation() {
    let f = CorrectionFixture::new();
    let operation = Uuid::new_v4();
    let a = confirm(f.owner, operation);
    let other = HandwritingOwner::new(f.owner.profile, Uuid::new_v4());
    let b = confirm(other, operation);
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        let first = threads.spawn(|| {
            barrier.wait();
            f.command(&f.a, &a, true)
        });
        let second = threads.spawn(|| {
            barrier.wait();
            f.command(&f.a, &b, true)
        });
        let mut failures = [
            first.join().unwrap().failure,
            second.join().unwrap().failure,
        ];
        failures.sort_by_key(Option::is_some);
        assert_eq!(failures, [None, Some(Failure::OperationConflict)]);
    });
    let other_scope = other
        .domain()
        .unwrap()
        .scope(f.binding.group, f.binding.digest.clone())
        .unwrap();
    assert_eq!(
        [&f.scope, &other_scope]
            .into_iter()
            .filter(|scope| { f.a.selected_receipt(scope, operation).unwrap().is_some() })
            .count(),
        1
    );
}
