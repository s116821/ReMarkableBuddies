use super::*;

struct RecoveryFixture {
    _paths: Fixture,
    wire: Wire,
    publisher: Arc<Store>,
    receiver: Arc<Store>,
    identity: catalog::Identity,
    scope: SelectionScope,
    key: memory::Key,
}

impl RecoveryFixture {
    fn new() -> Self {
        let paths = Fixture::new();
        let wire = Wire::new();
        let publisher = paths.store("publisher");
        let receiver = paths.store("receiver");
        let group = Uuid::new_v4();
        let profile = Uuid::new_v4();
        let binding = wire
            .client(publisher.paths.credentials.join("drive.json"), group)
            .binding_digest();
        let publisher = compound(publisher, group, &binding);
        let receiver = compound(receiver, group, &binding);
        choose(&publisher, group, &binding);
        choose(&receiver, group, &binding);
        let identity = catalog::Identity {
            group,
            profile,
            binding_sha256: binding.clone(),
        };
        let initial = fact(profile);
        let owner = catalog::Owner::GlobalFact {
            key: initial.key.clone(),
        };
        let scope = identity.aggregate_scope(&owner).unwrap();
        let root = existing_group(&wire, group, binding, profile, &scope, owner);
        let producer =
            selected_memory::SelectedMemory::new(publisher.clone(), identity.clone()).unwrap();
        let key = initial.key.clone();
        let registration = producer
            .initialize_aggregate(selected_memory::InitializeAggregate {
                operation: Uuid::new_v4(),
                accepted_base_sha256: genesis_base(&root, &scope.key_sha256),
                fact: initial,
            })
            .unwrap()
            .registration
            .unwrap();
        producer
            .initialize_selection(selected_memory::InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: root,
                registrations: vec![registration],
            })
            .unwrap();
        Self {
            _paths: paths,
            wire,
            publisher,
            receiver,
            identity,
            scope,
            key,
        }
    }

    fn reopen_publisher(&mut self) {
        let old = std::mem::replace(&mut self.publisher, self.receiver.clone());
        let paths = old.paths.clone();
        drop(old);
        self.publisher = Arc::new(Store::open_existing_for_mode(paths).unwrap());
    }

    fn interrupt_before_create(&self) -> Pending {
        let mut engine =
            fixture_engine(&self.wire, self.publisher.clone(), self.identity.group, 32);
        self.publisher.set_fault(Fault::AfterCommit).unwrap();
        for _ in 0..12 {
            if let Err(error) = engine.step() {
                let pending = engine.checkpoint.scopes[&self.scope.key_sha256]
                    .pending
                    .clone()
                    .unwrap();
                assert!(pending.outbox_prepared.is_none(), "{error:#}");
                assert!(
                    self.publisher
                        .inspect_selected_attempt(pending.operation)
                        .unwrap()
                        .is_some(),
                    "{error:#}"
                );
                assert!(self.wire.slot_posts().is_empty());
                return pending;
            }
        }
        panic!("outbox crash did not happen before create");
    }

    fn edit(&self) {
        let producer =
            selected_memory::SelectedMemory::new(self.publisher.clone(), self.identity.clone())
                .unwrap();
        let current = producer.inspect(&self.key).unwrap();
        let mut edited: memory::Fact = serde_json::from_value(current[0].payload.clone()).unwrap();
        edited.value = "new local edit after original preparation".into();
        producer
            .put(memory::Mutation {
                operation: Uuid::new_v4(),
                expected: current
                    .into_iter()
                    .map(|envelope| envelope.revision_id)
                    .collect(),
                fact: edited,
            })
            .unwrap();
    }
}

fn drive_applied(engine: &mut Engine, key: &str) {
    for _ in 0..12 {
        engine.step().unwrap();
        if engine
            .checkpoint
            .scopes
            .get(key)
            .is_some_and(|progress| progress.applied.is_some() && progress.receive.is_none())
        {
            return;
        }
    }
    panic!("verified Worker receipt was not applied");
}

