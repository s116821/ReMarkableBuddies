use super::*;
use crate::handwriting::{mutation_identity, Action, Correction};
use crate::handwriting::{ConfirmedExample, ContextIdentity, ExampleIdentity, Provider};
use crate::storage::{digest, StorePaths};
use crate::storage::{Envelope, Kind, Namespace};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::{io::Cursor, path::PathBuf};
struct Local {
    root: PathBuf,
    config: Config,
}
impl Local {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rmb-transcription-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let config = Config {
            paths: StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            },
            ..Config::default()
        };
        Self { root, config }
    }
    fn confirm(
        &self,
        correction: Uuid,
        png: &[u8],
        original: &str,
        corrected: &str,
    ) -> (Uuid, Uuid) {
        let store = Arc::new(Store::open(self.config.paths.clone()).unwrap());
        let request = json!({"handwriting_version":1,"command":"confirm","profile":profile(),"correction":correction,
            "operation":Uuid::new_v4(),"expected_heads":[],"confirmation":"user-confirmed","original_text":original,
            "corrected_text":corrected,"confirmed_ms":1,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic consumer test only","source":null},"sample_png_base64":STANDARD.encode(png)});
        let input =
            handwriting::parse(&mut Cursor::new(serde_json::to_vec(&request).unwrap())).unwrap();
        let report = LocalCorrections::new(store.clone(), &self.config)
            .unwrap()
            .execute(input);
        assert_eq!(report.status, handwriting::Status::Complete);
        let value = (report.facts[0].record, report.facts[0].revision);
        drop(report);
        drop(store);
        value
    }
}
impl Drop for Local {
    fn drop(&mut self) {
        let path = self.root.canonicalize().unwrap();
        assert_eq!(
            path.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rmb-transcription-"));
        std::fs::remove_dir_all(path).unwrap();
    }
}
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/handwriting-transcription-v1.json"
    ))
    .unwrap()
}
fn profile() -> Uuid {
    Uuid::parse_str("00000000-0000-4000-8000-000000000010").unwrap()
}
fn png() -> Vec<u8> {
    STANDARD
        .decode(fixture()["target_png"].as_str().unwrap())
        .unwrap()
}
fn request(context: Value) -> Value {
    json!({"transcription_version":1,"invocation":Uuid::new_v4(),"document":Uuid::new_v4(),"page":7,"target_png":STANDARD.encode(png()),"context":context})
}
fn context(correction: Uuid, revision: Uuid) -> Value {
    json!({"profile":profile(),"examples":[{"correction":correction,"revision":revision}]})
}
fn good_reply() -> String {
    json!({"pages":[{"page":7,"status":"transcribed","text":"café x²"}]}).to_string()
}
struct Engine {
    parts: Vec<Value>,
    captured: Vec<Value>,
    calls: usize,
    reply: String,
    fail: bool,
}
impl Engine {
    fn new() -> Self {
        Self {
            parts: vec![],
            captured: vec![],
            calls: 0,
            reply: good_reply(),
            fail: false,
        }
    }
}
impl LLMEngine for Engine {
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(json!({"type":"text","text":text}));
    }
    fn add_image_content(&mut self, png: &str) {
        self.parts.push(json!({"type":"image_url","image_url":png}));
    }
    fn clear_content(&mut self) {
        self.parts.clear();
    }
    fn execute(&mut self) -> anyhow::Result<String> {
        self.calls += 1;
        self.captured = self.parts.clone();
        if self.fail {
            anyhow::bail!("PRIVATE_PROVIDER_CANARY");
        }
        Ok(self.reply.clone())
    }
}
fn run(local: &Local, input: &Value, engine: &mut Engine) -> Report {
    run_with_engine(
        &local.config,
        &serde_json::to_vec(input).unwrap(),
        "model-é/v1",
        engine,
        &mut || Ok(()),
    )
}
fn output(report: &Report) -> Value {
    let mut bytes = Vec::new();
    let _ = report.write_to(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
#[test]
fn default_normalizer_and_admitted_no_context_keep_frozen_payload_and_identity() {
    let local = Local::new();
    let vector = fixture();
    let mut engine = Engine::new();
    let bytes = png();
    let original = notebook::normalize(
        Uuid::new_v4(),
        &[notebook::PageImage {
            page: 7,
            png: &bytes,
        }],
        "model-é/v1",
        &mut engine,
        notebook::Options::default(),
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&engine.captured).unwrap(),
        vector["cases"][0]["prompt_parts"]
    );
    assert_eq!(original.prompt_sha256, vector["cases"][0]["prompt_sha256"]);
    assert_eq!(original.document.source_sha256, vector["source_sha256"]);
    assert_eq!(
        serde_json::to_string(&original.profile).unwrap(),
        vector["cases"][0]["profile_json"]
    );
    let mut input = request(Value::Null);
    input["document"] = json!(original.document.document);
    let report = run(&local, &input, &mut engine);
    assert_eq!(report.status, Status::Normalized);
    assert_eq!(
        serde_json::to_value(report.result.as_ref().unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    assert!(!local.config.paths.data.exists());
    assert!(engine.parts.is_empty());
    assert!(!report.confirmed);
}
#[test]
fn six_literal_vectors_exercise_production_context_payload_and_profile_recipes() {
    let local = Local::new();
    let vector = fixture();
    for case in vector["cases"].as_array().unwrap() {
        let target = notebook::admit_target(7, png(), &mut |_| Ok(())).unwrap();
        let examples = case["selected"].as_array().unwrap();
        let captured = if examples.is_empty() {
            None
        } else {
            let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
            let owner = LocalCorrections::new(store, &local.config).unwrap();
            let values = examples
                .iter()
                .map(|e| {
                    let raw = STANDARD.decode(e["sample_png"].as_str().unwrap()).unwrap();
                    ConfirmedExample {
                        identity: ExampleIdentity {
                            correction: serde_json::from_value(e["correction"].clone()).unwrap(),
                            record: serde_json::from_value(e["record"].clone()).unwrap(),
                            revision: serde_json::from_value(e["revision"].clone()).unwrap(),
                            sample_sha256: digest(&raw),
                            original_text_sha256: digest(
                                e["original_text"].as_str().unwrap().as_bytes(),
                            ),
                            corrected_text_sha256: digest(
                                e["corrected_text"].as_str().unwrap().as_bytes(),
                            ),
                        },
                        png: raw,
                        original_text: e["original_text"].as_str().unwrap().into(),
                        corrected_text: e["corrected_text"].as_str().unwrap().into(),
                    }
                })
                .collect::<Vec<_>>();
            // Recipe-only fixtures intentionally bypass current-head capture here;
            // separate actual-Store controls qualify that production admission.
            Some(ConfirmedContext {
                identity: ContextIdentity {
                    profile: profile(),
                    examples: values.iter().map(|e| e.identity.clone()).collect(),
                },
                examples: values,
                _owner: owner,
            })
        };
        let mut engine = Engine::new();
        let (outcome, called) = notebook::normalize_admitted(
            Uuid::new_v4(),
            &target,
            "model-é/v1",
            captured.as_ref(),
            &mut engine,
            &mut |_| Ok(()),
        );
        assert!(called);
        let Normalized::Page(result) = outcome.unwrap() else {
            panic!("expected page");
        };
        assert_eq!(
            serde_json::to_value(&engine.captured).unwrap(),
            case["prompt_parts"]
        );
        assert_eq!(result.prompt_sha256, case["prompt_sha256"]);
        assert_eq!(
            result.prompt_bytes,
            case["prompt_bytes"].as_u64().unwrap() as usize
        );
        assert_eq!(
            serde_json::to_string(&result.profile).unwrap(),
            case["profile_json"]
        );
        assert_eq!(
            result.document.extractor_revision,
            case["extractor_revision"]
        );
        assert_eq!(result.document.source_sha256, vector["source_sha256"]);
        assert_eq!(result.document.units.len(), 1);
        assert_eq!(result.document.units[0].page, Some(7));
    }
}
#[test]
fn real_current_capture_is_owned_through_provider_report_write_failure_and_drop() {
    let local = Local::new();
    let correction = Uuid::new_v4();
    let (_, revision) = local.confirm(correction, &png(), "PRIVATE_ORIGINAL", "PRIVATE_CORRECTED");
    let mut engine = Engine::new();
    let input = request(context(correction, revision));
    let report = run(&local, &input, &mut engine);
    assert_eq!(report.status, Status::Normalized);
    assert_eq!(engine.calls, 1);
    assert_eq!(report.provider_attempt, Attempt::Called);
    assert!(Store::open(local.config.paths.clone()).is_err());
    assert!(report
        ._owner
        .as_ref()
        .unwrap()
        ._owner
        .publisher_excluded_for_test());
    let data = output(&report);
    assert_eq!(data["context"]["examples"][0]["revision"], json!(revision));
    assert_eq!(data["confirmed"], false);
    assert!(data["context"]["examples"][0]
        .get("original_text")
        .is_none());
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("blocked"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(report.write_to(&mut Broken).is_err());
    assert!(Store::open(local.config.paths.clone()).is_err());
    drop(report);
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    assert!(store.legacy_publisher_lease().is_ok());
}
#[test]
fn missing_stale_duplicate_profile_and_unsupported_mode_never_call_or_publish_partial_context() {
    let local = Local::new();
    let correction = Uuid::new_v4();
    let (_, revision) = local.confirm(correction, &png(), "a", "b");
    let mut cases = vec![
        (context(correction, Uuid::new_v4()), Failure::ContextStale),
        (
            context(Uuid::new_v4(), revision),
            Failure::ContextUnavailable,
        ),
    ];
    let mut foreign = context(correction, revision);
    foreign["profile"] = json!(Uuid::new_v4());
    cases.push((foreign, Failure::ContextUnavailable));
    let mut duplicate = context(correction, revision);
    let first = duplicate["examples"][0].clone();
    duplicate["examples"].as_array_mut().unwrap().push(first);
    cases.push((duplicate, Failure::ContextInvalid));
    for (ctx, failure) in cases {
        let mut engine = Engine::new();
        let report = run(&local, &request(ctx), &mut engine);
        assert_eq!(report.failure, Some(failure));
        assert_eq!(engine.calls, 0);
        assert!(report.context.is_none());
        if failure != Failure::ContextInvalid {
            assert!(report._refused_owner.is_some());
            assert!(Store::open(local.config.paths.clone()).is_err());
        }
    }
    let mut config = local.config.clone();
    config.sync.enabled = true;
    let mut engine = Engine::new();
    let report = run_with_engine(
        &config,
        &serde_json::to_vec(&request(context(correction, revision))).unwrap(),
        "m",
        &mut engine,
        &mut || Ok(()),
    );
    assert_eq!(report.failure, Some(Failure::UnsupportedMode));
    assert_eq!(engine.calls, 0);
}
#[test]
fn retained_trailing_png_is_valid_storage_but_ineligible_for_context_without_rewrite() {
    let local = Local::new();
    let correction = Uuid::new_v4();
    let mut sample = png();
    sample.extend_from_slice(b"PRIVATE_TRAILING");
    let (record, revision) = local.confirm(correction, &sample, "old", "correct");
    let store = Store::open(local.config.paths.clone()).unwrap();
    let before = store
        .snapshot_revisions_matching(&[crate::storage::Namespace::Handwriting], 4096, |e| {
            e.record_id == record
        })
        .unwrap();
    let before = serde_json::to_vec(&before).unwrap();
    drop(store);
    let mut engine = Engine::new();
    let report = run(&local, &request(context(correction, revision)), &mut engine);
    assert_eq!(report.failure, Some(Failure::ContextUnavailable));
    assert!(report.context.is_none());
    assert_eq!(engine.calls, 0);
    drop(report);
    let store = Store::open(local.config.paths.clone()).unwrap();
    let after = store
        .snapshot_revisions_matching(&[crate::storage::Namespace::Handwriting], 4096, |e| {
            e.record_id == record
        })
        .unwrap();
    assert_eq!(before, serde_json::to_vec(&after).unwrap());
    assert_eq!(
        store
            .read_object(&crate::storage::ObjectRef {
                sha256: digest(&sample),
                bytes: sample.len() as u64
            })
            .unwrap(),
        sample
    );
}
#[test]
fn exact_reply_shapes_distinguish_blank_unreadable_malformed_and_provider_failure() {
    let local = Local::new();
    for (reply, status, failure) in [
        (
            r#"{"pages":[{"page":7,"status":"blank","text":""}]}"#,
            Status::Normalized,
            None,
        ),
        (
            r#"{"pages":[{"page":7,"status":"unreadable","text":""}]}"#,
            Status::Unreadable,
            None,
        ),
        (
            r#"{"pages":[{"page":7,"status":"unreadable","text":"guess"}]}"#,
            Status::Refused,
            Some(Failure::InvalidResponse),
        ),
        (
            r#"{"pages":[{"page":8,"status":"transcribed","text":"wrong page"}]}"#,
            Status::Refused,
            Some(Failure::InvalidResponse),
        ),
        (
            r#"{"pages":[],"pages":[]}"#,
            Status::Refused,
            Some(Failure::InvalidResponse),
        ),
        (
            "PRIVATE_PROSE",
            Status::Refused,
            Some(Failure::InvalidResponse),
        ),
    ] {
        let mut engine = Engine::new();
        engine.reply = reply.into();
        let report = run(&local, &request(Value::Null), &mut engine);
        assert_eq!(report.status, status);
        assert_eq!(report.failure, failure);
        assert_eq!(report.provider_attempt, Attempt::Called);
        assert_eq!(engine.calls, 1);
        assert!(engine.parts.is_empty());
    }
    let mut engine = Engine::new();
    engine.fail = true;
    let report = run(&local, &request(Value::Null), &mut engine);
    assert_eq!(report.failure, Some(Failure::ProviderFailed));
    assert!(!output(&report)
        .to_string()
        .contains("PRIVATE_PROVIDER_CANARY"));
}
#[test]
fn strict_envelope_limits_and_required_null_refuse_before_store_or_engine() {
    let local = Local::new();
    let mut variants = vec![];
    let mut bad = request(Value::Null);
    bad.as_object_mut().unwrap().remove("context");
    variants.push(bad);
    let mut bad = request(Value::Null);
    bad["page"] = json!(0);
    variants.push(bad);
    let mut bad = request(Value::Null);
    bad["extra"] = json!(true);
    variants.push(bad);
    let mut bad = request(Value::Null);
    bad["invocation"] = json!("00000000-0000-0000-0000-000000000000");
    variants.push(bad);
    let mut bad = request(Value::Null);
    bad["target_png"] = json!("not png");
    variants.push(bad);
    for input in variants {
        let mut engine = Engine::new();
        let report = run(&local, &input, &mut engine);
        assert!(report.failure.is_some());
        assert_eq!(report.provider_attempt, Attempt::NotCalled);
        assert_eq!(engine.calls, 0);
        assert!(!local.config.paths.data.exists());
    }
    assert!(matches!(
        bounded_input(&mut Cursor::new(vec![b' '; MAX_INPUT + 1])),
        Err(Failure::InputLimit)
    ));
}
#[test]
fn noncooperative_default_engine_finishes_but_post_return_cancellation_suppresses_result() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Ignores {
        done: Arc<AtomicBool>,
    }
    impl LLMEngine for Ignores {
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, _: &str) {}
        fn clear_content(&mut self) {}
        fn execute(&mut self) -> anyhow::Result<String> {
            self.done.store(true, Ordering::Release);
            Ok(good_reply())
        }
    }
    let local = Local::new();
    let done = Arc::new(AtomicBool::new(false));
    let mut engine = Ignores { done: done.clone() };
    let report = run_with_engine(
        &local.config,
        &serde_json::to_vec(&request(Value::Null)).unwrap(),
        "model",
        &mut engine,
        &mut || {
            if done.load(Ordering::Acquire) {
                anyhow::bail!("canceled");
            }
            Ok(())
        },
    );
    assert!(done.load(Ordering::Acquire));
    assert_eq!(report.failure, Some(Failure::Canceled));
    assert_eq!(report.provider_attempt, Attempt::Called);
    assert!(report.result.is_none());
    let mut engine = Engine::new();
    let report = run_with_engine(
        &local.config,
        &serde_json::to_vec(&request(Value::Null)).unwrap(),
        "model",
        &mut engine,
        &mut || anyhow::bail!("canceled"),
    );
    assert_eq!(report.failure, Some(Failure::Canceled));
    assert_eq!(report.provider_attempt, Attempt::NotCalled);
    assert_eq!(engine.calls, 0);
}

#[test]
fn actual_retained_apng_is_refused_by_static_consumer_before_invocation() {
    let local = Local::new();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/handwriting-consumer-png.json"
    ))
    .unwrap();
    let image = STANDARD.decode(fixture["apng"].as_str().unwrap()).unwrap();
    assert!(handwriting::png_dimensions(&image).is_ok());
    let correction = Uuid::new_v4();
    let (_, revision) = local.confirm(correction, &image, "a", "b");
    let mut engine = Engine::new();
    let report = run(&local, &request(context(correction, revision)), &mut engine);
    assert_eq!(report.failure, Some(Failure::ContextUnavailable));
    assert_eq!(report.provider_attempt, Attempt::NotCalled);
    assert!(report.context.is_none());
    assert_eq!(engine.calls, 0);
    drop(report);
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    let check = json!({"handwriting_version":1,"command":"inspect","profile":profile(),"correction":correction});
    let report = LocalCorrections::new(store, &local.config)
        .unwrap()
        .execute(
            handwriting::parse(&mut Cursor::new(serde_json::to_vec(&check).unwrap())).unwrap(),
        );
    assert_eq!(report.status, handwriting::Status::Complete);
    assert_eq!(report.facts[0].revision, revision);
}
#[test]
fn prompt_budget_refuses_all_context_and_whole_report_overflow_never_truncates_success() {
    let local = Local::new();
    let mut examples = vec![];
    for _ in 0..3 {
        let key = Uuid::new_v4();
        let text = "x\t".repeat(2048);
        let (_, revision) = local.confirm(key, &png(), &text, &text);
        examples.push(json!({"correction":key,"revision":revision}));
    }
    let mut engine = Engine::new();
    let report = run(
        &local,
        &request(json!({"profile":profile(),"examples":examples})),
        &mut engine,
    );
    assert_eq!(report.failure, Some(Failure::PromptLimit));
    assert_eq!(engine.calls, 0);
    assert!(report.context.is_some());
    drop(report);
    let mut report = run(&local, &request(Value::Null), &mut engine);
    report.result.as_mut().unwrap().document.units[0].text = "x".repeat(40000);
    let mut bytes = vec![];
    assert!(!report.write_to(&mut bytes).unwrap());
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["failure"], "output-limit");
    assert!(result["result"].is_null());
    assert!(bytes.len() <= 32768);
    assert_eq!(engine.calls, 1);
}

