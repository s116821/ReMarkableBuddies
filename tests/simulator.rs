use remarkable_reader_buddy::simulator::{
    execute,
    scenario::{Operation, Scenario},
    Run,
};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios")
}
fn load(name: &str) -> Scenario {
    serde_json::from_slice(&std::fs::read(root().join(format!("{name}.json"))).unwrap()).unwrap()
}
fn run(name: &str) -> Run {
    let run = execute(&load(name), &root()).unwrap();
    assert!(
        run.report.assertion_failures.is_empty(),
        "{name}: {:?}",
        run.report.assertion_failures
    );
    for request in run
        .report
        .trace
        .iter()
        .filter(|event| event.action == "model_request")
    {
        assert_eq!(request.detail, "scripted texts=1 images=4");
    }
    run
}
macro_rules! scenario_test {
    ($name:ident,$fixture:literal) => {
        #[test]
        fn $name() {
            run($fixture);
        }
    };
}
scenario_test!(blank_answer, "blank-answer");
scenario_test!(
    highlight_without_question_is_rejected,
    "highlight-no-question"
);
scenario_test!(
    highlighted_question_uses_existing_answer_workflow,
    "highlighted-g"
);
scenario_test!(
    question_without_selection_is_rejected,
    "question-no-selection"
);
scenario_test!(
    classification_corrupt_returns_without_writing,
    "classification-corrupt"
);
scenario_test!(
    header_corrupt_keeps_existing_decode_behavior,
    "header-corrupt"
);
scenario_test!(
    missing_cache_preserves_previous_answer,
    "missing-cache-append"
);
scenario_test!(append_preserves_previous_answers, "append");
scenario_test!(no_question, "no-question");
scenario_test!(illegible_question, "illegible-question");
scenario_test!(independent_disagreement, "disagreement");
scenario_test!(verification_error, "verification-error");
scenario_test!(occupied_return, "occupied-return");
scenario_test!(failed_return_never_retries, "failed-return");
scenario_test!(return_input_error, "return-input-error");
scenario_test!(return_capture_error, "return-capture-error");
scenario_test!(recovery_precheck_error, "recovery-precheck-error");
scenario_test!(end_page, "end-page");
scenario_test!(forward_no_movement, "forward-no-movement");
scenario_test!(stale_forward_documents_heuristic_limit, "stale-forward");
scenario_test!(initial_capture_error, "capture-error");
scenario_test!(proposal_error, "proposal-error");
scenario_test!(header_cache_error_still_writes_answer, "header-cache-error");
scenario_test!(render_error_draws_x, "render-error");
scenario_test!(body_error_draws_x, "body-error");
scenario_test!(short_tap_never_calls_model, "short-tap");

#[test]
fn stationary_hold_uses_virtual_deadline() {
    let run = run("stationary-hold");
    let trigger = run
        .report
        .trace
        .iter()
        .find(|e| e.action == "hold_triggered")
        .unwrap();
    assert_eq!(trigger.at_ms, 2000);
    assert_eq!(run.report.virtual_ms, 4600); // hold + tap + navigation + settle + classify + header.
}
#[test]
fn interrupted_hold_discards_elapsed_time() {
    let run = run("interrupted-hold");
    assert_eq!(
        run.report
            .trace
            .iter()
            .find(|e| e.action == "hold_triggered")
            .unwrap()
            .at_ms,
        4000
    );
}
#[test]
fn reports_and_pixels_are_deterministic() {
    let first = run("failed-return");
    let second = run("failed-return");
    assert_eq!(
        serde_json::to_vec(&first.report).unwrap(),
        serde_json::to_vec(&second.report).unwrap()
    );
    assert_eq!(first.images, second.images);
}
#[test]
fn wrong_expectations_cannot_pass() {
    let mut scenario = load("blank-answer");
    scenario.expect.active_page = Some(0);
    scenario.expect.text.insert(1, "wrong answer".into());
    scenario
        .expect
        .text_contains
        .insert(1, vec!["absent phrase".into()]);
    scenario.expect.operations.insert(Operation::Previous, 1);
    scenario.expect.unchanged_pages.push(1);
    let run = execute(&scenario, &root()).unwrap();
    assert_eq!(run.report.assertion_failures.len(), 5);
}
#[test]
fn schema_rejects_unknown_fields_modes_and_invalid_faults() {
    let mut value = serde_json::to_value(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(root().join("blank-answer.json")).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    value["accidental_option"] = true.into();
    assert!(serde_json::from_value::<Scenario>(value).is_err());
    let mut scenario = load("blank-answer");
    for mode in ["writer", "combined"] {
        scenario.mode = mode.into();
        assert!(scenario
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Unsupported"));
    }
    scenario.mode = "reader".into();
    scenario.active_page = 99;
    assert!(scenario.validate().is_err());
    scenario.active_page = 0;
    scenario
        .faults
        .push(remarkable_reader_buddy::simulator::scenario::Fault {
            operation: Operation::Text,
            call: 1,
            effect: remarkable_reader_buddy::simulator::scenario::Effect::NoMove,
        });
    assert!(scenario.validate().is_err());
}
#[test]
fn cli_is_offline_and_saves_report_before_assertion_failure() {
    let temporary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!(
            "simulator-cli-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&temporary).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root().join("blank-answer.json")).unwrap()).unwrap();
    value["pages"][0]["image"] = root()
        .join("../../validation/images/rem13/final-input.png")
        .to_string_lossy()
        .to_string()
        .into();
    value["output"] = "result".into();
    value["expect"]["active_page"] = 0.into();
    let path = temporary.join("scenario.json");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .args(["--simulate", path.to_str().unwrap()])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .current_dir(&temporary)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(temporary.join("result/report.json")).unwrap())
            .unwrap();
    assert_eq!(report["model_mode"], "scripted-offline");
    assert_eq!(report["assertion_failures"].as_array().unwrap().len(), 1);
    assert!(temporary.join("result/page-1.png").exists());
    std::fs::remove_dir_all(&temporary).unwrap();
}

scenario_test!(
    highlight_with_illegible_question_is_rejected,
    "highlight-illegible"
);