#[test]
fn selected_worker_recovery_restores_and_retries_exact_current_original() {
    let mut fixture = RecoveryFixture::new();
    let pending = fixture.interrupt_before_create();
    let staged = pending.descriptor.clone().unwrap();
    let original = fixture
        .publisher
        .inspect_selected_attempt(pending.operation)
        .unwrap()
        .unwrap();
    let key = fixture.scope.key_sha256.clone();
    // Actual Store reopen after outbox publication but before the first create.
    fixture.reopen_publisher();
    fixture.wire.remove_fixture_object(&staged.descriptor_id);
    let before = fixture.wire.requests().len();
    let mut engine = fixture_engine(
        &fixture.wire,
        fixture.publisher.clone(),
        fixture.identity.group,
        32,
    );
    assert_eq!(engine.step().unwrap(), Status::Pending);
    let requests = fixture.wire.requests();
    assert!(requests[before..]
        .iter()
        .any(|request| request.starts_with("POST /upload/")));
    assert!(requests[before..]
        .iter()
        .all(|request| !request.contains("generateIds")));
    assert!(fixture.wire.slot_posts().is_empty());
    let restored = engine.checkpoint.scopes[&key].pending.as_ref().unwrap();
    assert_eq!(restored.operation, pending.operation);
    assert_eq!(restored.descriptor.as_ref().unwrap().bytes, staged.bytes);
    assert_eq!(
        restored.descriptor.as_ref().unwrap().descriptor_id,
        staged.descriptor_id
    );

    // The create wins, but the exact-slot observation is temporarily unavailable.
    // No preliminary missing read may consume this injected fault before POST.
    fixture.wire.fail_get_once(&pending.slot);
    assert!(engine.step().is_err());
    assert_eq!(
        fixture.wire.slot_posts(),
        std::slice::from_ref(&pending.slot)
    );
    assert!(engine.checkpoint.scopes[&key].receive.is_none());
    drop(engine);
    fixture.reopen_publisher();
    let mut engine = fixture_engine(
        &fixture.wire,
        fixture.publisher.clone(),
        fixture.identity.group,
        32,
    );
    drive_applied(&mut engine, &key);
    assert_eq!(
        fixture.wire.slot_posts(),
        [pending.slot.clone(), pending.slot]
    );
    let retired = fixture
        .publisher
        .inspect_selected_attempt(pending.operation)
        .unwrap()
        .unwrap();
    assert_eq!(retired.request, original.request);
    assert_eq!(retired.state, SelectedAttemptState::Withdrawn);
    let mut passive = fixture_engine(
        &fixture.wire,
        fixture.receiver.clone(),
        fixture.identity.group,
        32,
    );
    drive_applied(&mut passive, &key);
    let health = passive.health(Status::Current);
    let scope = &health["scopes"][0];
    assert_eq!(scope["committed"], staged.sha256);
    assert_eq!(scope["applied"], staged.sha256);
    assert_ne!(
        scope["applied_local"]["store_generation"],
        serde_json::json!(original.selection.store_generation)
    );
    assert!(scope["pending_original"].is_null());
    assert!(fixture
        .receiver
        .selected_attempt_inventory()
        .unwrap()
        .is_empty());
}

