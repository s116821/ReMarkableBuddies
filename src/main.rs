use anyhow::Result;
use clap::Parser;
use dotenv::dotenv;
use log::info;
use remarkable_reader_buddy::llm::openai::DEFAULT_MODEL;
use remarkable_reader_buddy::{OpenAI, Orchestrator, TriggerCorner, Workflow};
use std::thread::sleep;
use std::time::Duration;

#[derive(Parser)]
#[command(author, version = env!("READER_BUDDY_VERSION"))]
#[command(about = "ReMarkable Reader Buddy - AI-powered reading assistant for reMarkable tablets")]
pub struct Args {
    /// Run a bounded local scenario (offline unless explicitly live)
    #[arg(long, value_name = "SCENARIO", conflicts_with_all = ["api_key", "model", "base_url", "trigger_corner", "debug_dump"])]
    simulate: Option<std::path::PathBuf>,
    /// API key (prefer OPENAI_API_KEY to avoid shell history/process-list exposure)
    #[arg(long)]
    api_key: Option<String>,
    /// OpenAI model to use
    #[arg(long, short, default_value = DEFAULT_MODEL)]
    model: String,
    /// OpenAI endpoint (overrides OPENAI_BASE_URL)
    #[arg(long)]
    base_url: Option<String>,
    /// Trigger corner (UR, UL, LR, LL)
    #[arg(long, default_value = "LL")]
    trigger_corner: String,
    /// Global log level, overriding RUST_LOG: off, error, warn, info, debug, trace
    #[arg(long)]
    log_level: Option<log::LevelFilter>,
    /// Save local page-image diagnostics (independent of logging)
    #[arg(long)]
    debug_dump: bool,
}

fn api_key(cli: Option<String>, environment: Option<String>) -> Result<String> {
    let key = cli
        .or(environment)
        .ok_or_else(|| anyhow::anyhow!("Set OPENAI_API_KEY or supply --api-key"))?;
    anyhow::ensure!(!key.trim().is_empty(), "Selected API key is empty");
    Ok(key)
}

fn debug_dump_enabled(explicit: bool, environment: Option<&str>) -> Result<bool> {
    if explicit {
        return Ok(true);
    }
    match environment {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(_) => anyhow::bail!("READER_BUDDY_DEBUG_DUMP must be true, false, 1 or 0"),
    }
}

fn logging(level: Option<log::LevelFilter>, environment: Option<&str>) -> env_logger::Builder {
    let mut builder = env_logger::Builder::new();
    if let Some(level) = level {
        builder.filter_level(level);
    } else if let Some(filter) = environment {
        builder.parse_filters(filter);
    } else {
        builder
            .filter_level(log::LevelFilter::Info)
            .filter_module("reader_buddy", log::LevelFilter::Debug)
            .filter_module("remarkable_reader_buddy", log::LevelFilter::Debug);
    }
    builder.format_timestamp_millis();
    builder
}

