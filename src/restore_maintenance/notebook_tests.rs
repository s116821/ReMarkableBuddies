use super::*;
use crate::storage::{
    ordinary_restore::tests::{record, Fixture},
    Fault,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs};

fn frame(report: Report) -> Value {
    let mut bytes = Vec::new();
    report.write_to(&mut bytes).unwrap();
    assert!(bytes.len() <= MAX_IO && bytes.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
    serde_json::from_slice(&bytes).unwrap()
}
fn invoke(f: &Fixture, operation: Operation, body: &Value) -> Value {
    let bytes = serde_json::to_vec(body).unwrap();
    frame(run(
        operation,
        Path::new("-"),
        Some(f.root.join("config.json")),
        &mut bytes.as_slice(),
    ))
}
fn archive(f: &Fixture) -> (PathBuf, Uuid) {
    let store = Store::prepare_notebook(f.paths()).unwrap();
    let envelope = record(&store);
    let revision = envelope.revision_id;
    store.commit(vec![envelope], BTreeMap::new()).unwrap();
    let path = f.root.join("notebook-archive");
    store.export_notebook_backup(&path).unwrap();
    (path, revision)
}
fn prepared(f: &Fixture, source: &Path) -> Value {
    let report = invoke(
        f,
        Operation::PrepareNotebook,
        &json!({"restore_version":2,"operation":Uuid::new_v4(),"source":source}),
    );
    assert_eq!(report["status"], "ok", "{report}");
    assert_eq!(report["outcome"], "prepared");
    assert_eq!(report["report_version"], 2);
    json!({"source":source,"intent":report["data"]["intent"]})
}

#[test]
fn notebook_restore_real_prepare_apply_status_reopen_and_legacy_isolation() {
    let f = Fixture::new();
    let (source, original) = archive(&f);
    let store = Store::open_existing_for_notebook_restore(f.paths()).unwrap();
    let extra = record(&store);
    let extra_revision = extra.revision_id;
    store.commit(vec![extra], BTreeMap::new()).unwrap();
    drop(store);
    let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
    let body = prepared(&f, &source);
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
    assert_eq!(
        invoke(
            &f,
            Operation::Prepare,
            &json!({"restore_version":1,"operation":Uuid::new_v4(),"source":source})
        )["status"],
        "unsupported"
    );
    assert_eq!(invoke(&f, Operation::Apply, &body)["status"], "unsupported");
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["outcome"],
        "restored"
    );
    let desired = fs::read(f.paths().data.join("CURRENT")).unwrap();
    assert_ne!(before, desired);
    assert_eq!(
        invoke(&f, Operation::StatusNotebook, &body)["outcome"],
        "desired-generation-observed"
    );
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "conflict"
    );
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), desired);
    let store = Store::open_existing_for_notebook_restore(f.paths()).unwrap();
    let records = store
        .snapshot_revisions_matching(&[crate::storage::Namespace::Conversation], 32, |_| true)
        .unwrap();
    assert!(records.iter().any(|r| r.revision_id == original));
    assert!(records.iter().any(|r| r.revision_id == extra_revision));
    assert!(Store::open_existing(f.paths()).is_err());
}

#[test]
fn notebook_restore_refuses_actor_corrupt_archive_and_stale_source_without_activation() {
    let f = Fixture::new();
    let (source, _) = archive(&f);
    let foreign = Fixture::new();
    drop(Store::prepare_notebook(foreign.paths()).unwrap());
    let before = fs::read(foreign.paths().data.join("CURRENT")).unwrap();
    assert_eq!(
        invoke(
            &foreign,
            Operation::PrepareNotebook,
            &json!({"restore_version":2,"operation":Uuid::new_v4(),"source":source})
        )["status"],
        "unsupported"
    );
    assert_eq!(
        fs::read(foreign.paths().data.join("CURRENT")).unwrap(),
        before
    );
    let body = prepared(&f, &source);
    let marker = source.join("backup.json");
    let raw = fs::read(&marker).unwrap();
    let mut altered = raw.clone();
    altered.push(b' ');
    fs::write(&marker, altered).unwrap();
    let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "conflict"
    );
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
    fs::write(marker, raw).unwrap();
    let object = fs::read_dir(source.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(object, b"corrupt").unwrap();
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "unsupported"
    );
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
}

