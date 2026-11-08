use super::*;
use crate::{
    conversation::{
        notebook::{
            selected::fixture_resident, NotebookPlan, NotebookStageKind, NotebookStageOutcome,
        },
        *,
    },
    retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    storage::{
        selection::{SelectedReadBudget, SelectionChange},
        Coverage,
    },
    writer::notebook::{runner::Status as NotebookStatus, selected::run_fixture},
    LLMEngine,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    BeforeHandoff,
    AfterHandoff,
}

struct Interlock<'a> {
    ordinal: usize,
    boundary: Boundary,
    change: Box<dyn FnOnce() + 'a>,
}

#[derive(Default)]
struct Model<'a> {
    calls: usize,
    parts: Vec<String>,
    images: Vec<String>,
    interlock: Option<Interlock<'a>>,
    invalid_output: bool,
    expected_memory: bool,
}
impl Model<'_> {
    fn reach(&mut self, ordinal: usize, boundary: Boundary) {
        if self
            .interlock
            .as_ref()
            .is_some_and(|interlock| interlock.ordinal == ordinal && interlock.boundary == boundary)
        {
            (self.interlock.take().unwrap().change)();
        }
    }
}

impl LLMEngine for Model<'_> {
    fn guarded_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(text.to_owned());
    }
    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.into());
    }
    fn clear_content(&mut self) {
        self.parts.clear();
        self.images.clear();
    }
    fn execute(&mut self) -> Result<String> {
        panic!("selected notebook bypassed guarded handoff");
    }
    fn execute_with_handoff(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut crate::llm::ModelHandoff<'_>,
    ) -> Result<String> {
        let ordinal = self.calls;
        progress()?;
        self.reach(ordinal, Boundary::BeforeHandoff);
        handoff(&mut || {
            self.calls += 1;
            Ok(())
        })?;
        self.reach(ordinal, Boundary::AfterHandoff);
        progress()?;
        let payload: serde_json::Value = serde_json::from_str(
            self.parts
                .iter()
                .find(|part| part.starts_with('{'))
                .unwrap(),
        )?;
        assert_eq!(payload["request"], "Preserve the source facts.");
        if self.invalid_output {
            return Ok("  ".into());
        }
        if payload.get("ordered_map_results").is_some() {
            assert_eq!(payload["ordered_map_results"].as_array().unwrap().len(), 2);
            assert_eq!(
                self.parts
                    .iter()
                    .any(|part| part.contains("notebook fixture memory")),
                self.expected_memory
            );
            assert_eq!(self.images.len(), 4);
            let associations = payload["historical_images"].as_array().unwrap();
            let hashes = payload["historical_png_base64_sha256"].as_array().unwrap();
            use base64::Engine as _;
            for (index, image) in self.images.iter().enumerate() {
                let bytes = base64::engine::general_purpose::STANDARD.decode(image)?;
                assert_eq!(associations[index]["attachment_index"], index);
                assert_eq!(associations[index]["ordinal"], index);
                assert_eq!(associations[index]["sha256"], digest(&bytes));
                assert_eq!(hashes[index], digest(image.as_bytes()));
            }
            Ok("# Exact note\n\nForce = 10 N.".into())
        } else {
            assert_eq!(payload["chunk"]["ordinal"], self.calls - 1);
            assert!(self.images.is_empty() && payload.get("historical_images").is_none());
            Ok("Force = 10 N.".into())
        }
    }
}

struct NotebookFixture {
    fixture: Fixture,
    wire: Wire,
    first: Arc<Store>,
    second: Arc<Store>,
    writer: SelectedAdmission,
    identity: catalog::Identity,
    operation: Uuid,
    conversation: Uuid,
    config: PathBuf,
    request: serde_json::Value,
    frozen: crate::writer::notebook::ValidatedNotebook,
}