#[test]
fn selected_worker_recovery_stale_original_observes_only_and_old_epoch_stops() {
    let fixture = RecoveryFixture::new();
    let pending = fixture.interrupt_before_create();
    fixture.edit();
    let before = fixture.wire.requests().len();
    let mut engine = fixture_engine(
        &fixture.wire,
        fixture.publisher.clone(),
        fixture.identity.group,
        32,
    );
    assert_eq!(engine.step().unwrap(), Status::Pending);
    let requests = fixture.wire.requests();
    assert!(requests[before..]
        .iter()
        .any(|request| request.starts_with("GET /drive/v3/files/fact-first?")));
    assert!(requests[before..]
        .iter()
        .all(|request| request.starts_with("GET ") && !request.contains("generateIds")));
    let progress = &engine.checkpoint.scopes[&fixture.scope.key_sha256];
    assert_eq!(
        progress.pending.as_ref().unwrap().operation,
        pending.operation
    );
    assert!(progress.receive.is_none());
    assert_eq!(
        fixture
            .publisher
            .inspect_selected_attempt(pending.operation)
            .unwrap()
            .unwrap()
            .state,
        SelectedAttemptState::Pending
    );

    fixture.wire.object(
        &pending.slot,
        fixture.identity.group,
        b"occupied by non-pointer data".to_vec(),
    );
    let before = fixture.wire.slot_posts();
    assert_eq!(engine.step().unwrap(), Status::UnsupportedOrCorrupt);
    assert_eq!(
        engine.health(Status::UnsupportedOrCorrupt)["status"],
        "unsupported-or-corrupt"
    );
    assert_eq!(
        engine.checkpoint.scopes[&fixture.scope.key_sha256]
            .pending
            .as_ref()
            .unwrap()
            .operation,
        pending.operation
    );
    assert_eq!(fixture.wire.slot_posts(), before);

    fixture
        .publisher
        .set_sync_mode(
            &fixture.publisher.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    let before = fixture.wire.requests().len();
    assert!(engine.step().is_err());
    assert_eq!(fixture.wire.requests().len(), before);
}

fn register_document(
    store: Arc<Store>,
    identity: &catalog::Identity,
    owner: catalog::Owner,
    root: &str,
) {
    let scope = identity.aggregate_scope(&owner).unwrap();
    let mut reads = crate::storage::selection::SelectedReadBudget::new(
        16,
        MAX_ITEMS as u64,
        MAX_METADATA as u64,
        MAX_MEDIA,
    );
    let read = store
        .selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut reads)
        .unwrap()
        .unwrap();
    let genesis = read.accepted_history.last().unwrap();
    selected_memory::SelectedMemory::new(store, identity.clone())
        .unwrap()
        .initialize_selection(selected_memory::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: root.into(),
            registrations: vec![catalog::Registration {
                format: FORMAT,
                catalog: identity.record_id().unwrap(),
                key_sha256: scope.key_sha256,
                owner,
                genesis_operation: genesis.transaction.operation,
                genesis_sha256: genesis.sha256.clone(),
            }],
        })
        .unwrap();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WinnerCase {
    Own,
    CompetingOverlap,
    CompetingExclusiveRetention,
}

fn writer_request(
    snapshot: &crate::storage::selection::SelectedSnapshot,
    document: Uuid,
) -> crate::conversation::SelectedWriterPrepare {
    use crate::{
        conversation::*,
        retrieval::{NormalizedDocument, SourceFormat, SourceUnit},
    };
    let projection = SelectedDomainProjection::from_snapshot(snapshot).unwrap();
    let conversation = snapshot
        .selected_records
        .iter()
        .find_map(|envelope| match Ledger::decode(envelope).ok()? {
            Record::Root(root) => Some(root.id),
            _ => None,
        })
        .unwrap();
    let operation = Uuid::new_v4();
    let normalized = NormalizedTextSource::new(
        conversation,
        crate::writer::stage(operation, "user"),
        NormalizedDocument {
            document,
            format: SourceFormat::Notebook,
            source_sha256: digest(b"document scope Writer probe"),
            extractor_revision: "worker-custody-v1".into(),
            units: vec![SourceUnit {
                page: Some(1),
                section: None,
                text: "Preserve source custody.".into(),
            }],
        },
        TextScope::SuppliedNotebook,
        None,
    )
    .unwrap();
    SelectedWriterPrepare {
        operation,
        root: projection
            .current_reference(Namespace::Conversation, conversation)
            .unwrap(),
        request: "Preserve source custody.".into(),
        source: SelectedWriterSource::Generate { normalized },
    }
}

