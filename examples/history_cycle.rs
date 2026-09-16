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
        args.len() == 3,
        "usage: history_cycle QA_FILE OUTPUT_DIRECTORY"
    );
    let qa = std::fs::read_to_string(&args[1])?;
    anyhow::ensure!(
        qa.is_ascii() && qa.len() <= 500 && qa.ends_with('\n'),
        "Diagnostic requires a bounded ASCII Q&A"
    );
    let directory = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(directory)?;
    let mut workflow = Workflow::new(false, TriggerCorner::LowerLeft, false)?;
    sleep(Duration::from_secs(1));
    workflow.set_body_text_mode()?;
    workflow.render_qa(&qa)?;
    anyhow::ensure!(
        workflow.history_state() == State::Applied,
        "History did not arm after native append"
    );
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