impl NotebookFixture {
    fn new(with_memory: bool) -> Self {
        let fixture = Fixture::new();
        let wire = Wire::new();
        let group = Uuid::new_v4();
        let profile = Uuid::new_v4();
        let first = fixture.store("writer");
        let second = fixture.store("receiver");
        let binding = wire
            .client(first.paths.credentials.join("drive.json"), group)
            .binding_digest();
        let document = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        let root = Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: 1,
            record_id: conversation,
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: first.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(Record::Root(crate::conversation::Root {
                id: conversation,
                next_sequence: 1,
                binding: None,
                created_ms: 1,
                updated_ms: 1,
            }))
            .unwrap(),
            media_descriptors: vec![],
        };
        let first = compound(first, group, &binding);
        let second = compound(second, group, &binding);
        choose(&first, group, &binding);
        choose(&second, group, &binding);
        let identity = catalog::Identity {
            group,
            profile,
            binding_sha256: binding,
        };
        let owner = catalog::Owner::Document { document };
        let scope = identity.aggregate_scope(&owner).unwrap();
        let root_sha = existing_group(
            &wire,
            group,
            identity.binding_sha256.clone(),
            profile,
            &scope,
            owner.clone(),
        );
        let bytes = serde_json::to_vec_pretty(&root).unwrap();
        let root_ref = ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        let turn_id = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        let resident = fixture_resident(document, conversation, turn_id, source_id).unwrap();
        let mut objects = BTreeMap::from([
            (root_ref.sha256.clone(), bytes),
            (
                resident.evidence.original_png.sha256.clone(),
                resident.original_png,
            ),
        ]);
        let mut descriptors = vec![resident.evidence.original_png.clone()];
        for (descriptor, bytes) in resident.evidence.images.iter().zip(resident.images) {
            descriptors.push(descriptor.media.clone());
            objects.insert(descriptor.media.sha256.clone(), bytes);
        }
        let mut media: Vec<_> = descriptors
            .iter()
            .map(|descriptor| ObjectRef {
                sha256: descriptor.sha256.clone(),
                bytes: descriptor.bytes,
            })
            .collect();
        media.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        let media_coverage = media
            .iter()
            .map(|object| Coverage::Included {
                sha256: object.sha256.clone(),
            })
            .collect();
        let turn = Record::Turn(Turn {
            id: turn_id,
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some("Earlier image question?".into()),
            sources: vec![source_id],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        });
        let evidence = Record::ResidentCapture(Box::new(resident.evidence));
        let mut records = vec![root_ref.clone()];
        let mut namespaces = BTreeMap::from([(root_ref.sha256.clone(), Namespace::Conversation)]);
        for (record_id, namespace, schema, record, media_descriptors) in [
            (turn_id, Namespace::Conversation, 1, turn, vec![]),
            (source_id, Namespace::Source, 2, evidence, descriptors),
        ] {
            let envelope = Envelope {
                envelope_version: FORMAT,
                namespace,
                domain_schema_version: schema,
                record_id,
                revision_id: Uuid::new_v4(),
                operation_id: Uuid::new_v4(),
                actor_id: first.actor_id,
                parents: Default::default(),
                kind: Kind::Value,
                payload: serde_json::to_value(record).unwrap(),
                media_descriptors,
            };
            let bytes = serde_json::to_vec(&envelope).unwrap();
            let sha256 = digest(&bytes);
            records.push(ObjectRef {
                sha256: sha256.clone(),
                bytes: bytes.len() as u64,
            });
            namespaces.insert(sha256.clone(), namespace);
            objects.insert(sha256, bytes);
        }
        records.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        let genesis = Uuid::new_v4();
        let initialized = first
            .initialize_selected(
                &scope,
                SelectionChange {
                    operation: genesis,
                    accepted_base_sha256: genesis_base(&root_sha, &scope.key_sha256),
                    selected: Manifest {
                        format: FORMAT,
                        transaction_id: genesis,
                        scope: Scope::SelectedRecords,
                        records,
                        record_namespaces: namespaces,
                        media,
                        media_coverage,
                    },
                    retained: vec![],
                },
                objects,
            )
            .unwrap();
        let memory = selected_memory::SelectedMemory::new(first.clone(), identity.clone()).unwrap();
        memory
            .initialize_selection(selected_memory::InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: root_sha,
                registrations: vec![catalog::Registration {
                    format: FORMAT,
                    catalog: identity.record_id().unwrap(),
                    key_sha256: scope.key_sha256.clone(),
                    owner,
                    genesis_operation: genesis,
                    genesis_sha256: initialized.token.selection_sha256().to_owned(),
                }],
            })
            .unwrap();
        if with_memory {
            let mut note = fact(profile);
            note.key.scope = memory::Scope::Document { document };
            note.claim = "preserve the source facts".into();
            note.value = "notebook fixture memory".into();
            memory
                .put(memory::Mutation {
                    operation: Uuid::new_v4(),
                    expected: Default::default(),
                    fact: note,
                })
                .unwrap();
        }
        let writer = SelectedAdmission::new(first.clone(), scope.clone()).unwrap();
        let source = NormalizedDocument {
            document,
            format: SourceFormat::Notebook,
            source_sha256: digest(b"original source"),
            extractor_revision: "selected-notebook-worker-v1".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "Force = 10 N. ".repeat(3000),
            }],
        };
        let raw = serde_json::to_vec_pretty(&source).unwrap();
        let source_path = fixture.0.join("source.json");
        fs::write(&source_path, &raw).unwrap();
        let config = fixture.0.join("config.json");
        let mut config_json =
            serde_json::json!({"paths":first.paths, "product_settings":{"writer_enabled":true}});
        if with_memory {
            config_json["memory"] =
                serde_json::json!({"enabled":true,"profile":profile,"selection":identity});
        }
        files::atomic_json(&config, &config_json).unwrap();
        let mut budget = SelectedReadBudget::new(64, 4096, 32 * 1024 * 1024, 32 * 1024 * 1024);
        let read = first
            .selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut budget)
            .unwrap()
            .unwrap();
        let operation = Uuid::new_v4();
        let request = serde_json::json!({
            "wire_version":1,
            "expected":{
                "selection":IntentSelectionEvidence::from_token(&read.snapshot.token),
                "raw_head_sha256":read.accepted_history[0].sha256,
            },
            "root":IntentRecordRef {
                namespace:Namespace::Conversation,record_id:conversation,
                revision_id:root.revision_id,object:root_ref,
            },
            "notebook":{
                "command":"generate-notebook","conversation":conversation,"operation":operation,
                "request":"Preserve the source facts.","document_file":source_path,
                "document_sha256":digest(&raw),"selection":{"scope":"supplied-notebook"},
                "guidance":null,"max_model_calls":3,"history_range":null,
            },
        });
        Self {
            fixture,
            wire,
            first,
            second,
            writer,
            identity,
            operation,
            conversation,
            config,
            request,
            frozen: crate::writer::notebook::ValidatedNotebook::from_bytes(&raw, &digest(&raw))
                .unwrap(),
        }
    }

    fn produce(&self, model: &mut Model<'_>) -> crate::writer::notebook::runner::Report {
        self.produce_with_fault(model, None)
    }

    fn produce_with_fault(
        &self,
        model: &mut Model<'_>,
        fault: Option<Fault>,
    ) -> crate::writer::notebook::runner::Report {
        let started = Instant::now();
        let preferences = RequestPreferences::notebook_from_binding(
            self.first.clone(),
            self.config.clone(),
            super::Binding {
                group: self.identity.group,
                root_id: "shared-root".into(),
                digest: self.identity.binding_sha256.clone(),
            },
            started,
            3,
            |store| {
                Ok(self.wire.client(
                    store.paths.credentials.join("drive.json"),
                    self.identity.group,
                ))
            },
        )
        .unwrap();
        let memory =
            preferences.config().memory.enabled.then(|| {
                memory::configured(self.first.clone(), &preferences.config().memory).unwrap()
            });
        if let Some(fault) = fault {
            self.first.set_fault(fault).unwrap();
        }
        run_fixture(
            &self.writer,
            &preferences,
            memory.as_deref(),
            model,
            self.request.clone(),
            Some(self.frozen.clone()),
            started,
        )
        .unwrap()
    }
}

