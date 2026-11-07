pub mod openai;

use anyhow::Result;

/// Invoked at owned submission, never reconstructed from request JSON. The
/// caller holds its complete ordered admission only while submit runs.
pub type ModelHandoff<'a> = dyn FnMut(&mut dyn FnMut() -> Result<()>) -> Result<()> + 'a;

pub trait LLMEngine {
    /// Guarded adapters emit periodic progress no faster than indicator::CADENCE,
    /// plus at most four preflight/completion calls. Unsupported adapters return
    /// None. This is a timing contract, not permission to submit a request.
    fn guarded_timeout(&self) -> Option<std::time::Duration> {
        None
    }
    fn add_text_content(&mut self, text: &str);
    fn add_image_content(&mut self, base64_image: &str);
    fn clear_content(&mut self);
    fn execute(&mut self) -> Result<String>;
    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        progress()?;
        self.execute()
    }
    fn execute_with_handoff(
        &mut self,
        _progress: &mut dyn FnMut() -> Result<()>,
        _handoff: &mut ModelHandoff<'_>,
    ) -> Result<String> {
        anyhow::bail!("model adapter does not support guarded submission")
    }
}
