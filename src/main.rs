use anyhow::Result;
use clap::Parser;
use dotenv::dotenv;
use log::info;
use remarkable_reader_buddy::config::{self, Config, Environment, Overrides};
use remarkable_reader_buddy::storage::{files, sync::Worker, Store};
use remarkable_reader_buddy::{OpenAI, Orchestrator, Workflow};
use std::sync::Arc;
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
    #[arg(long, short)]
    model: Option<String>,
    /// OpenAI endpoint (overrides OPENAI_BASE_URL)
    #[arg(long)]
    base_url: Option<String>,
    /// Trigger corner (UR, UL, LR, LL)
    #[arg(long)]
    trigger_corner: Option<String>,
    /// Global log level, overriding RUST_LOG: off, error, warn, info, debug, trace
    #[arg(long)]
    log_level: Option<log::LevelFilter>,
    /// Save local page-image diagnostics (independent of logging)
    #[arg(long)]
    debug_dump: bool,
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
    if let Some(path) = args.simulate {
        logging(args.log_level, std::env::var("RUST_LOG").ok().as_deref()).init();
        remarkable_reader_buddy::simulator::run_file(&path)?;
        return Ok(());
    }
    let config = Config::load(std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into))?;
    let effective = config::resolve(
        &config,
        Overrides {
            api_key: args.api_key,
            model: args.model,
            base_url: args.base_url,
            trigger_corner: args.trigger_corner,
            log_level: args.log_level,
            debug_dump: args.debug_dump,
        },
        Environment::current(),
    )?;
    let mut logger = logging(effective.log_level, effective.log_filter.as_deref());
    // HTTP dependency trace messages can include private resumable-session URLs.
    // Keep the established app logging policy, but never emit transport internals.
    logger
        .filter_module("ureq", log::LevelFilter::Off)
        .filter_module("ureq_proto", log::LevelFilter::Off)
        .init();
    info!("=== ReMarkable Reader Buddy Starting ===");
    let store = Arc::new(Store::open(config.paths.clone())?);
    files::atomic_json(
        &store.paths.data.join("effective-config.json"),
        &effective.sources,
    )?;
    info!("Model: {}", effective.model);
    info!("Trigger Corner: {}", effective.corner_name);
    let llm = OpenAI::new(
        effective.model,
        effective.key.into_string(),
        effective.base_url,
    );
    let workflow = Workflow::new(false, effective.trigger_corner, effective.debug_dump)?;
    sleep(Duration::from_millis(1000));
    let mut orchestrator = Orchestrator::new(workflow, llm);
    let _sync_worker = Worker::google(store, config.sync)?;
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
        assert!(args.model.is_none());
        assert!(args.trigger_corner.is_none());
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
