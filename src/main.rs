use anyhow::Result;
use clap::Parser;
use dotenv::dotenv;
use log::info;
use remarkable_reader_buddy::llm::openai::DEFAULT_MODEL;
use remarkable_reader_buddy::{OpenAI, Orchestrator, TriggerCorner, Workflow};
use std::thread::sleep;
use std::time::Duration;

#[derive(Parser)]
#[command(author, version)]
#[command(about = "ReMarkable Reader Buddy - AI-powered reading assistant for reMarkable tablets")]
#[command(
    long_about = "ReMarkable Reader Buddy watches for circled content and handwritten questions, \
                        then uses ChatGPT to provide answers directly on your reMarkable tablet."
)]
pub struct Args {
    /// Run a bounded local scenario without tablet access or API credentials
    #[arg(long, value_name = "SCENARIO", conflicts_with_all = ["screenshot_only", "model", "base_url", "no_trigger", "once", "trigger_corner"])]
    simulate: Option<std::path::PathBuf>,
    /// Capture a PNG and exit without credentials, input devices, or an AI call
    #[arg(long, value_name = "FILE")]
    screenshot_only: Option<String>,
    /// OpenAI model to use
    #[arg(long, short, default_value = DEFAULT_MODEL)]
    model: String,

    /// OpenAI base URL (for custom endpoints)
    #[arg(long)]
    base_url: Option<String>,

    /// Disable trigger waiting (run immediately)
    #[arg(long)]
    no_trigger: bool,

    /// Run only once instead of looping
    #[arg(long)]
    once: bool,

    /// Trigger corner (UR, UL, LR, LL)
    #[arg(long, default_value = "LL")]
    trigger_corner: String,
}

fn debug_dump_enabled(value: Option<&str>) -> Result<bool> {
    match value {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(_) => anyhow::bail!("READER_BUDDY_DEBUG_DUMP must be true, false, 1 or 0"),
    }
}

fn main() -> Result<()> {
    // Load .env file if it exists
    dotenv().ok();

    let args = Args::parse();

    // Initialize logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    info!("=== ReMarkable Reader Buddy Starting ===");
    if let Some(path) = args.simulate {
        remarkable_reader_buddy::simulator::run_file(&path)?;
        return Ok(());
    }
    if let Some(path) = args.screenshot_only {
        let mut screenshot = remarkable_reader_buddy::Screenshot::new()?;
        screenshot.take_screenshot()?;
        screenshot.save_image(&path)?;
        info!("Screenshot saved to {}", path);
        return Ok(());
    }
    info!("Model: {}", args.model);
    info!("Trigger Corner: {}", args.trigger_corner);

    // Parse trigger corner
    let trigger_corner = TriggerCorner::from_string(&args.trigger_corner)?;

    let debug_dump = debug_dump_enabled(std::env::var("READER_BUDDY_DEBUG_DUMP").ok().as_deref())?;
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY environment variable not set"))?;
    anyhow::ensure!(!api_key.trim().is_empty(), "OPENAI_API_KEY is empty");
    let base_url = args
        .base_url
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok());
    let llm = OpenAI::new(args.model, api_key, base_url);

    // Initialize workflow
    let workflow = Workflow::new(false, trigger_corner, debug_dump)?;

    // Give time for the virtual devices to be initialized
    sleep(Duration::from_millis(1000));

    // Create orchestrator
    let mut orchestrator = Orchestrator::new(workflow, llm);
    orchestrator.set_trigger_enabled(!args.no_trigger);

    info!("Initialization complete");

    // Run the workflow
    if args.once {
        info!("Running single iteration");
        orchestrator.run_iteration()?;
    } else {
        info!("Starting main loop");
        orchestrator.run_loop()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_bounded_diagnostics_and_defaults() {
        let args = Args::try_parse_from(["reader-buddy", "--once", "--no-trigger"]).unwrap();
        assert!(args.once && args.no_trigger);
        assert_eq!(args.model, DEFAULT_MODEL);
        assert_eq!(args.trigger_corner, "LL");
        let args = Args::try_parse_from(["reader-buddy", "--screenshot-only", "page.png"]).unwrap();
        assert_eq!(args.screenshot_only.as_deref(), Some("page.png"));
    }

    #[test]
    fn simulator_has_one_explicit_selector_and_rejects_conflicting_modes() {
        let args = Args::try_parse_from(["reader-buddy", "--simulate", "scenario.json"]).unwrap();
        assert_eq!(
            args.simulate.unwrap(),
            std::path::PathBuf::from("scenario.json")
        );
        for extra in [
            vec!["--once"],
            vec!["--no-trigger"],
            vec!["--model", "example"],
            vec!["--trigger-corner", "UR"],
            vec!["--screenshot-only", "out.png"],
        ] {
            assert!(Args::try_parse_from(
                ["reader-buddy", "--simulate", "scenario.json"]
                    .into_iter()
                    .chain(extra)
            )
            .is_err());
        }
    }

    #[test]
    fn removed_modes_are_rejected_instead_of_silently_ignored() {
        for removed_args in [
            vec!["--input-png", "input.png"],
            vec!["--save-screenshot", "output.png"],
            vec!["--no-draw"],
            vec!["--api-key", "not-a-real-key"],
            vec!["--log-level", "debug"],
            vec!["--debug-dump"],
        ] {
            assert!(
                Args::try_parse_from(
                    std::iter::once("reader-buddy").chain(removed_args.iter().copied())
                )
                .is_err(),
                "{removed_args:?}"
            );
        }
    }

    #[test]
    fn image_dumps_require_explicit_valid_configuration() {
        for value in [None, Some("false"), Some("0")] {
            assert!(!debug_dump_enabled(value).unwrap());
        }
        for value in [Some("true"), Some("1")] {
            assert!(debug_dump_enabled(value).unwrap());
        }
        assert!(debug_dump_enabled(Some("tru")).is_err());
    }
}
