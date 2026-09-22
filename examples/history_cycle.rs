//! Explicit offline mutation diagnostic for an already selected disposable page.
//! Prepare and visually verify its native text insertion cursor before running.
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use remarkable_reader_buddy::{
        workflow::{
            history::{Action, State},
            Workflow,
        },
        TriggerCorner,
    };
    use std::{thread::sleep, time::Duration};
    env_logger::init();
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 3
            || (args.len() == 4
                && matches!(
                    args[3].as_str(),
                    "--gestures" | "--reader-only" | "--loss-recovery"
                )),
        "usage: history_cycle QA_FILE OUTPUT_DIRECTORY [--gestures|--reader-only|--loss-recovery]"
    );
    let qa = std::fs::read_to_string(&args[1])?;
    anyhow::ensure!(
        qa.is_ascii()
            && qa.len() <= remarkable_reader_buddy::workflow::history::MAX_CHARACTERS
            && qa.ends_with('\n'),
        "Diagnostic requires a bounded ASCII Q&A"
    );
    let directory = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(directory)?;
    let mut workflow = Workflow::new(false, TriggerCorner::LowerLeft, false)?;
    sleep(Duration::from_secs(1));
    if args.get(3).is_some_and(|mode| mode == "--reader-only") {
        println!("Waiting for lower-left Reader hold without typing or a model call");
        workflow.wait_for_reader_bounded(120)?;
        println!("Reader trigger accepted");
        return Ok(());
    }
    workflow.set_body_text_mode()?;
    workflow.render_qa(&qa)?;
    if args.get(3).is_some_and(|mode| mode == "--loss-recovery") {
        anyhow::ensure!(
            workflow.history_state() == State::Empty,
            "Expected externally interrupted history preparation"
        );
        println!(
            "History unavailable after external interruption; waiting for a fresh Reader hold"
        );
        workflow.wait_for_reader_bounded(120)?;
        println!("Reader trigger accepted after observer recovery");
        return Ok(());
    }
    anyhow::ensure!(
        workflow.history_state() == State::Applied,
        "History did not arm after native append"
    );
    if args.len() == 4 {
        println!("History armed; waiting for gestures, then a lower-left Reader hold to finish without a model call");
        workflow.wait_for_reader_bounded(120)?;
        println!(
            "Reader trigger accepted; history {:?}",
            workflow.history_state()
        );
        return Ok(());
    }
    for (index, action) in [
        None,
        Some(Action::Undo),
        Some(Action::Redo),
        Some(Action::Undo),
        Some(Action::Redo),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(action) = action {
            anyhow::ensure!(
                workflow.history_action(action)?,
                "Native action was not accepted"
            );
        }
        let (_, png) = workflow.capture_screenshot_with_data()?;
        std::fs::write(directory.join(format!("cycle-{index}.png")), png)?;
        println!("cycle-{index}: {:?}", workflow.history_state());
        sleep(Duration::from_secs(1));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("Native history diagnostic requires Linux tablet hardware")
}