fn envelope(store: &Store, value: &Correction, parents: BTreeSet<Uuid>) -> Envelope {
    let operation = Uuid::new_v4();
    let record = value.record_id().unwrap();
    let action = if parents.is_empty() {
        Action::Confirm
    } else {
        Action::Update
    };
    let (_, revision) = mutation_identity(
        action,
        operation,
        record,
        &parents,
        Some(&value.value_digest().unwrap()),
    )
    .unwrap();
    Envelope {
        envelope_version: 1,
        namespace: Namespace::Handwriting,
        domain_schema_version: 1,
        record_id: record,
        revision_id: revision,
        parents,
        operation_id: operation,
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: serde_json::to_value(value).unwrap(),
        media_descriptors: vec![value.sample.clone()],
    }
}
fn import(store: &Store, envelope: Envelope, sample: Option<&[u8]>) {
    use crate::storage::{Coverage, Manifest, ObjectRef, Omission, Scope};
    let bytes = serde_json::to_vec(&envelope).unwrap();
    let hash = digest(&bytes);
    let descriptor = &envelope.media_descriptors[0];
    let mut objects = std::collections::BTreeMap::from([(hash.clone(), bytes.clone())]);
    let (media, coverage) = if let Some(sample) = sample {
        objects.insert(descriptor.sha256.clone(), sample.to_vec());
        (
            vec![ObjectRef {
                sha256: descriptor.sha256.clone(),
                bytes: descriptor.bytes,
            }],
            Coverage::Included {
                sha256: descriptor.sha256.clone(),
            },
        )
    } else {
        (
            vec![],
            Coverage::Omitted {
                sha256: descriptor.sha256.clone(),
                reason: Omission::UnavailableAtSource,
            },
        )
    };
    let manifest = Manifest {
        format: 1,
        transaction_id: Uuid::new_v4(),
        scope: Scope::LocalTransaction,
        records: vec![ObjectRef {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
        }],
        record_namespaces: std::collections::BTreeMap::from([(hash, Namespace::Handwriting)]),
        media,
        media_coverage: vec![coverage],
    };
    store.import(manifest, objects).unwrap();
}