fn step_until_observed(engine: &mut Engine, key: &str) -> Result<Status> {
    for _ in 0..12 {
        let result = engine.step()?;
        if result == Status::IncompleteMedia
            || engine
                .checkpoint
                .scopes
                .get(key)
                .is_some_and(|progress| progress.applied.is_some() && progress.receive.is_none())
        {
            return Ok(result);
        }
    }
    anyhow::bail!("notebook Worker did not reach an observed result within the fixture item bound")
}

fn remote_manifest(fixture: &NotebookFixture, original: &Commit, manifest: &Manifest, id: &str) {
    let bytes = serde_json::to_vec(manifest).unwrap();
    let mut commit = original.clone();
    commit.manifest = ObjectRef {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
    };
    commit.manifest_id = id.into();
    fixture.wire.object(id, fixture.identity.group, bytes);
    fixture.wire.slot(
        &SlotBinding {
            group: fixture.identity.group,
            scope_sha256: original.key.clone(),
            slot_id: original.slot.clone(),
        },
        &format!("{id}-descriptor"),
        serde_json::to_vec(&commit).unwrap(),
    );
}

fn receiving_variant(fixture: &NotebookFixture, name: &str) -> (Arc<Store>, Engine) {
    let store = fixture.fixture.store(name);
    let identity = &fixture.identity;
    let store = compound(store, identity.group, &identity.binding_sha256);
    choose(&store, identity.group, &identity.binding_sha256);
    let engine = fixture_engine(&fixture.wire, store.clone(), identity.group, 64);
    (store, engine)
}