#[test]
fn notebook_restore_original_base_and_configuration_are_not_refreshed() {
    let f = Fixture::new();
    let (source, _) = archive(&f);
    let body = prepared(&f, &source);
    let store = Store::open_existing_for_notebook_restore(f.paths()).unwrap();
    store.commit(vec![record(&store)], BTreeMap::new()).unwrap();
    drop(store);
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "conflict"
    );
    let body = prepared(&f, &source);
    let path = f.root.join("config.json");
    let mut raw = fs::read(&path).unwrap();
    raw.push(b' ');
    fs::write(path, raw).unwrap();
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "conflict"
    );
}

#[test]
fn notebook_restore_preserves_existing_target_and_refuses_busy_wrong_route_and_missing_media() {
    let f = Fixture::new();
    let (source, _) = archive(&f);
    let body = prepared(&f, &source);
    let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
    let store = Store::open_existing_for_notebook_restore(f.paths()).unwrap();
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "busy"
    );
    drop(store);
    let mut wrong = body.clone();
    wrong["intent"]["restore_version"] = json!(1);
    assert_ne!(
        invoke(&f, Operation::ApplyNotebook, &wrong)["outcome"],
        "restored"
    );
    let target = f
        .paths()
        .data
        .join("generations")
        .join(body["intent"]["target_generation"].as_str().unwrap());
    fs::create_dir(&target).unwrap();
    fs::write(target.join("held-staging"), b"preserve").unwrap();
    assert_eq!(
        invoke(&f, Operation::ApplyNotebook, &body)["status"],
        "conflict"
    );
    assert_eq!(fs::read(target.join("held-staging")).unwrap(), b"preserve");
    assert_eq!(
        invoke(&f, Operation::StatusNotebook, &body)["outcome"],
        "unresolved"
    );
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
    let object = fs::read_dir(source.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(object).unwrap();
    assert_eq!(
        invoke(
            &f,
            Operation::PrepareNotebook,
            &json!({"restore_version":2,"operation":Uuid::new_v4(),"source":source})
        )["status"],
        "unsupported"
    );
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
}

#[test]
fn notebook_restore_faults_keep_original_outcome_and_read_only_status() {
    for fault in [Fault::BeforeActivation, Fault::AfterActivation] {
        let f = Fixture::new();
        let (source, _) = archive(&f);
        let body = prepared(&f, &source);
        let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
        let document =
            Document::load(Some(f.root.join("config.json"))).unwrap_or_else(|_| panic!("config"));
        let bytes = serde_json::to_vec(&body).unwrap();
        let report = intent_operation_with(
            Operation::ApplyNotebook,
            &document,
            &bytes,
            |store, path, conditions, doc| {
                store.set_fault(fault)?;
                store.apply_notebook_restore(path, conditions, || doc.original())
            },
        )
        .unwrap_or_else(|_| panic!("request"));
        let report = frame(report);
        assert_eq!(report["status"], "failed");
        let status = invoke(&f, Operation::StatusNotebook, &body);
        if fs::read(f.paths().data.join("CURRENT")).unwrap() == before {
            assert_eq!(report["outcome"], "not-activated");
            assert_eq!(status["outcome"], "unresolved");
        } else {
            assert_eq!(report["outcome"], "unknown");
            assert_eq!(status["outcome"], "desired-generation-observed");
        }
        assert_ne!(
            invoke(&f, Operation::ApplyNotebook, &body)["outcome"],
            "restored"
        );
    }
}

#[test]
fn notebook_restore_lease_survives_report_flush_and_no_second_frame() {
    struct Sink<'a> {
        paths: &'a crate::storage::StorePaths,
        flushes: usize,
    }
    impl Write for Sink<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            assert!(matches!(
                Store::open_existing_for_notebook_restore(self.paths.clone()),
                Err(Refusal::Busy)
            ));
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            assert!(matches!(
                Store::open_existing_for_notebook_restore(self.paths.clone()),
                Err(Refusal::Busy)
            ));
            Err(std::io::Error::other("flush"))
        }
    }
    let f = Fixture::new();
    let (source, _) = archive(&f);
    let paths = f.paths();
    let bytes = serde_json::to_vec(
        &json!({"restore_version":2,"operation":Uuid::new_v4(),"source":source}),
    )
    .unwrap();
    let report = run(
        Operation::PrepareNotebook,
        Path::new("-"),
        Some(f.root.join("config.json")),
        &mut bytes.as_slice(),
    );
    let mut sink = Sink {
        paths: &paths,
        flushes: 0,
    };
    assert!(report.write_to(&mut sink).is_err());
    assert_eq!(sink.flushes, 1);
    assert!(Store::open_existing_for_notebook_restore(paths).is_ok());
}
