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
    /// Linux-only prepared deletion of one owned derived source-cache slot
    #[arg(long, value_enum, requires="source_cache_input", conflicts_with_all=["simulate","inspect_owned_data","api_key","model","base_url","trigger_corner","log_level","debug_dump","memory_operation","memory_input","sync_mode_operation","sync_mode_input","archive_operation","archive_input","personalization_operation","export_note","configuration_operation","basic_settings_operation","basic_settings_input","restore_operation","restore_input","model_settings_operation","model_settings_input","ordinary_export_operation","ordinary_export_input","memory_settings_operation","memory_settings_input","model_credential_operation","model_credential_input","credential_reference_operation","credential_reference_input"])]
    source_cache_operation: Option<remarkable_reader_buddy::source_cache_maintenance::Operation>,
    #[arg(long, value_name = "INPUT", requires = "source_cache_operation")]
    source_cache_input: Option<std::path::PathBuf>,
    /// Private saved credential reference only; no credential-file or runtime activation
    #[arg(long,value_enum,conflicts_with_all=["simulate","inspect_owned_data","api_key","model","base_url","trigger_corner","log_level","debug_dump","memory_operation","memory_input","sync_mode_operation","sync_mode_input","archive_operation","archive_input","personalization_operation","export_note","configuration_operation","basic_settings_operation","basic_settings_input","restore_operation","restore_input","model_settings_operation","model_settings_input","ordinary_export_operation","ordinary_export_input","memory_settings_operation","memory_settings_input","model_credential_operation","model_credential_input"])]
    credential_reference_operation: Option<config::credential_reference::Operation>,
    #[arg(
        long,
        value_name = "INPUT",
        requires = "credential_reference_operation"
    )]
    credential_reference_input: Option<std::path::PathBuf>,
    /// Protected declared credential only; confidential transport, no provider activation
    #[arg(long,value_enum,conflicts_with_all=["simulate","inspect_owned_data","api_key","model","base_url","trigger_corner","log_level","debug_dump","memory_operation","memory_input","sync_mode_operation","sync_mode_input","archive_operation","archive_input","personalization_operation","export_note","configuration_operation","basic_settings_operation","basic_settings_input","restore_operation","restore_input","model_settings_operation","model_settings_input","ordinary_export_operation","ordinary_export_input","memory_settings_operation","memory_settings_input"])]
    model_credential_operation: Option<config::model_credential::Operation>,
    #[arg(long, value_name = "INPUT", requires = "model_credential_operation")]
    model_credential_input: Option<std::path::PathBuf>,
    /// Saved memory declarations only; no profile creation or runtime activation
    #[arg(long,value_enum,conflicts_with_all=["simulate","inspect_owned_data","api_key","model","base_url","trigger_corner","log_level","debug_dump","memory_operation","memory_input","sync_mode_operation","sync_mode_input","archive_operation","archive_input","personalization_operation","export_note","configuration_operation","basic_settings_operation","basic_settings_input","restore_operation","restore_input","model_settings_operation","model_settings_input","ordinary_export_operation","ordinary_export_input"])]
    memory_settings_operation: Option<config::memory_settings::Operation>,
    #[arg(long, value_name = "INPUT", requires = "memory_settings_operation")]
    memory_settings_input: Option<std::path::PathBuf>,
    /// Original-bound ordinary format1 export; no runtime or selected activation
    #[arg(long,value_enum,requires="ordinary_export_input",conflicts_with_all=["simulate","inspect_owned_data","api_key","model","base_url","trigger_corner","log_level","debug_dump","memory_operation","memory_input","sync_mode_operation","sync_mode_input","archive_operation","archive_input","personalization_operation","export_note","configuration_operation","basic_settings_operation","basic_settings_input","restore_operation","restore_input","model_settings_operation","model_settings_input"])]
    ordinary_export_operation:
        Option<remarkable_reader_buddy::ordinary_export_maintenance::Operation>,
    #[arg(long, value_name = "INPUT", requires = "ordinary_export_operation")]
    ordinary_export_input: Option<std::path::PathBuf>,
    /// Private saved model/endpoint editing; never provider/runtime activation
    #[arg(long, value_enum, conflicts_with_all = ["simulate", "inspect_owned_data", "api_key", "model", "base_url", "trigger_corner", "log_level", "debug_dump", "memory_operation", "memory_input", "sync_mode_operation", "sync_mode_input", "archive_operation", "archive_input", "personalization_operation", "export_note", "configuration_operation", "basic_settings_operation", "basic_settings_input", "restore_operation", "restore_input"])]
    model_settings_operation: Option<config::model_settings::Operation>,
    #[arg(long, value_name = "INPUT", requires = "model_settings_operation")]
    model_settings_input: Option<std::path::PathBuf>,
    /// Conditional ordinary owned restore; never selected/device/runtime activation
    #[arg(long, value_enum, requires = "restore_input", conflicts_with_all = ["simulate", "inspect_owned_data", "api_key", "model", "base_url", "trigger_corner", "log_level", "debug_dump", "memory_operation", "memory_input", "sync_mode_operation", "sync_mode_input", "archive_operation", "archive_input", "personalization_operation", "export_note", "configuration_operation", "basic_settings_operation", "basic_settings_input"])]
    restore_operation: Option<remarkable_reader_buddy::restore_maintenance::Operation>,
    /// Exact-dash bounded JSON stdin; no file fallback
    #[arg(long, value_name = "INPUT", requires = "restore_operation")]
    restore_input: Option<std::path::PathBuf>,
    /// Deliberate three-field saved settings maintenance; never runtime activation
    #[arg(long, value_enum, conflicts_with_all = ["simulate", "inspect_owned_data", "api_key", "model", "base_url", "trigger_corner", "log_level", "debug_dump", "memory_operation", "memory_input", "sync_mode_operation", "sync_mode_input", "archive_operation", "archive_input", "personalization_operation", "export_note", "configuration_operation"])]
    basic_settings_operation: Option<config::basic::Operation>,
    /// Exact-dash bounded JSON stdin for prepare/apply/status; never a file fallback
    #[arg(long, value_name = "INPUT", requires = "basic_settings_operation")]
    basic_settings_input: Option<std::path::PathBuf>,
    /// Inspect saved configuration declarations without runtime or Store startup
    #[arg(long, value_enum, conflicts_with_all = ["simulate", "inspect_owned_data", "api_key", "model", "base_url", "trigger_corner", "log_level", "debug_dump", "memory_operation", "memory_input", "sync_mode_operation", "sync_mode_input", "archive_operation", "archive_input", "personalization_operation", "export_note"])]
    configuration_operation: Option<config::inspection::Operation>,
    /// Run a bounded local scenario (offline unless explicitly live)
    #[arg(long, value_name = "SCENARIO", conflicts_with_all = ["api_key", "model", "base_url", "trigger_corner", "debug_dump"])]
    simulate: Option<std::path::PathBuf>,
    /// Report existing Buddy-owned file sizes without starting the Reader
    #[arg(long, conflicts_with_all = ["simulate", "api_key", "model", "base_url", "trigger_corner", "debug_dump", "export_note", "personalization_operation"])]
    inspect_owned_data: bool,
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
    /// One-shot local memory maintenance; no provider or tablet runtime
    #[arg(long, value_enum, requires = "memory_input", conflicts_with_all = ["simulate", "api_key", "model", "base_url", "trigger_corner", "debug_dump", "log_level", "inspect_owned_data", "export_note", "personalization_operation"])]
    memory_operation: Option<remarkable_reader_buddy::memory::maintenance::Operation>,
    /// Bounded JSON input for an explicit memory operation
    #[arg(long, value_name = "FILE", requires = "memory_operation")]
    memory_input: Option<std::path::PathBuf>,
    /// Explicit local storage-mode maintenance; no sync or runtime activation
    #[arg(long, value_enum, conflicts_with_all = ["simulate", "api_key", "model", "base_url", "trigger_corner", "debug_dump", "log_level", "inspect_owned_data", "export_note", "personalization_operation", "memory_operation", "memory_input"])]
    sync_mode_operation: Option<remarkable_reader_buddy::mode_maintenance::Operation>,
    /// Bounded choice JSON from an absolute file or exact-dash stdin
    #[arg(
        long,
        value_name = "FILE",
        requires = "sync_mode_operation",
        required_if_eq("sync_mode_operation", "choose")
    )]
    sync_mode_input: Option<std::path::PathBuf>,
    /// Explicit offline archive inspection or selected-history export
    #[arg(long, value_enum, requires = "archive_input", conflicts_with_all = ["simulate", "api_key", "model", "base_url", "trigger_corner", "debug_dump", "log_level", "inspect_owned_data", "export_note", "personalization_operation", "memory_operation", "memory_input", "sync_mode_operation", "sync_mode_input"])]
    archive_operation: Option<remarkable_reader_buddy::archive_maintenance::Operation>,
    /// Strict bounded path JSON from an absolute file or exact-dash stdin
    #[arg(long, value_name = "FILE", requires = "archive_operation")]
    archive_input: Option<std::path::PathBuf>,
    /// Fixed local personalization maintenance; mutation JSON comes from bounded stdin
    #[arg(long, value_enum, conflicts_with_all=["simulate","api_key","model","base_url","trigger_corner","debug_dump","log_level","export_note"])]
    personalization_operation: Option<config::personalization::Operation>,
    /// Export prepared Markdown or reconcile a prior export; bounded JSON stdin
    #[arg(long, conflicts_with_all=["simulate","api_key","model","base_url","trigger_corner","debug_dump","log_level","personalization_operation"])]
    export_note: bool,
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
    let args = Args::parse();
    if let Some(operation) = args.source_cache_operation {
        return remarkable_reader_buddy::source_cache_maintenance::run(
            operation,
            args.source_cache_input.as_deref().expect("required input"),
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        )
        .write_to(&mut std::io::stdout().lock());
    }
    if let Some(operation) = args.credential_reference_operation {
        let report = if operation == config::credential_reference::Operation::Inspect {
            if args.credential_reference_input.is_some() {
                config::credential_reference::invalid_input(operation)
            } else {
                config::credential_reference::inspect(
                    std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                )
            }
        } else if args
            .credential_reference_input
            .as_ref()
            .is_some_and(|input| input.as_os_str() == std::ffi::OsStr::new("-"))
        {
            config::credential_reference::run_stdin(
                operation,
                std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                &mut std::io::stdin().lock(),
            )
        } else {
            config::credential_reference::unsupported_input(operation)
        };
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.model_credential_operation {
        let report = if operation == config::model_credential::Operation::Inspect {
            if args.model_credential_input.is_some() {
                config::model_credential::invalid_input(operation)
            } else {
                config::model_credential::inspect(
                    std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                )
            }
        } else if args.model_credential_input.as_deref() == Some(std::path::Path::new("-")) {
            config::model_credential::run_stdin(
                operation,
                std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                &mut std::io::stdin().lock(),
            )
        } else {
            config::model_credential::unsupported_input(operation)
        };
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.memory_settings_operation {
        let report = if operation == config::memory_settings::Operation::Inspect {
            if args.memory_settings_input.is_some() {
                config::memory_settings::invalid_input(operation)
            } else {
                config::memory_settings::inspect(
                    std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                )
            }
        } else if args
            .memory_settings_input
            .as_ref()
            .is_some_and(|input| input.as_os_str() == std::ffi::OsStr::new("-"))
        {
            config::memory_settings::run_stdin(
                operation,
                std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                &mut std::io::stdin().lock(),
            )
        } else {
            config::memory_settings::unsupported_input(operation)
        };
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.ordinary_export_operation {
        let report = remarkable_reader_buddy::ordinary_export_maintenance::run(
            operation,
            args.ordinary_export_input
                .as_deref()
                .expect("required export input"),
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        );
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.model_settings_operation {
        let report = if operation == config::model_settings::Operation::Inspect {
            if args.model_settings_input.is_some() {
                config::model_settings::invalid_input(operation)
            } else {
                config::model_settings::inspect(
                    std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                )
            }
        } else if args
            .model_settings_input
            .as_ref()
            .is_some_and(|input| input.as_os_str() == std::ffi::OsStr::new("-"))
        {
            config::model_settings::run_stdin(
                operation,
                std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                &mut std::io::stdin().lock(),
            )
        } else {
            config::model_settings::unsupported_input(operation)
        };
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.restore_operation {
        let report = remarkable_reader_buddy::restore_maintenance::run(
            operation,
            args.restore_input
                .as_deref()
                .expect("required restore input"),
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        );
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.basic_settings_operation {
        let report = if operation == config::basic::Operation::Inspect {
            if args.basic_settings_input.is_some() {
                config::basic::invalid_input(operation)
            } else {
                config::basic::inspect(
                    std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                )
            }
        } else if args
            .basic_settings_input
            .as_ref()
            .is_some_and(|input| input.as_os_str() == std::ffi::OsStr::new("-"))
        {
            config::basic::run_stdin(
                operation,
                std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
                &mut std::io::stdin().lock(),
            )
        } else {
            config::basic::unsupported_input(operation)
        };
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.configuration_operation {
        let report = config::inspection::run(
            operation,
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
        );
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.archive_operation {
        let report = remarkable_reader_buddy::archive_maintenance::run(
            operation,
            args.archive_input
                .as_deref()
                .expect("required archive input"),
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        );
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.sync_mode_operation {
        let report = remarkable_reader_buddy::mode_maintenance::run(
            operation,
            args.sync_mode_input.as_deref(),
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        );
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if let Some(operation) = args.memory_operation {
        let input = args
            .memory_input
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("memory input absent"))?;
        let config_path = std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into);
        let report = if input.as_os_str() == std::ffi::OsStr::new("-") {
            remarkable_reader_buddy::memory::maintenance::run_stdin(
                operation,
                &mut std::io::stdin().lock(),
                config_path,
            )
        } else {
            remarkable_reader_buddy::memory::maintenance::run(operation, input, config_path)
        };
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    if args.inspect_owned_data {
        let report = match Config::load(std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into)) {
            Ok(config) => remarkable_reader_buddy::storage::inspection::inspect_owned_data(&config.paths),
            Err(_) => remarkable_reader_buddy::storage::inspection::OwnedDataReport::configuration_unavailable(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if args.export_note {
        let report = remarkable_reader_buddy::note_export::command::run(
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        );
        serde_json::to_writer(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if let Some(operation) = args.personalization_operation {
        let report = config::personalization::run(
            operation,
            std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
            &mut std::io::stdin().lock(),
        );
        report.write_to(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    dotenv().ok();
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
    let _ordinary_runtime_lifetime =
        remarkable_reader_buddy::mode_maintenance::admit_ordinary(&store, config.sync.enabled)?;
    store.snapshot_config(&config)?;
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
    let mut orchestrator = Orchestrator::new(
        workflow,
        llm,
        remarkable_reader_buddy::conversation::Ledger::new(store.clone()),
    );
    let _sync_worker = Worker::google(store.clone(), config.sync)?;
    info!("Initialization complete; starting main loop");
    orchestrator.run_loop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use log::{Level, LevelFilter, Log, Metadata};

    #[test]
    fn source_cache_requires_input_and_rejects_other_execution_modes() {
        for args in [
            vec!["reader-buddy", "--source-cache-operation", "inspect"],
            vec!["reader-buddy", "--source-cache-input", "-"],
        ] {
            assert!(Args::try_parse_from(args).is_err());
        }
        for extra in [
            vec!["--api-key", "private-fixture-sentinel"],
            vec!["--model", "private-fixture-sentinel"],
            vec!["--simulate", "private-fixture-sentinel"],
            vec!["--inspect-owned-data"],
            vec!["--export-note"],
            vec!["--credential-reference-operation", "inspect"],
            vec!["--memory-settings-operation", "inspect"],
        ] {
            let mut args = vec![
                "reader-buddy",
                "--source-cache-operation",
                "inspect",
                "--source-cache-input",
                "-",
            ];
            args.extend(extra);
            let error = Args::try_parse_from(args)
                .err()
                .expect("execution modes conflict");
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
            assert!(!error.to_string().contains("private-fixture-sentinel"));
        }
        assert!(Args::try_parse_from([
            "reader-buddy",
            "--source-cache-operation",
            "inspect",
            "--source-cache-input",
            "-"
        ])
        .is_ok());
    }
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
                "archive-input",
                "archive-operation",
                "base-url",
                "basic-settings-input",
                "basic-settings-operation",
                "configuration-operation",
                "credential-reference-input",
                "credential-reference-operation",
                "debug-dump",
                "export-note",
                "inspect-owned-data",
                "log-level",
                "memory-input",
                "memory-operation",
                "memory-settings-input",
                "memory-settings-operation",
                "model",
                "model-credential-input",
                "model-credential-operation",
                "model-settings-input",
                "model-settings-operation",
                "ordinary-export-input",
                "ordinary-export-operation",
                "personalization-operation",
                "restore-input",
                "restore-operation",
                "simulate",
                "source-cache-input",
                "source-cache-operation",
                "sync-mode-input",
                "sync-mode-operation",
                "trigger-corner"
            ]
        );
        let args = Args::try_parse_from(["reader-buddy"]).unwrap();
        assert!(args.source_cache_operation.is_none());
        assert!(args.source_cache_input.is_none());
        assert!(args.restore_operation.is_none());
        assert!(args.restore_input.is_none());
        assert!(args.basic_settings_operation.is_none());
        assert!(args.basic_settings_input.is_none());
        assert!(args.configuration_operation.is_none());
        assert!(args.model.is_none());
        assert!(args.trigger_corner.is_none());
        assert!(!args.debug_dump);
        assert!(!args.inspect_owned_data);
        assert!(!args.export_note);
        assert!(
            Args::try_parse_from(["reader-buddy", "--export-note"])
                .unwrap()
                .export_note
        );
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
    fn personalization_rejects_runtime_overrides() {
        for operation in ["inspect", "set", "disable", "clear"] {
            assert!(Args::try_parse_from([
                "reader-buddy",
                "--personalization-operation",
                operation
            ])
            .is_ok());
            for extra in [
                vec!["--api-key", "fixture-secret"],
                vec!["--model", "model"],
                vec!["--base-url", "url"],
                vec!["--trigger-corner", "UR"],
                vec!["--debug-dump"],
                vec!["--log-level", "trace"],
                vec!["--simulate", "scenario.json"],
                vec!["--export-note"],
            ] {
                assert!(Args::try_parse_from(
                    ["reader-buddy", "--personalization-operation", operation]
                        .into_iter()
                        .chain(extra)
                )
                .is_err());
            }
        }
    }

    #[test]
    fn prepared_export_rejects_runtime_overrides_without_echoing_credentials() {
        for extra in [
            vec!["--api-key", "fixture-secret"],
            vec!["--model", "model"],
            vec!["--base-url", "url"],
            vec!["--trigger-corner", "UR"],
            vec!["--debug-dump"],
            vec!["--log-level", "trace"],
            vec!["--simulate", "scenario.json"],
            vec!["--personalization-operation", "inspect"],
        ] {
            let error =
                Args::try_parse_from(["reader-buddy", "--export-note"].into_iter().chain(extra))
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