fn omit_media(manifest: &mut Manifest, hash: &str) {
    manifest.media.retain(|reference| reference.sha256 != hash);
    let coverage = manifest
        .media_coverage
        .iter_mut()
        .find(|coverage| coverage.hash() == hash)
        .unwrap();
    *coverage = Coverage::Omitted {
        sha256: hash.into(),
        reason: crate::storage::Omission::UnavailableAtSource,
    };
}

fn refine_received(
    fixture: &NotebookFixture,
    store: Arc<Store>,
    received: &crate::storage::selection::SelectedSnapshot,
    draft: Uuid,
    model: &mut Model<'_>,
) -> Result<crate::writer::notebook::runner::Report> {
    let writer = SelectedAdmission::new(store.clone(), fixture.writer.scope().clone())?;
    let projection = SelectedDomainProjection::from_snapshot(received)?;
    let conversation = fixture.conversation;
    let draft = projection.current_reference(Namespace::Conversation, draft)?;
    let Record::Turn(turn) = projection.exact_reference(&draft)? else {
        anyhow::bail!("fixture received draft absent");
    };
    let source = projection.current_reference(Namespace::Source, turn.sources[0])?;
    let path = store
        .paths
        .data
        .parent()
        .unwrap()
        .join("receiving-config.json");
    files::atomic_json(
        &path,
        &serde_json::json!({"paths":store.paths,"product_settings":{"writer_enabled":true}}),
    )?;
    let started = Instant::now();
    let preferences = RequestPreferences::notebook_from_binding(
        store.clone(),
        path,
        super::Binding {
            group: fixture.identity.group,
            root_id: "shared-root".into(),
            digest: fixture.identity.binding_sha256.clone(),
        },
        started,
        3,
        |store| {
            Ok(fixture.wire.client(
                store.paths.credentials.join("drive.json"),
                fixture.identity.group,
            ))
        },
    )?;
    let request = serde_json::json!({
        "wire_version":1,
        "expected":{
            "selection":IntentSelectionEvidence::from_token(&received.token),
            "raw_head_sha256":received.token.selection_sha256(),
        },
        "root":projection.current_reference(Namespace::Conversation, conversation)?,
        "refine":{"source":source,"draft":draft},
        "notebook":{
            "command":"refine-notebook","conversation":conversation,"operation":Uuid::new_v4(),
            "request":"Preserve the source facts.","draft":draft.record_id,"max_model_calls":3,"history_range":null,
        },
    });
    run_fixture(&writer, &preferences, None, model, request, None, started)
}