#[test]
fn real_forked_import_conflicts_and_declared_missing_media_do_not_become_context() {
    let local = Local::new();
    let key = Uuid::new_v4();
    let (record, root_revision) = local.confirm(key, &png(), "old", "correct");
    let store = Store::open(local.config.paths.clone()).unwrap();
    let root = store
        .snapshot_revisions_matching(&[Namespace::Handwriting], 4096, |e| e.record_id == record)
        .unwrap()
        .remove(0);
    let mut value: Correction = serde_json::from_value(root.payload).unwrap();
    let mut selected = root_revision;
    for text in ["fork A", "fork B"] {
        value.corrected_text = text.into();
        value.confirmed_ms += 1;
        let fork = envelope(&store, &value, BTreeSet::from([root_revision]));
        selected = fork.revision_id;
        import(&store, fork, Some(&png()));
    }
    drop(store);
    let mut engine = Engine::new();
    let report = run(&local, &request(context(key, selected)), &mut engine);
    assert_eq!(report.failure, Some(Failure::ContextConflict));
    assert_eq!(engine.calls, 0);
    assert!(report.context.is_none());
    drop(report);
    let missing = Local::new();
    let store = Store::open(missing.config.paths.clone()).unwrap();
    let absent = envelope(&store, &value, BTreeSet::new());
    let revision = absent.revision_id;
    import(&store, absent, None);
    drop(store);
    let report = run(&missing, &request(context(key, revision)), &mut engine);
    assert_eq!(report.failure, Some(Failure::ContextUnavailable));
    assert_eq!(engine.calls, 0);
}

