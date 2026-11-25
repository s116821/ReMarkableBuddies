//! Strict deliberate historical subject correction/retarget through existing CLI.
use super::*;
use crate::{config::Config, workflow::conversation_dispatch::Dispatch};
use serde::Deserialize;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: Uuid,
    conversation: Uuid,
    user: Uuid,
    expected_root: ExpectedHeads,
    text: String,
    change: TargetChange,
}
#[derive(Serialize)]
pub struct Report {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Acknowledgment>,
}
impl Report {
    pub fn write_to(&self, output: &mut impl Write) -> Result<()> {
        serde_json::to_writer(&mut *output, self)?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    }
}
pub fn run(config_path: Option<PathBuf>, path: &Path, input: &mut impl Read) -> Report {
    let result = (|| -> Result<WriteResult> {
        ensure!(
            path == Path::new("-"),
            "target changes require bounded explicit stdin"
        );
        let mut bytes = Vec::new();
        input.take(32 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 32 * 1024,
            "target change input exceeds bound"
        );
        let request: Request = serde_json::from_slice(&bytes)?;
        let ids = [request.operation, request.conversation, request.user];
        ensure!(
            ids.iter().all(|id| !id.is_nil())
                && ids.into_iter().collect::<BTreeSet<_>>().len() == 3
                && request.expected_root.0.len() == 1
                && !request.text.trim().is_empty()
                && request.text.len() <= 4096
                && !request.text.contains('\0'),
            "invalid target change request"
        );
        let config = Config::load(config_path)?;
        ensure!(
            config.reader_enabled()
                && !config.sync.enabled
                && config.reader.selection.is_none()
                && config.paths.data.join("identity.json").exists(),
            "target change requires existing ordinary policy"
        );
        let ledger = Ledger::new(Arc::new(Store::open_source_target_startup(&config)?));
        let _dispatch = Dispatch::local(&config, &ledger)?;
        ledger.change_target(
            request.operation,
            request.expected_root,
            Turn {
                id: request.user,
                conversation: request.conversation,
                exchange: request.operation,
                sequence: 0,
                role: Role::User,
                mode: Mode::Reader,
                outcome: Outcome::Interpreted,
                text: Some(request.text),
                sources: vec![],
                correction_of: None,
                created_ms: 0,
                updated_ms: 0,
                completion: None,
            },
            &request.change,
        )
    })();
    match result {
        Ok(result) => Report {
            status: "acknowledged",
            result: Some(result.historical),
        },
        Err(_) => Report {
            status: "refused",
            result: None,
        },
    }
}