fn reject_bad_notebook_variants(
    fixture: &NotebookFixture,
    original: &Commit,
    sent: &crate::storage::selection::SelectedSnapshot,
    draft: Uuid,
) {
    let scope = fixture.writer.scope();
    let key = &scope.key_sha256;
    let media = sent
        .selected_records
        .iter()
        .find_map(|envelope| match Ledger::decode(envelope).ok()? {
            Record::ArchivedNormalizedSource(source) => Some(source.media.sha256.clone()),
            _ => None,
        })
        .unwrap();

    let mut omitted = sent.transaction.selected.clone();
    omit_media(&mut omitted, &media);
    remote_manifest(fixture, original, &omitted, "missing-canonical-notebook");
    let (store, mut receiver) = receiving_variant(fixture, "required-media-receiver");
    assert_eq!(
        step_until_observed(&mut receiver, key).unwrap(),
        Status::IncompleteMedia
    );
    let progress = &receiver.checkpoint.scopes[key];
    assert!(progress.receive.is_some() && progress.applied.is_none());
    assert!(store.selected_snapshot(scope, MAX_ITEMS).unwrap().is_none());

    let mut optional = sent.transaction.selected.clone();
    let derivative = sent
        .selected_records
        .iter()
        .find_map(|envelope| match Ledger::decode(envelope).ok()? {
            Record::ResidentCapture(capture) => Some(capture.images[0].media.sha256.clone()),
            _ => None,
        })
        .unwrap();
    omit_media(&mut optional, &derivative);
    remote_manifest(fixture, original, &optional, "omitted-optional-image");
    let (store, mut receiver) = receiving_variant(fixture, "optional-media-receiver");
    assert_eq!(
        step_until_observed(&mut receiver, key).unwrap(),
        Status::IncompleteMedia
    );
    assert!(receiver.checkpoint.scopes[key].applied.is_some());
    assert_eq!(receiver.step().unwrap(), Status::IncompleteMedia);
    let adopted = store.selected_snapshot(scope, MAX_ITEMS).unwrap().unwrap();
    assert!(adopted
        .transaction
        .selected
        .media
        .iter()
        .any(|reference| reference.sha256 == media));
    assert!(!adopted
        .transaction
        .selected
        .media
        .iter()
        .any(|reference| reference.sha256 == derivative));

    let mut model = Model {
        calls: 0,
        parts: Vec::new(),
        images: Vec::new(),
        interlock: None,
        invalid_output: false,
        expected_memory: false,
    };
    let error = refine_received(fixture, store, &adopted, draft, &mut model)
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("historical evidence unavailable"),
        "{error:#}"
    );
    assert_eq!(model.calls, 0);

    let mut invalid = sent.transaction.selected.clone();
    let (mut bad, prior_hash) = sent
        .selected_records
        .iter()
        .zip(&invalid.records)
        .find_map(
            |(envelope, reference)| match Ledger::decode(envelope).ok()? {
                Record::NotebookStage(stage)
                    if stage.kind == NotebookStageKind::Reduce && stage.outcome.is_some() =>
                {
                    Some((envelope.clone(), reference.sha256.clone()))
                }
                _ => None,
            },
        )
        .unwrap();
    bad.payload["record"]["prepared_revision"] =
        serde_json::json!(digest(b"nonexistent notebook preparation"));
    let bad = serde_json::to_vec(&bad).unwrap();
    let bad_hash = digest(&bad);
    invalid
        .records
        .retain(|reference| reference.sha256 != prior_hash);
    invalid.record_namespaces.remove(&prior_hash);
    invalid.records.push(ObjectRef {
        sha256: bad_hash.clone(),
        bytes: bad.len() as u64,
    });
    invalid
        .records
        .sort_by(|left, right| left.sha256.cmp(&right.sha256));
    invalid
        .record_namespaces
        .insert(bad_hash, Namespace::Conversation);
    fixture
        .wire
        .object("bad-stage-object", fixture.identity.group, bad);
    remote_manifest(fixture, original, &invalid, "invalid-notebook-stage");
    let (store, mut receiver) = receiving_variant(fixture, "invalid-stage-receiver");
    let error = step_until_observed(&mut receiver, key)
        .unwrap_err()
        .to_string();
    assert!(
        error == "notebook revision is not committed or in this publication",
        "{error}"
    );
    assert!(receiver.checkpoint.scopes[key].receive.is_some());
    assert!(receiver.checkpoint.scopes[key].applied.is_none());
    assert!(store.selected_snapshot(scope, MAX_ITEMS).unwrap().is_none());
}