fn main() -> Result<()> {
    dotenv().ok();
    let args = Args::parse();
    logging(args.log_level, std::env::var("RUST_LOG").ok().as_deref()).init();
    info!("=== ReMarkable Reader Buddy Starting ===");
    if let Some(path) = args.simulate {
        remarkable_reader_buddy::simulator::run_file(&path)?;
        return Ok(());
    }
    let trigger_corner = TriggerCorner::from_string(&args.trigger_corner)?;
    let debug_dump = debug_dump_enabled(
        args.debug_dump,
        std::env::var("READER_BUDDY_DEBUG_DUMP").ok().as_deref(),
    )?;
    let key = api_key(args.api_key, std::env::var("OPENAI_API_KEY").ok())?;
    let base_url = args
        .base_url
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok());
    info!("Model: {}", args.model);
    info!("Trigger Corner: {}", args.trigger_corner);
    let llm = OpenAI::new(args.model, key, base_url);
    let workflow = Workflow::new(false, trigger_corner, debug_dump)?;
    sleep(Duration::from_millis(1000));
    let mut orchestrator = Orchestrator::new(workflow, llm);
    info!("Initialization complete; starting main loop");
    orchestrator.run_loop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use log::{Level, LevelFilter, Log, Metadata};

    #[test]
    fn exact_requested_interface_and_defaults() {
        let mut names: Vec<_> = Args::command()
            .get_arguments()
            .filter_map(|a| a.get_long().map(str::to_owned))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "api-key",
                "base-url",
                "debug-dump",
                "log-level",
                "model",
                "simulate",
                "trigger-corner"
            ]
        );
        let args = Args::try_parse_from(["reader-buddy"]).unwrap();
        assert_eq!(args.model, DEFAULT_MODEL);
        assert_eq!(args.trigger_corner, "LL");
        assert!(!args.debug_dump);
        for flag in [
            "--once",
            "--no-trigger",
            "--screenshot-only",
            "--input-png",
            "--save-screenshot",
            "--no-draw",
            "--debug",
        ] {
            assert!(
                Args::try_parse_from(["reader-buddy", flag]).is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn simulation_rejects_ignored_overrides_but_accepts_logging() {
        assert!(Args::try_parse_from([
            "reader-buddy",
            "--simulate",
            "scenario.json",
            "--log-level",
            "warn"
        ])
        .is_ok());
        for extra in [
            vec!["--api-key", "fixture-secret"],
            vec!["--model", "m"],
            vec!["--base-url", "url"],
            vec!["--trigger-corner", "UR"],
            vec!["--debug-dump"],
        ] {
            let error = Args::try_parse_from(
                ["reader-buddy", "--simulate", "scenario.json"]
                    .into_iter()
                    .chain(extra),
            )
            .err()
            .unwrap()
            .to_string();
            assert!(!error.contains("fixture-secret"));
        }
    }

    #[test]
    fn credentials_resolve_without_value_bearing_errors() {
        assert_eq!(
            api_key(Some("cli-fixture".into()), Some("env-fixture".into())).unwrap(),
            "cli-fixture"
        );
        assert_eq!(
            api_key(None, Some("env-fixture".into())).unwrap(),
            "env-fixture"
        );
        assert!(api_key(None, None).is_err());
        assert!(api_key(Some("  ".into()), Some("env-fixture".into())).is_err());
        assert!(api_key(None, Some("\t".into())).is_err());
        let help = Args::command().render_long_help().to_string();
        assert!(!help.contains("env-fixture"));
    }

    #[test]
    fn dumps_are_independent_and_explicit_flag_wins() {
        for value in [None, Some("false"), Some("0")] {
            assert!(!debug_dump_enabled(false, value).unwrap());
        }
        for value in [Some("true"), Some("1")] {
            assert!(debug_dump_enabled(false, value).unwrap());
        }
        assert!(debug_dump_enabled(false, Some("invalid")).is_err());
        assert!(debug_dump_enabled(true, Some("invalid")).unwrap());
        assert!(debug_dump_enabled(true, Some("false")).unwrap());
    }

    fn enabled(logger: &env_logger::Logger, target: &str, level: Level) -> bool {
        logger.enabled(&Metadata::builder().target(target).level(level).build())
    }

    #[test]
    fn logging_default_and_override_precedence() {
        let default = logging(None, None).build();
        for target in ["reader_buddy", "remarkable_reader_buddy::workflow"] {
            assert!(enabled(&default, target, Level::Debug));
            assert!(!enabled(&default, target, Level::Trace));
        }
        assert!(!enabled(&default, "ureq", Level::Debug));
        assert!(enabled(&default, "ureq", Level::Info));
        let env = logging(None, Some("error,remarkable_reader_buddy=trace")).build();
        assert!(enabled(&env, "remarkable_reader_buddy", Level::Trace));
        assert!(!enabled(&env, "reader_buddy", Level::Info));
        let cli = logging(Some(LevelFilter::Warn), Some("trace")).build();
        assert!(!enabled(&cli, "remarkable_reader_buddy", Level::Info));
        assert!(enabled(&cli, "ureq", Level::Warn));
        for level in ["off", "error", "warn", "info", "debug", "trace"] {
            assert!(Args::try_parse_from(["reader-buddy", "--log-level", level]).is_ok());
        }
        assert!(Args::try_parse_from(["reader-buddy", "--log-level", "verbose"]).is_err());
    }
}
