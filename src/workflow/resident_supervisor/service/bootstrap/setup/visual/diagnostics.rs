//! Development evidence only. No protocol, completion or dispatch authority.
use super::*;

const MAX_EVENTS: u8 = 48;
const MAX_ERROR_CHARS: usize = 512;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    SetupProgress,
    ActorEnded,
    CaptureProgress,
    Capture,
    Reconnect,
    CaptureApi,
    Publication,
    CaptureResult,
    Shutdown,
}

/// A finite set of exclusively created, synced records in the pinned control root.
/// An entry without a returned record means incomplete observation, not dispatch.
pub struct Diagnostics {
    root: PathBuf,
    root_identity: (u64, u64),
    binding: Binding,
    start: Instant,
    sequence: u8,
}
impl Diagnostics {
    pub fn new(root: &Path, binding: &Binding) -> Result<Self> {
        binding.validate()?;
        let metadata = private_root(root)?.metadata()?;
        Ok(Self {
            root: root.into(),
            root_identity: (metadata.dev(), metadata.ino()),
            binding: binding.clone(),
            start: Instant::now(),
            sequence: 0,
        })
    }
    fn record(
        &mut self,
        stage: Option<Stage>,
        phase: Phase,
        outcome: &str,
        deadline: Instant,
        detail: Option<String>,
    ) -> Result<()> {
        ensure!(self.sequence < MAX_EVENTS, "visual diagnostic event bound");
        let metadata = private_root(&self.root)?.metadata()?;
        ensure!(
            (metadata.dev(), metadata.ino()) == self.root_identity,
            "visual diagnostic root changed"
        );
        let now = Instant::now();
        let detail = detail.map(|text| text.chars().take(MAX_ERROR_CHARS).collect::<String>());
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": 1, "binding": self.binding, "sequence": self.sequence,
            "stage": stage, "phase": phase, "outcome": outcome,
            "elapsed_ms": now.saturating_duration_since(self.start).as_millis(),
            "remaining_ms": deadline.saturating_duration_since(now).as_millis(),
            "detail": detail, "native_authority": false,
        }))?;
        ensure!(bytes.len() <= 4096, "visual diagnostic record bound");
        write_new(
            &self.root,
            &format!("visual-diagnostic-{:03}.json", self.sequence),
            &bytes,
        )?;
        self.sequence += 1;
        Ok(())
    }
    /// Retain entry before invoking the operation and its error before returning.
    /// Checks after recording still use the original deadline.
    pub fn step<T>(
        &mut self,
        stage: Option<Stage>,
        phase: Phase,
        deadline: Instant,
        action: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.record(stage, phase, "entered", deadline, None)?;
        let result = check(deadline).and_then(|()| action(self));
        let retained = self.record(
            stage,
            phase,
            if result.is_ok() { "returned" } else { "error" },
            deadline,
            result.as_ref().err().map(|error| format!("{error:#}")),
        );
        match (result, retained) {
            (Err(error), Err(retention)) => {
                Err(error.context(format!("diagnostic retention failed: {retention:#}")))
            }
            (Ok(value), Ok(())) => {
                // Retention consumes the existing operation budget too. A
                // returned record is historical, never permission to continue
                // after its fsync exhausted that original deadline.
                if let Err(error) = check(deadline) {
                    let error =
                        error.context("visual diagnostic retention exhausted original deadline");
                    if let Err(retention) =
                        self.record(stage, phase, "error", deadline, Some(format!("{error:#}")))
                    {
                        return Err(
                            error.context(format!("diagnostic retention failed: {retention:#}"))
                        );
                    }
                    return Err(error);
                }
                Ok(value)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }
    /// Diagnostic refusal must never bypass owned-child cleanup or renew its bound.
    pub fn shutdown<T: std::fmt::Debug>(
        &mut self,
        deadline: Instant,
        action: impl FnOnce() -> T,
    ) -> T {
        let _ = self.record(None, Phase::Shutdown, "entered", deadline, None);
        let result = action();
        let _ = self.record(
            None,
            Phase::Shutdown,
            "returned",
            deadline,
            Some(format!("{result:?}")),
        );
        result
    }
}
