use remarkable_reader_buddy::{conversation::*, storage::*};
use std::{io::Cursor, path::PathBuf, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-legacy-{}", Uuid::new_v4())))
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(
            Store::open(StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("secrets"),
            })
            .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn input(value: u8, role: LegacyImageRole, ordinal: Option<u32>) -> LegacyImageInput {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        3,
        image::Rgba([value, 0, 0, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let bytes = bytes.into_inner();
    LegacyImageInput {
        descriptor: LegacyImage {
            media: Media {
                sha256: digest(&bytes),
                bytes: bytes.len() as u64,
                media_type: "image/png".into(),
            },
            dimensions: [2, 3],
            role,
            provider_ordinal: ordinal,
        },
        bytes,
    }
}
fn turn(id: Uuid) -> Turn {
    Turn {
        id: Uuid::new_v4(),
        conversation: id,
        exchange: Uuid::new_v4(),
        sequence: 0,
        role: Role::User,
        mode: Mode::Reader,
        outcome: Outcome::Prepared,
        text: None,
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    }
}
fn heads(ledger: &Ledger, id: Uuid) -> ExpectedHeads {
    ledger.expected(Namespace::Conversation, id).unwrap()
}

#[test]
fn exact_unbound_bytes_survive_reopen_retry_context_and_delete() {
    let f = Fixture::new();
    let store = f.open();
    let ledger = Ledger::new(store.clone());
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 1).unwrap();
    let t = turn(id);
    let evidence = Uuid::new_v4();
    let op = Uuid::new_v4();
    let old = heads(&ledger, id);
    let prepare = || {
        ledger.prepare_legacy(
            op,
            old.clone(),
            t.clone(),
            evidence,
            Some(input(3, LegacyImageRole::AcquisitionParent, None)),
            vec![
                input(1, LegacyImageRole::Overview, Some(0)),
                input(2, LegacyImageRole::Detail, Some(1)),
            ],
        )
    };
    let prepared = prepare().unwrap();
    assert!(prepare().is_ok());
    assert_ne!(prepared.parent.as_ref().unwrap(), &prepared.images[0]);
    assert_eq!(
        prepared.images[1],
        input(2, LegacyImageRole::Detail, Some(1)).bytes
    );
    let mut interpreted = ledger
        .inspect(id, false)
        .unwrap()
        .into_iter()
        .find_map(|r| match r {
            Record::Turn(t) => Some(t),
            _ => None,
        })
        .unwrap();
    interpreted.outcome = Outcome::Interpreted;
    interpreted.text = Some("verified user request".into());
    ledger
        .advance(
            Uuid::new_v4(),
            heads(&ledger, id),
            heads(&ledger, t.id),
            interpreted,
        )
        .unwrap();
    drop(ledger);
    drop(store);
    let ledger = Ledger::new(f.open());
    let reopened = ledger.stored_legacy_images(evidence).unwrap();
    assert_eq!(reopened.images, prepared.images);
    let context = ledger
        .context(
            id,
            &ContextBudget {
                max_turns: 10,
                max_text_bytes: 1000,
                provider_token_limit: Some(100),
            },
            |_| Ok(1),
        )
        .unwrap();
    assert!(context.sources.is_empty() && context.captures.is_empty());
    assert_eq!(context.legacy_captures.len(), 1);
    let json = serde_json::to_value(&reopened.evidence).unwrap();
    assert!(json["identity"].is_null() && json["qualification"].is_null());
    for field in ["identity", "qualification", "parent"] {
        let mut bad = json.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<LegacyCapture>(bad).is_err(),
            "missing {field}"
        );
    }
    ledger
        .delete(id, Uuid::new_v4(), heads(&ledger, id), None)
        .unwrap();
    assert_eq!(ledger.retained_media(id).unwrap().len(), 3);
    assert!(ledger.stored_legacy_images(evidence).is_err());
}

#[test]
fn invalid_descriptors_and_precommit_failures_never_publish_prepared_turn() {
    for fault in [
        Fault::BeforeObjects,
        Fault::AfterObjects,
        Fault::BeforeCommit,
    ] {
        let f = Fixture::new();
        let store = f.open();
        let ledger = Ledger::new(store.clone());
        let id = Uuid::new_v4();
        ledger.create(id, Uuid::new_v4(), 1).unwrap();
        store.set_fault(fault).unwrap();
        assert!(ledger
            .prepare_legacy(
                Uuid::new_v4(),
                heads(&ledger, id),
                turn(id),
                Uuid::new_v4(),
                None,
                vec![input(1, LegacyImageRole::Overview, Some(0))]
            )
            .is_err());
        assert!(!ledger
            .inspect(id, false)
            .unwrap()
            .iter()
            .any(|r| matches!(r, Record::Turn(_))));
    }
    let f = Fixture::new();
    let ledger = Ledger::new(f.open());
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 1).unwrap();
    let mut bad = input(1, LegacyImageRole::Overview, Some(0));
    bad.bytes.truncate(30);
    bad.descriptor.media.sha256 = digest(&bad.bytes);
    bad.descriptor.media.bytes = 30;
    assert!(ledger
        .prepare_legacy(
            Uuid::new_v4(),
            heads(&ledger, id),
            turn(id),
            Uuid::new_v4(),
            None,
            vec![bad]
        )
        .is_err());
    assert!(ledger
        .prepare_legacy(
            Uuid::new_v4(),
            heads(&ledger, id),
            turn(id),
            Uuid::new_v4(),
            None,
            vec![input(1, LegacyImageRole::Detail, Some(0))]
        )
        .is_err());
}
#[test]
fn uncertain_output_reconciliation_is_fact_only_cas_and_fingerprint_checked() {
    let f = Fixture::new();
    let ledger = Ledger::new(f.open());
    let id = Uuid::new_v4();
    ledger.create(id, Uuid::new_v4(), 1).unwrap();
    let mut draft = turn(id);
    draft.role = Role::Assistant;
    draft.outcome = Outcome::Generated;
    draft.text = Some("original draft".into());
    ledger
        .append(Uuid::new_v4(), heads(&ledger, id), draft.clone(), vec![])
        .unwrap();
    draft = ledger
        .inspect(id, false)
        .unwrap()
        .into_iter()
        .find_map(|r| match r {
            Record::Turn(t) => Some(t),
            _ => None,
        })
        .unwrap();
    let root = heads(&ledger, id);
    let head = heads(&ledger, draft.id);
    let op = Uuid::new_v4();
    draft.outcome = Outcome::ReconcileRequired;
    ledger
        .record_attempt_outcome(
            op,
            root.clone(),
            head.clone(),
            draft.clone(),
            AttemptReason::OutputPending,
        )
        .unwrap();
    ledger
        .record_attempt_outcome(
            op,
            root.clone(),
            head.clone(),
            draft.clone(),
            AttemptReason::OutputPending,
        )
        .unwrap();
    assert!(ledger
        .record_attempt_outcome(
            op,
            root,
            head,
            draft.clone(),
            AttemptReason::DeviceUncertain
        )
        .is_err());
    let root = heads(&ledger, id);
    let head = heads(&ledger, draft.id);
    draft.outcome = Outcome::Failed;
    assert!(ledger
        .advance(Uuid::new_v4(), root.clone(), head.clone(), draft.clone())
        .is_err());
    let mut changed = draft.clone();
    changed.text = Some("changed draft".into());
    assert!(ledger
        .record_attempt_outcome(
            Uuid::new_v4(),
            root.clone(),
            head.clone(),
            changed,
            AttemptReason::NoSuccessor
        )
        .is_err());
    ledger
        .record_attempt_outcome(
            Uuid::new_v4(),
            root,
            head,
            draft,
            AttemptReason::NoSuccessor,
        )
        .unwrap();
    let records = ledger.inspect(id, false).unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, Record::OutcomeFact(_)))
            .count(),
        2
    );
    assert!(records.iter().any(|r|matches!(r,Record::Turn(t) if t.outcome==Outcome::Failed && t.text.as_deref()==Some("original draft"))));
}
