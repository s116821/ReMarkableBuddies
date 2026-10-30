//! Capture once and compare models against identical, non-mutating input.
use anyhow::{bail, ensure, Context, Result};
use remarkable_reader_buddy::{
    workflow::orchestrator::ANALYSIS_PROMPT, LLMEngine, OpenAI, Screenshot,
};
use serde::{Deserialize, Serialize};
use std::{fs, time::Instant};

#[derive(Serialize, Deserialize)]
struct Fixture {
    prompt: String,
    images: Vec<String>,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("capture") => {
            let path = args.get(2).context("capture OUTPUT.json")?;
            let mut screenshot = Screenshot::new()?;
            screenshot.take_screenshot()?;
            let mut images = vec![screenshot.base64()?];
            images.extend(screenshot.detail_images_base64()?);
            fs::write(
                path,
                serde_json::to_vec(&Fixture {
                    prompt: ANALYSIS_PROMPT.into(),
                    images,
                })?,
            )?;
            screenshot.save_image(&format!("{path}.png"))?;
            println!("Captured fixture with overview and full-width details");
        }
        Some("evaluate") => {
            let path = args
                .get(2)
                .context("evaluate FIXTURE.json MODEL RESULT.json")?;
            let model = args.get(3).context("model required")?;
            let output = args.get(4).context("result path required")?;
            let fixture: Fixture = serde_json::from_slice(&fs::read(path)?)?;
            ensure!(
                fixture.prompt == ANALYSIS_PROMPT,
                "Fixture has a different prompt"
            );
            let mut llm = OpenAI::from_env(Some(model.clone()))?;
            llm.add_text_content(ANALYSIS_PROMPT);
            for image in fixture.images {
                llm.add_image_content(&image);
            }
            let start = Instant::now();
            let response = llm.execute()?;
            let elapsed = start.elapsed().as_secs_f64();
            fs::write(
                output,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "model_requested": model,
                    "elapsed_seconds": elapsed,
                    "response": response,
                }))?,
            )?;
            println!("Evaluation completed in {elapsed:.2}s; result saved");
        }
        _ => bail!("Use capture OUTPUT.json or evaluate FIXTURE.json MODEL RESULT.json"),
    }
    Ok(())
}
