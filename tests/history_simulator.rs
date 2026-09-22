use remarkable_reader_buddy::{
    device::interaction::Contact,
    simulator::{
        execute,
        scenario::{ContactFrame, Effect, Fault, HistoryAction, Operation, Scenario},
        Run,
    },
    workflow::history::State,
};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios")
}
fn scenario(name: &str) -> Scenario {
    serde_json::from_slice(&std::fs::read(root().join(format!("{name}.json"))).unwrap()).unwrap()
}
fn contacts(count: u8) -> Vec<Contact> {
    (0..count)
        .map(|slot| Contact {
            slot,
            tracking: i32::from(slot) + 10,
            x: 250 + i32::from(slot) * 70,
            y: 600,
        })
        .collect()
}
fn hold(count: u8) -> HistoryAction {
    HistoryAction::Hold {
        frames: vec![
            ContactFrame {
                at_ms: 0,
                contacts: contacts(count),
            },
            ContactFrame {
                at_ms: 2100,
                contacts: Vec::new(),
            },
        ],
    }
}
fn checked(scenario: &Scenario) -> Run {
    let run = execute(scenario, &root()).unwrap();
    assert!(
        run.report.assertion_failures.is_empty(),
        "{:?}",
        run.report.assertion_failures
    );
    run
}

#[test]
fn repeated_toggles_preserve_header_and_prior_answer_without_model_calls() {
    let mut scenario = scenario("append");
    scenario.iterations[1].actions = vec![hold(4), hold(2), hold(4), hold(2)];
    scenario.expect.history = Some("applied".into());
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 4);
    let run = checked(&scenario);
    assert_eq!(run.report.model_calls, 4);
    let first = run
        .report
        .trace
        .iter()
        .position(|e| e.action == "historymutation")
        .unwrap();
    assert!(!run.report.trace[first..]
        .iter()
        .any(|e| matches!(e.action.as_str(), "statuscircle" | "statusclear" | "text")));
}

#[test]
fn first_qa_undo_retains_header() {
    let mut scenario = scenario("blank-answer");
    scenario.iterations[0].actions = vec![hold(4)];
    scenario
        .expect
        .text
        .insert(1, "=== Reader Buddy Answers ===\n\n\n".into());
    scenario.expect.history = Some("undone".into());
    checked(&scenario);
}

#[test]
fn returning_restarting_or_losing_input_never_revives_old_history() {
    for actions in [
        vec![
            HistoryAction::Page { page: 0 },
            HistoryAction::Page { page: 1 },
        ],
        vec![HistoryAction::Restart],
        vec![HistoryAction::InputLost],
    ] {
        let mut scenario = scenario("blank-answer");
        scenario.iterations[0].actions = actions;
        scenario.iterations[0].actions.extend([hold(4), hold(2)]);
        scenario.expect.history = Some("empty".into());
        scenario
            .expect
            .operations
            .insert(Operation::HistoryMutation, 0);
        checked(&scenario);
    }
}

#[test]
fn manual_edit_remains_after_attempted_undo() {
    let mut scenario = scenario("blank-answer");
    scenario.iterations[0].actions = vec![
        HistoryAction::Edit {
            text: "Manual note\n".into(),
        },
        hold(4),
    ];
    scenario
        .expect
        .text
        .get_mut(&1)
        .unwrap()
        .push_str("Manual note\n");
    scenario.expect.history = Some("empty".into());
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 0);
    checked(&scenario);
}

#[test]
fn stable_stale_snapshot_never_arms_but_complete_delayed_text_can() {
    for effect in [Effect::Stale, Effect::Lag] {
        let mut scenario = scenario("blank-answer");
        scenario.faults.push(Fault {
            operation: Operation::HistorySnapshot,
            call: 2,
            effect,
        });
        scenario.iterations[0].actions = vec![hold(4)];
        if effect == Effect::Lag {
            scenario
                .expect
                .text
                .insert(1, "=== Reader Buddy Answers ===\n\n\n".into());
            scenario.expect.history = Some("undone".into());
        } else {
            scenario.expect.history = Some("empty".into());
            scenario
                .expect
                .operations
                .insert(Operation::HistoryMutation, 0);
        }
        checked(&scenario);
    }
}