fn check_writer_fence(store: Arc<Store>, scope: &SelectionScope, document: Uuid) -> Result<()> {
    let snapshot = store.selected_snapshot(scope, MAX_ITEMS)?.unwrap();
    let admission = SelectedAdmission::new(store, scope.clone())?;
    admission.selected_writer_input(&snapshot.token, &writer_request(&snapshot, document))?;
    Ok(())
}

fn retain_foreign_sibling(
    original: &Store,
    receiver: &Store,
    scope: &SelectionScope,
    operation: Uuid,
) {
    use crate::{
        conversation::{Ledger, Record},
        storage::selection::SelectionChange,
    };
    let current = receiver
        .selected_snapshot(scope, MAX_ITEMS)
        .unwrap()
        .unwrap();
    let mut sibling = original
        .selected_predecessor(scope, operation)
        .unwrap()
        .unwrap()
        .selected;
    let mut alternate = sibling
        .records
        .iter()
        .find_map(|reference| {
            let bytes = original.read_object(reference).ok()?;
            let envelope: Envelope = serde_json::from_slice(&bytes).ok()?;
            matches!(Ledger::decode(&envelope), Ok(Record::Root(_))).then_some(envelope)
        })
        .unwrap();
    let root = alternate.revision_id;
    let active = current
        .selected_records
        .iter()
        .find(|record| record.record_id == alternate.record_id && record.parents.contains(&root))
        .unwrap();
    alternate.parents = BTreeSet::from([root]);
    alternate.revision_id = Uuid::new_v4();
    alternate.operation_id = Uuid::new_v4();
    alternate.actor_id = receiver.actor_id;
    assert_eq!(active.parents, alternate.parents);
    assert_ne!(active.revision_id, alternate.revision_id);
    let bytes = serde_json::to_vec(&alternate).unwrap();
    let hash = digest(&bytes);
    sibling.transaction_id = Uuid::new_v4();
    sibling.records.push(ObjectRef {
        sha256: hash.clone(),
        bytes: bytes.len() as u64,
    });
    sibling
        .record_namespaces
        .insert(hash.clone(), Namespace::Conversation);
    receiver
        .activate_selected(
            &current.token,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: current.token.accepted_base_sha256().into(),
                selected: current.transaction.selected,
                retained: vec![sibling],
            },
            BTreeMap::from([(hash, bytes)]),
        )
        .unwrap();
}

fn assert_missing_local_lineage(
    paths: &Fixture,
    origin: &Store,
    identity: &catalog::Identity,
    scope: &SelectionScope,
    document: Uuid,
    operation: Uuid,
) {
    use crate::{
        conversation::{Ledger, Record},
        storage::selection::SelectionChange,
    };
    let receiver = compound(
        paths.store("unproven"),
        identity.group,
        &identity.binding_sha256,
    );
    choose(&receiver, identity.group, &identity.binding_sha256);
    let snapshot = origin.selected_snapshot(scope, MAX_ITEMS).unwrap().unwrap();
    let mut manifest = snapshot.transaction.selected;
    let mut objects = BTreeMap::new();
    for (reference, envelope) in manifest.records.iter_mut().zip(snapshot.selected_records) {
        let mut envelope = envelope;
        match Ledger::decode(&envelope).unwrap() {
            Record::Receipt(receipt) if receipt.acknowledgment.operation == operation => (),
            Record::OutcomeFact(fact)
                if fact.reason == crate::conversation::AttemptReason::OutputPending => {}
            _ => {
                objects.insert(
                    reference.sha256.clone(),
                    origin.read_object(reference).unwrap(),
                );
                continue;
            }
        }
        // Local actor attribution without a matching original Commit is
        // ambiguous even though the enclosing domain graph is well formed.
        envelope.actor_id = receiver.actor_id;
        let bytes = serde_json::to_vec(&envelope).unwrap();
        manifest.record_namespaces.remove(&reference.sha256);
        reference.sha256 = digest(&bytes);
        reference.bytes = bytes.len() as u64;
        manifest
            .record_namespaces
            .insert(reference.sha256.clone(), Namespace::Conversation);
        objects.insert(reference.sha256.clone(), bytes);
    }
    receiver
        .initialize_selected(
            scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: snapshot.token.accepted_base_sha256().into(),
                selected: manifest,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
    let error = check_writer_fence(receiver, scope, document).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("uncertainty original local publication absent"),
        "{error:#}"
    );
}

