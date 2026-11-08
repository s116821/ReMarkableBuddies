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
        let seed_owner = catalog::Owner::Document {
            document: Uuid::new_v4(),
        };
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
