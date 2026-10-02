//! Authorized development-tablet diagnostic: one immediate, potentially paid Q&A.
//! Stop the normal service first. Uses environment credentials, default model and LL.
use remarkable_reader_buddy::{OpenAI, Orchestrator, TriggerCorner, Workflow};

fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::args_os().len() == 1,
        "Usage: reader_once (environment configuration only)"
    );
    dotenv::dotenv().ok();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,remarkable_reader_buddy=debug"),
    )
    .format_timestamp_millis()
    .init();
    let llm = OpenAI::from_env(None)?;
    let workflow = Workflow::new(false, TriggerCorner::from_string("LL")?, false)?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let config = remarkable_reader_buddy::config::Config::load(
        std::env::var_os("REMARKABLE_BUDDIES_CONFIG").map(Into::into),
    )?;
    let store = std::sync::Arc::new(remarkable_reader_buddy::storage::Store::open(config.paths)?);
    let mut orchestrator = Orchestrator::new(
        workflow,
        llm,
        remarkable_reader_buddy::conversation::Ledger::new(store),
    );
    orchestrator.set_trigger_enabled(false);
    orchestrator.run_iteration()
}