#[test]
fn partial_mutation_keeps_actual_partial_text_and_discards_redo() {
    let mut scenario = scenario("blank-answer");
    let applied = scenario.expect.text[&1].clone();
    scenario.expect.text.remove(&1);
    scenario.expect.history = Some("empty".into());
    scenario.expect.errors = vec!["Injected partial history mutation".into()];
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 1);
    scenario.faults.push(Fault {
        operation: Operation::HistoryMutation,
        call: 1,
        effect: Effect::Partial,
    });
    scenario.iterations[0].actions = vec![hold(4), hold(2), hold(4)];
    let run = checked(&scenario);
    let text = &run.report.pages[1].text;
    assert!(text.starts_with("=== Reader Buddy Answers ===\n\n\nQ @ (0.5, 0.22):"));
    assert!(text.len() < applied.len());
    assert_eq!(run.report.pages[1].x_count, 0);
}

#[test]
fn prolonged_partial_release_cancels_instead_of_becoming_a_two_finger_redo() {
    let mut scenario = scenario("blank-answer");
    scenario.iterations[0].actions = vec![
        HistoryAction::Hold {
            frames: vec![
                ContactFrame {
                    at_ms: 0,
                    contacts: contacts(4),
                },
                ContactFrame {
                    at_ms: 2100,
                    contacts: contacts(2),
                },
                ContactFrame {
                    at_ms: 2701,
                    contacts: Vec::new(),
                },
            ],
        },
        hold(4),
    ];
    scenario.expect.history = Some("empty".into());
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 0);
    let run = checked(&scenario);
    assert_eq!(run.report.history, State::Empty);
}

#[test]
fn declined_new_iteration_forgets_the_previous_answer_before_analysis() {
    let mut scenario = scenario("append");
    scenario.replies[2].text = Some("NONE".into());
    scenario.replies.pop();
    scenario.iterations[1].actions = vec![hold(4)];
    scenario
        .expect
        .text
        .insert(1, self::scenario("blank-answer").expect.text[&1].clone());
    scenario.expect.active_page = Some(0);
    scenario.expect.model_calls = Some(3);
    scenario.expect.x_count.insert(0, 1);
    scenario.expect.unchanged_pages.clear();
    scenario.expect.operations.clear();
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 0);
    scenario.expect.history = Some("empty".into());
    checked(&scenario);
}

#[test]
fn stable_wrong_page_metadata_cannot_arm_history() {
    let mut scenario = scenario("blank-answer");
    scenario.faults.push(Fault {
        operation: Operation::HistorySnapshot,
        call: 1,
        effect: Effect::WrongPage,
    });
    scenario.iterations[0].actions = vec![hold(4)];
    scenario.expect.history = Some("empty".into());
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 0);
    checked(&scenario);
}

#[test]
fn missing_native_identity_keeps_rendered_answer_but_never_revives_its_history() {
    let mut scenario = scenario("blank-answer");
    scenario.faults.push(Fault {
        operation: Operation::HistorySnapshot,
        call: 1,
        effect: Effect::Unavailable,
    });
    scenario.iterations[0].actions = vec![hold(4), hold(2), hold(4)];
    scenario.expect.history = Some("empty".into());
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 0);
    let run = checked(&scenario);
    assert!(run
        .report
        .trace
        .iter()
        .any(|event| event.action == "history_identity_unavailable"));
}

#[test]
fn overview_tap_invalidates_even_when_page_identity_does_not_change() {
    let mut scenario = scenario("blank-answer");
    scenario.iterations[0].actions = vec![
        HistoryAction::Hold {
            frames: vec![
                ContactFrame {
                    at_ms: 0,
                    contacts: vec![Contact {
                        slot: 0,
                        tracking: 71,
                        x: 30,
                        y: 807,
                    }],
                },
                ContactFrame {
                    at_ms: 100,
                    contacts: Vec::new(),
                },
            ],
        },
        hold(4),
    ];
    scenario.expect.history = Some("empty".into());
    scenario
        .expect
        .operations
        .insert(Operation::HistoryMutation, 0);
    checked(&scenario);
}