#[test]
fn selected_notebook_crosses_real_worker_with_exact_source_and_stage_objects() {
    let fixture = NotebookFixture::new(true);
    let mut model = Model {
        calls: 0,
        parts: Vec::new(),
        images: Vec::new(),
        interlock: None,
        invalid_output: false,
        expected_memory: true,
    };
    let report = fixture.produce(&mut model);
    assert_eq!(
        (report.status, report.failure),
        (NotebookStatus::Generated, None)
    );
    assert_eq!(
        (
            model.calls,
            report.local_invocations,
            report.confirmed_stage_results
        ),
        (3, 3, 3)
    );
    let scope = fixture.writer.scope();
    let sent = fixture
        .first
        .selected_snapshot(scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    let manifest = &sent.transaction.selected;
    assert_eq!(manifest.media.len(), 6);
    let key = &scope.key_sha256;
    let mut publisher = fixture_engine(
        &fixture.wire,
        fixture.first.clone(),
        fixture.identity.group,
        64,
    );
    assert_eq!(
        step_until_observed(&mut publisher, key).unwrap(),
        Status::Current
    );
    assert!(fixture
        .first
        .selected_attempt_inventory()
        .unwrap()
        .iter()
        .any(|attempt| attempt.state == SelectedAttemptState::Withdrawn));
    let recorded = &publisher.checkpoint.scopes[key].chain[0];
    let original: Commit = serde_json::from_slice(&recorded.bytes).unwrap();
    reject_bad_notebook_variants(&fixture, &original, &sent, report.draft.unwrap());
    fixture.wire.slot(
        &SlotBinding {
            group: fixture.identity.group,
            scope_sha256: key.clone(),
            slot_id: recorded.slot.clone(),
        },
        &recorded.descriptor_id,
        recorded.bytes.clone(),
    );
    let mut receiver = fixture_engine(
        &fixture.wire,
        fixture.second.clone(),
        fixture.identity.group,
        64,
    );
    assert_eq!(
        step_until_observed(&mut receiver, key).unwrap(),
        Status::Current
    );
    let received = fixture
        .second
        .selected_snapshot(scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    assert_eq!(manifest.records, received.transaction.selected.records);
    assert_eq!(manifest.media, received.transaction.selected.media);
    assert_eq!(received.token.accepted_base_sha256(), recorded.sha256);
    assert_ne!(
        sent.token.store_generation(),
        received.token.store_generation()
    );
    for object in manifest.records.iter().chain(&manifest.media) {
        assert_eq!(
            fixture.first.read_object(object).unwrap(),
            fixture.second.read_object(object).unwrap()
        );
    }
    let projection = SelectedDomainProjection::from_snapshot(&received).unwrap();
    let records = projection.inspect(fixture.conversation).unwrap();
    assert!(records.iter().any(|record| matches!(record, Record::Turn(turn)
        if Some(turn.id) == report.draft && turn.text.as_deref() == Some("# Exact note\n\nForce = 10 N."))));
    assert!(records
        .iter()
        .any(|record| matches!(record, Record::NotebookPlan(plan)
        if plan.id == NotebookPlan::identity(fixture.operation) && plan.chunks.len() == 2)));
    assert!(records.iter().any(|record| matches!(record, Record::NotebookStage(stage)
        if stage.kind == NotebookStageKind::Reduce && stage.outcome == Some(NotebookStageOutcome::Generated)
            && stage.predecessors.len() == 2 && stage.prepared_revision.is_some())));
    assert!(fixture
        .second
        .selected_attempt_inventory()
        .unwrap()
        .is_empty());
    let mut refine_model = Model {
        calls: 0,
        parts: Vec::new(),
        images: Vec::new(),
        interlock: None,
        invalid_output: false,
        expected_memory: false,
    };
    let refined = refine_received(
        &fixture,
        fixture.second.clone(),
        &received,
        report.draft.unwrap(),
        &mut refine_model,
    )
    .unwrap();
    assert_eq!(
        (
            refined.status,
            refined.confirmed_stage_results,
            refine_model.calls
        ),
        (NotebookStatus::Generated, 3, 3)
    );
    assert_ne!(refined.draft, report.draft);
}

#[test]
fn selected_notebook_refuses_late_result_after_winner_activation() {
    let fixture = NotebookFixture::new(false);
    let mut model = Model {
        calls: 0,
        parts: Vec::new(),
        images: Vec::new(),
        interlock: Some(Interlock {
            ordinal: 0,
            boundary: Boundary::AfterHandoff,
            change: Box::new(|| {
                let prior = fixture
                    .first
                    .selected_snapshot(fixture.writer.scope(), MAX_ITEMS)
                    .unwrap()
                    .unwrap();
                fixture
                    .first
                    .activate_selected(
                        &prior.token,
                        SelectionChange {
                            operation: Uuid::new_v4(),
                            accepted_base_sha256: digest(b"other winning descriptor"),
                            selected: prior.transaction.selected,
                            retained: prior.transaction.retained,
                        },
                        BTreeMap::new(),
                    )
                    .unwrap();
            }),
        }),
        invalid_output: false,
        expected_memory: false,
    };
    let report = fixture.produce(&mut model);
    assert_eq!(report.status, NotebookStatus::Stale);
    assert_eq!((model.calls, report.confirmed_stage_results), (1, 0));
    assert!(report.draft.is_none() && report.remote_cost_unknown);
    let current = fixture
        .first
        .selected_snapshot(fixture.writer.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let records = SelectedDomainProjection::from_snapshot(&current)
        .unwrap()
        .inspect(fixture.conversation)
        .unwrap();
    assert!(records
        .iter()
        .any(|record| matches!(record, Record::NotebookStage(stage) if stage.outcome.is_none())));
    assert!(!records
        .iter()
        .any(|record| matches!(record, Record::Turn(turn) if turn.outcome == Outcome::Generated)));
}

#[test]
fn selected_notebook_initial_ack_loss_is_uncertain_and_reopens_historical_without_model() {
    for fault in [Fault::AfterCommit, Fault::AfterActivation] {
        let fixture = NotebookFixture::new(false);
        let mut model = Model {
            calls: 0,
            parts: Vec::new(),
            images: Vec::new(),
            interlock: None,
            invalid_output: false,
            expected_memory: false,
        };
        let report = fixture.produce_with_fault(&mut model, Some(fault));
        assert_eq!(
            (report.status, report.failure),
            (
                NotebookStatus::Uncertain,
                Some(crate::writer::notebook::runner::Failure::Storage)
            )
        );
        assert!(report.unobserved_publication && !report.admitted && !report.remote_cost_unknown);
        assert_eq!((model.calls, report.local_invocations), (0, 0));
        assert!(fixture
            .first
            .selected_receipt(fixture.writer.scope(), fixture.operation)
            .unwrap()
            .is_some());
        let NotebookFixture {
            fixture,
            first,
            writer,
            config,
            request,
            ..
        } = fixture;
        drop(writer);
        drop(first);
        let bytes = serde_json::to_vec(&request).unwrap();
        let replay = crate::writer::command::dispatch(Some(config.clone()), &mut bytes.as_slice());
        let replay = serde_json::to_value(replay).unwrap();
        assert_eq!(replay["status"], "historical");
        assert_eq!(replay["local_invocations"], 0);
        assert!(replay["draft"].is_null());
        let mut changed = request;
        changed["notebook"]["request"] = serde_json::json!("Changed request.");
        let bytes = serde_json::to_vec(&changed).unwrap();
        let refusal = crate::writer::command::dispatch(Some(config), &mut bytes.as_slice());
        assert_eq!(serde_json::to_value(refusal).unwrap()["status"], "refused");
        drop(fixture);
    }
}

#[test]
fn selected_notebook_invalid_map_is_durable_and_never_reduces() {
    let fixture = NotebookFixture::new(false);
    let mut model = Model {
        calls: 0,
        parts: Vec::new(),
        images: Vec::new(),
        interlock: None,
        invalid_output: true,
        expected_memory: false,
    };
    let report = fixture.produce(&mut model);
    assert_eq!(report.status, NotebookStatus::Failed);
    assert_eq!((model.calls, report.confirmed_stage_results), (1, 1));
    assert!(report.draft.is_none());
    let current = fixture
        .first
        .selected_snapshot(fixture.writer.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let records = SelectedDomainProjection::from_snapshot(&current)
        .unwrap()
        .inspect(fixture.conversation)
        .unwrap();
    assert!(records.iter().any(|record| matches!(record, Record::NotebookStage(stage)
        if stage.kind == NotebookStageKind::Map && stage.outcome == Some(NotebookStageOutcome::Failed))));
    assert!(!records.iter().any(|record| matches!(record, Record::NotebookStage(stage) if stage.kind == NotebookStageKind::Reduce)));
}

fn assert_interlocked_maps(fixture: &NotebookFixture) -> crate::storage::selection::SelectionToken {
    let snapshot = fixture
        .first
        .selected_snapshot(fixture.writer.scope(), MAX_ITEMS)
        .unwrap()
        .unwrap();
    let records = SelectedDomainProjection::from_snapshot(&snapshot)
        .unwrap()
        .inspect(fixture.conversation)
        .unwrap();
    let mut stages: Vec<_> = records
        .into_iter()
        .filter_map(|record| match record {
            Record::NotebookStage(stage) => Some((stage.kind, stage.ordinal, stage.outcome)),
            Record::Turn(turn) => {
                assert_ne!(turn.outcome, Outcome::Generated);
                None
            }
            _ => None,
        })
        .collect();
    stages.sort_by_key(|(_, ordinal, _)| *ordinal);
    assert_eq!(
        stages,
        [
            (
                NotebookStageKind::Map,
                0,
                Some(NotebookStageOutcome::Generated)
            ),
            (NotebookStageKind::Map, 1, None),
        ]
    );
    snapshot.token
}

#[test]
fn selected_notebook_interlock_refuses_next_handoff_after_shared_preference_edit() {
    let fixture = NotebookFixture::new(false);
    let binding = super::Binding {
        group: fixture.identity.group,
        root_id: "shared-root".into(),
        digest: fixture.identity.binding_sha256.clone(),
    };
    let edit = |enabled| {
        preference_command::set_fixture_preference(
            fixture.first.clone(),
            &fixture.config,
            binding.clone(),
            &fixture.wire,
            serde_json::json!({"product_settings": {"writer_enabled": enabled}}),
        )
        .unwrap();
    };
    edit(true);
    let mut model = Model {
        interlock: Some(Interlock {
            ordinal: 1,
            boundary: Boundary::BeforeHandoff,
            change: Box::new(|| {
                let token = assert_interlocked_maps(&fixture);
                edit(false);
                assert_eq!(assert_interlocked_maps(&fixture), token);
            }),
        }),
        ..Default::default()
    };
    let report = fixture.produce(&mut model);
    assert!(
        model.interlock.is_none(),
        "second Map never reached handoff"
    );
    assert_eq!(
        (report.status, report.confirmed_stage_results),
        (NotebookStatus::Failed, 1)
    );
    assert_eq!((model.calls, report.local_invocations), (1, 1));
    assert!(report.draft.is_none() && !report.unobserved_publication);
    assert_interlocked_maps(&fixture);
}

#[test]
fn selected_notebook_interlock_refuses_next_result_after_independent_memory_edit() {
    let fixture = NotebookFixture::new(true);
    let memory =
        selected_memory::SelectedMemory::new(fixture.first.clone(), fixture.identity.clone())
            .unwrap();
    let mut global = fact(fixture.identity.profile);
    global.claim = "preserve the source facts globally".into();
    let original = memory
        .initialize_aggregate(selected_memory::InitializeAggregate {
            operation: Uuid::new_v4(),
            accepted_base_sha256: digest(b"independent notebook memory genesis"),
            fact: global.clone(),
        })
        .unwrap()
        .registration
        .unwrap();
    memory
        .register_selection(selected_memory::RegisterSelection {
            operation: Uuid::new_v4(),
            registration: original,
        })
        .unwrap();

    let mut model = Model {
        interlock: Some(Interlock {
            ordinal: 1,
            boundary: Boundary::AfterHandoff,
            change: Box::new(|| {
                let document = assert_interlocked_maps(&fixture);
                let mut updated = global.clone();
                updated.value = "independently edited after the second handoff".into();
                let expected = memory
                    .inspect(&updated.key)
                    .unwrap()
                    .into_iter()
                    .map(|record| record.revision_id)
                    .collect();
                memory
                    .put(memory::Mutation {
                        operation: Uuid::new_v4(),
                        expected,
                        fact: updated,
                    })
                    .unwrap();
                assert_eq!(assert_interlocked_maps(&fixture), document);
            }),
        }),
        expected_memory: true,
        ..Default::default()
    };
    let report = fixture.produce(&mut model);
    assert!(model.interlock.is_none(), "second Map never submitted");
    assert_eq!(
        (report.status, report.confirmed_stage_results),
        (NotebookStatus::Failed, 1)
    );
    assert_eq!((model.calls, report.local_invocations), (2, 2));
    assert!(report.draft.is_none() && !report.unobserved_publication);
    assert_interlocked_maps(&fixture);
}