#[test]
fn actual_revision_only_update_changes_extractor_but_not_transmitted_prompt() {
    let local = Local::new();
    let key = Uuid::new_v4();
    let (_, revision) = local.confirm(key, &png(), "old", "correct");
    let mut engine = Engine::new();
    let report = run(&local, &request(context(key, revision)), &mut engine);
    let original = report.result.as_ref().unwrap();
    let first = (
        original.prompt_sha256.clone(),
        original.document.extractor_revision.clone(),
    );
    drop(report);
    let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
    let update = json!({"handwriting_version":1,"command":"update","profile":profile(),"correction":key,
        "operation":Uuid::new_v4(),"expected_heads":[revision],"confirmation":"user-confirmed","original_text":"old","corrected_text":"correct","confirmed_ms":2,
        "provenance":{"kind":"caller-confirmed-sample","description":"Synthetic consumer test only","source":null},"sample_png_base64":STANDARD.encode(png())});
    let accepted = LocalCorrections::new(store.clone(), &local.config)
        .unwrap()
        .execute(
            handwriting::parse(&mut Cursor::new(serde_json::to_vec(&update).unwrap())).unwrap(),
        );
    assert_eq!(accepted.status, handwriting::Status::Complete);
    let next = accepted.facts[0].revision;
    assert_ne!(next, revision);
    drop(accepted);
    drop(store);
    let report = run(&local, &request(context(key, next)), &mut engine);
    let result = report.result.as_ref().unwrap();
    assert_eq!(result.prompt_sha256, first.0);
    assert_ne!(result.document.extractor_revision, first.1);
    assert_eq!(engine.calls, 2);
}