fn worker_unresolved_winner(case: WinnerCase) {
    use crate::{
        conversation::{fixture_pending_for_worker, IntentUncertainty},
        storage::selection::SelectionChange,
    };
    let paths = Fixture::new();
    let wire = Wire::new();
    let a = paths.store("original");
    let b = paths.store("peer");
    let group = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let binding = wire
        .client(a.paths.credentials.join("drive.json"), group)
        .binding_digest();
    let a = compound(a, group, &binding);
    let b = compound(b, group, &binding);
    choose(&a, group, &binding);
    choose(&b, group, &binding);
    let identity = catalog::Identity {
        group,
        profile,
        binding_sha256: binding.clone(),
    };
    let document = Uuid::new_v4();
    let owner = catalog::Owner::Document { document };
    let scope = identity.aggregate_scope(&owner).unwrap();
    let key = &scope.key_sha256;
    let root = existing_group(&wire, group, binding, profile, &scope, owner.clone());
    let base = genesis_base(&root, key);
    let (_, operation) =
        fixture_pending_for_worker(a.clone(), scope.clone(), document, base.clone());
    register_document(a.clone(), &identity, owner.clone(), &root);
    let original = a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
    let receipt = original
        .selected_records
        .iter()
        .zip(&original.transaction.selected.records)
        .find_map(|(envelope, reference)| {
            match crate::conversation::Ledger::decode(envelope).ok()? {
                crate::conversation::Record::Receipt(receipt)
                    if receipt.acknowledgment.operation == operation =>
                {
                    Some(reference.clone())
                }
                _ => None,
            }
        })
        .unwrap();
    let original_bytes = a.read_object(&receipt).unwrap();

    if case != WinnerCase::Own {
        let mut reads = crate::storage::selection::SelectedReadBudget::new(
            16,
            MAX_ITEMS as u64,
            MAX_METADATA as u64,
            MAX_MEDIA,
        );
        let read = a
            .selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut reads)
            .unwrap()
            .unwrap();
        let input = if case == WinnerCase::CompetingOverlap {
            read.snapshot.transaction.selected
        } else {
            read.accepted_history
                .last()
                .unwrap()
                .transaction
                .selected
                .clone()
        };
        let objects = input
            .records
            .iter()
            .chain(&input.media)
            .map(|r| (r.sha256.clone(), a.read_object(r).unwrap()))
            .collect();
        b.initialize_selected(
            &scope,
            SelectionChange {
                operation: Uuid::new_v4(),
                accepted_base_sha256: base,
                selected: input,
                retained: vec![],
            },
            objects,
        )
        .unwrap();
        register_document(b.clone(), &identity, owner, &root);
        if case == WinnerCase::CompetingOverlap {
            let error = check_writer_fence(b.clone(), &scope, document).unwrap_err();
            assert!(
                error.to_string().contains("verified Worker custody"),
                "{error:#}"
            );
        } else {
            // The competing Writer creates a real root revision on the genesis
            // branch. The retained pending root is its sibling, not a second head.
            let snapshot = b.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
            SelectedAdmission::new(b.clone(), scope.clone())
                .unwrap()
                .prepare_selected_writer(&snapshot.token, writer_request(&snapshot, document), 4)
                .unwrap();
        }
        let producer = selected_memory::SelectedMemory::new(b.clone(), identity.clone()).unwrap();
        let mut note = fact(profile);
        note.key.scope = memory::Scope::Document { document };
        producer
            .put(memory::Mutation {
                operation: Uuid::new_v4(),
                expected: Default::default(),
                fact: note,
            })
            .unwrap();
        let mut peer = fixture_engine(&wire, b.clone(), group, 32);
        drive_applied(&mut peer, key);
        assert_eq!(wire.slot_posts(), ["fact-first"]);
    }

    let mut engine = fixture_engine(&wire, a.clone(), group, 32);
    if case == WinnerCase::CompetingExclusiveRetention {
        drive_applied(&mut engine, key);
        let current = a.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        assert!(!current.transaction.selected.records.contains(&receipt));
        assert!(current
            .transaction
            .retained
            .iter()
            .any(|manifest| manifest.records.contains(&receipt)));
        assert_eq!(a.read_object(&receipt).unwrap(), original_bytes);
        let error = check_writer_fence(a.clone(), &scope, document).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("earlier retained pending intent unresolved"),
            "{error:#}"
        );
        let admission = SelectedAdmission::new(a.clone(), scope.clone()).unwrap();
        assert_eq!(
            admission.retained_intent_uncertainty(operation).unwrap(),
            IntentUncertainty::Unresolved
        );
        assert_eq!(wire.slot_posts(), ["fact-first"]);
        return;
    }

    let mut blocked = false;
    for _ in 0..12 {
        if engine.step().unwrap() == Status::CommittedNotApplied {
            blocked = true;
            break;
        }
    }
    assert!(blocked);
    let observed = engine.checkpoint.scopes[key].receive.as_ref().unwrap();
    assert!(observed.verified && !observed.applied);
    let winner: Commit = serde_json::from_slice(&observed.descriptor.bytes).unwrap();
    assert_eq!(
        winner.actor,
        if case == WinnerCase::Own {
            a.actor_id
        } else {
            b.actor_id
        }
    );
    let receive = observed.operation;
    let descriptor = observed.descriptor.sha256.clone();
    assert!(a.selected_receipt(&scope, receive).unwrap().is_none());
    assert_eq!(
        a.selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .token,
        original.token
    );
    assert_eq!(a.read_object(&receipt).unwrap(), original_bytes);
    let health = engine.health(Status::Pending);
    assert_eq!(health["status"], "committed-not-applied");
    assert_eq!(health["scopes"][0]["committed"], descriptor);
    assert!(health["scopes"][0]["applied"].is_null());
    assert_eq!(
        health["scopes"][0]["receiving_original"],
        serde_json::json!(receive)
    );
    drop(engine);
    let mut resumed = fixture_engine(&wire, a.clone(), group, 32);
    let before = wire.slot_posts();
    assert_eq!(resumed.step().unwrap(), Status::CommittedNotApplied);
    let error = check_writer_fence(a.clone(), &scope, document).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("earlier active pending intent unresolved"),
        "{error:#}"
    );
    assert_eq!(
        resumed.checkpoint.scopes[key]
            .receive
            .as_ref()
            .unwrap()
            .operation,
        receive
    );
    assert_eq!(wire.slot_posts(), before);
    if case == WinnerCase::Own {
        let saved = resumed.checkpoint.scopes[key].receive.as_ref().unwrap();
        let manifest = saved.manifest.as_ref().unwrap();
        let raw = serde_json::to_vec_pretty(manifest).unwrap();
        let canonical = serde_json::to_vec(manifest).unwrap();
        assert_ne!(digest(&raw), digest(&canonical));
        let mut imported = winner;
        imported.manifest_id = "pretty-manifest".into();
        imported.manifest = ObjectRef {
            sha256: digest(&raw),
            bytes: raw.len() as u64,
        };
        wire.object(&imported.manifest_id, group, raw.clone());
        wire.slot(
            &SlotBinding {
                group,
                scope_sha256: key.clone(),
                slot_id: imported.slot.clone(),
            },
            "pretty-manifest-descriptor",
            serde_json::to_vec(&imported).unwrap(),
        );
        let mut peer = fixture_engine(&wire, b.clone(), group, 32);
        drive_applied(&mut peer, key);
        assert!(b
            .selected_snapshot(&scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
            .transaction
            .selected
            .records
            .contains(&receipt));
        assert_eq!(b.read_object(&receipt).unwrap(), original_bytes);
        retain_foreign_sibling(&a, &b, &scope, operation);
        let receipt = peer.checkpoint.scopes[key].receipts.last().unwrap();
        assert_eq!(
            receipt.manifest_reference.as_ref(),
            Some(&imported.manifest)
        );
        let path = b
            .paths
            .data
            .join("sync/incoming")
            .join(peer.checkpoint.epoch.to_string())
            .join("manifest-custody")
            .join(key)
            .join(&imported.manifest.sha256);
        assert_eq!(files::read(&path, imported.manifest.bytes).unwrap(), raw);
        let requests = wire.requests().len();
        check_writer_fence(b.clone(), &scope, document).unwrap();

        // Change only whitespace at the same length and leave the parsed value
        // identical. The descriptor must still reject these substituted bytes.
        let mut substituted = raw.clone();
        let offset = substituted.iter().position(|byte| *byte == b' ').unwrap();
        substituted[offset] = b'\t';
        assert_eq!(
            serde_json::from_slice::<Manifest>(&substituted).unwrap(),
            *receipt.manifest.as_ref().unwrap()
        );
        files::atomic(&path, &substituted).unwrap();
        let error = check_writer_fence(b.clone(), &scope, document).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("manifest does not match the original descriptor"),
            "{error:#}"
        );
        files::atomic(&path, &raw).unwrap();

        // A receipt with the right immutable bytes cannot substitute the saved
        // parsed manifest, even where its membership would still match.
        let receipt = peer
            .checkpoint
            .scopes
            .get_mut(key)
            .unwrap()
            .receipts
            .last_mut()
            .unwrap();
        let manifest_id = receipt.manifest.as_ref().unwrap().transaction_id;
        receipt.manifest.as_mut().unwrap().transaction_id = Uuid::new_v4();
        peer.save().unwrap();
        let error = check_writer_fence(b.clone(), &scope, document).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("original manifest differs from saved parsed correspondence"),
            "{error:#}"
        );
        peer.checkpoint
            .scopes
            .get_mut(key)
            .unwrap()
            .receipts
            .last_mut()
            .unwrap()
            .manifest
            .as_mut()
            .unwrap()
            .transaction_id = manifest_id;
        peer.save().unwrap();
        check_writer_fence(b.clone(), &scope, document).unwrap();

        // An older checkpoint can still prove a noncanonical original if its
        // incoming cache survived. Without those bytes it must fail closed.
        let receipt = peer
            .checkpoint
            .scopes
            .get_mut(key)
            .unwrap()
            .receipts
            .last_mut()
            .unwrap();
        receipt.manifest_reference = None;
        peer.save().unwrap();
        assert!(check_writer_fence(b.clone(), &scope, document).is_err());
        let incoming = b
            .paths
            .data
            .join("sync/incoming")
            .join(peer.checkpoint.epoch.to_string())
            .join(key)
            .join(&imported.manifest.sha256);
        files::object(&incoming, &imported.manifest.sha256, &raw).unwrap();
        check_writer_fence(b.clone(), &scope, document).unwrap();
        assert_eq!(wire.requests().len(), requests);
        assert!(SelectedAdmission::new(b.clone(), scope.clone())
            .unwrap()
            .recover_original_intent(operation)
            .unwrap()
            .is_none());
        assert_missing_local_lineage(&paths, &a, &identity, &scope, document, operation);
        assert!(b.selected_attempt_inventory().unwrap().is_empty());
    }
}

#[test]
fn selected_worker_recovery_own_unresolved_winner_is_committed_not_applied() {
    worker_unresolved_winner(WinnerCase::Own);
}

#[test]
fn selected_worker_recovery_competing_unresolved_winner_is_committed_not_applied() {
    worker_unresolved_winner(WinnerCase::CompetingOverlap);
}

#[test]
fn selected_worker_recovery_competing_winner_keeps_exclusive_retention() {
    worker_unresolved_winner(WinnerCase::CompetingExclusiveRetention);
}
