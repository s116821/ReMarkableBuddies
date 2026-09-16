use anyhow::Result;
use log::{debug, error, info};

use super::{AnswerPageType, Workflow};
use crate::analysis::BoundingBox;
use crate::llm::{openai::OpenAI, LLMEngine};

/// Shared by the live workflow and bounded vision-comparison helper.
pub const ANALYSIS_PROMPT: &str =
    "Read the handwritten question annotation on this tablet page and answer it from \
             the technical concept selected by the hand-drawn outline. Handwriting may be neat \
             uppercase block letters, connected cursive, or abbreviated shorthand. \
             Questions may appear in the margins or above the printed document title.\n\
             The first image is the full-page overview; \
             the following three images are overlapping full-width detail strips in \
             top-to-bottom order. They show the SAME page.\n\
             Transcribe the ink question as written, retaining abbreviations. Distinguish \
             these annotations from the document's typeset prose, equations and headings. \
             Do not turn printed source text into a question. If there is no readable question \
             annotation, no closed outline, or the question's meaning is ambiguous, return NONE. \
             Do not guess missing question words or confidently answer an uncertain reading. \
             Cross-check the question in the overview and full-width detail before answering. \
             Resolve shorthand only if its meaning is clear from the visible words and source. \
             If any essential word or abbreviation is unreadable or has competing meanings, \
             return NONE. A plausible transcription alone is not enough: you must understand \
             exactly what the annotation is asking. Never substitute a general summary of the \
             passage for an answer to an unclear question.\n\
             The outline selects the topic, not a restriction on sources. You may use the \
             surrounding page and general knowledge to explain that topic and answer the \
             actual handwritten question. Distinguish general explanation from claims about \
             this paper. For paper-specific values, preserve visible numbers, uncertainties \
             and units rather than substituting remembered values or inventing measurements. \
             If a source detail is unreadable, say so rather than guessing.\n\
             A question inside an outline is also valid. An X mark is not a closed outline.\n\
             Reply in this exact format, with coordinates in the overview's 768x1024 space:\n\
             QUESTION: [question]\n\
             QUESTION_BOX: x,y,width,height\n\
             OUTLINE_BOX: x,y,width,height\n\
             ---\n\
             ANSWER: [concise answer about the selected concept]\n\
             Use plain ASCII notation: +/- for uncertainty, * for multiplication, ^ for powers, \
             spelled-out Greek letters. No LaTeX or Markdown.";

/// Result from LLM analysis containing question, answer, and bounding boxes
struct AnalysisResult {
    question: String,
    answer: String,
    _question_box: Option<BoundingBox>,
    _outline_box: Option<BoundingBox>,
    _screenshot_data: Vec<u8>, // PNG data for downstream processing (reserved for future use)
}

/// High-level orchestrator for the complete workflow
pub struct Orchestrator {
    workflow: Workflow,
    llm: OpenAI,
    trigger_enabled: bool,
}

impl Orchestrator {
    pub fn new(workflow: Workflow, llm: OpenAI) -> Self {
        Self {
            workflow,
            llm,
            trigger_enabled: true,
        }
    }

    pub fn set_trigger_enabled(&mut self, enabled: bool) {
        self.trigger_enabled = enabled;
    }

    /// Run one complete iteration of the reader buddy workflow
    /// NOTE: v0.1 processes ONE outline-question pair per trigger
    pub fn run_iteration(&mut self) -> Result<()> {
        info!("=== Starting Reader Buddy Iteration ===");

        // Step 1: Wait for trigger
        if self.trigger_enabled {
            self.workflow.wait_for_trigger()?;
        }

        // Step 2: Capture screenshot (of current/question page)
        let (screenshot_base64, screenshot_png_data) =
            self.workflow.capture_screenshot_with_data()?;

        // Step 3: Propose a question and answer, then independently verify
        // the question before navigating or writing:
        // - Detect outlined region
        // - Extract question text
        // - Generate answer
        let result = self.analyze_and_answer(&screenshot_base64, screenshot_png_data)?;

        match result {
            None => {
                info!("No outlined regions or questions detected");
                // Draw failure X on current page (no text output)
                self.workflow.draw_failure_x()?;
                return Ok(());
            }
            Some(result) => {
                if !self.verify_question(&result)? {
                    info!("Independent question reading disagreed or was uncertain; no answer written");
                    self.workflow.draw_failure_x()?;
                    return Ok(());
                }
                info!(
                    "Got Q&A - Question: {} | Answer: {}",
                    result.question, result.answer
                );

                if let Err(e) = self.render_answer(&result) {
                    error!("Error rendering answer: {}", e);
                    // On error, draw failure X (no text output)
                    self.workflow.draw_failure_x()?;
                }
            }
        }

        info!("=== Iteration Complete ===");
        Ok(())
    }

    /// First pass:
    /// 1. Detects outlined content
    /// 2. Extracts handwritten question
    /// 3. Generates answer
    /// 4. Provides bounding boxes
    ///
    /// Returns None if no outline/question found, or Some((question, answer, question_box, outline_box))
    fn analyze_and_answer(
        &mut self,
        screenshot_base64: &str,
        screenshot_png_data: Vec<u8>,
    ) -> Result<Option<AnalysisResult>> {
        info!("Sending analysis + answer proposal");

        self.llm.clear_content();
        self.llm.add_text_content(ANALYSIS_PROMPT);
        self.llm.add_image_content(screenshot_base64);
        for detail in self.workflow.screenshot.detail_images_base64()? {
            self.llm.add_image_content(&detail);
        }

        let response = self.llm.execute()?;
        info!("LLM Response: {}", response);
        Ok(Self::parse_analysis_response(
            &response,
            screenshot_png_data,
        ))
    }

    fn parse_analysis_response(
        response: &str,
        screenshot_png_data: Vec<u8>,
    ) -> Option<AnalysisResult> {
        // Parse the response
        if response.trim().to_uppercase().starts_with("NONE") {
            return None;
        }

        // Parse the structured response
        // Missing fields or explicit abstention must never become typed Q&A.
        let (header, body) = response.split_once("---")?;
        let answer_text = body.trim().strip_prefix("ANSWER:")?.trim();

        // Extract question text
        let question_text = Self::extract_field(header, "QUESTION:");
        if [question_text.as_str(), answer_text]
            .iter()
            .any(|value| value.is_empty() || value.eq_ignore_ascii_case("NONE"))
        {
            return None;
        }

        // Extract bounding boxes
        let question_box = Self::parse_bounding_box(&Self::extract_field(header, "QUESTION_BOX:"));
        let outline_box = Self::parse_bounding_box(&Self::extract_field(header, "OUTLINE_BOX:"));

        debug!("Parsed - Question: {}", question_text);
        debug!("Question box: {:?}", question_box);
        debug!("Outline box: {:?}", outline_box);

        Some(AnalysisResult {
            question: question_text,
            answer: answer_text.to_string(),
            _question_box: question_box,
            _outline_box: outline_box,
            _screenshot_data: screenshot_png_data,
        })
    }

    fn verify_question(&mut self, result: &AnalysisResult) -> Result<bool> {
        let Some(bounds) = &result._question_box else {
            return Ok(false);
        };
        if !(0..1024).contains(&bounds.y) || bounds.height <= 0 {
            return Ok(false);
        }
        self.llm.clear_content();
        self.llm.add_text_content(
            "Transcribe only the handwritten question annotation in these images of the same page. Typeset \
             document text is not a handwritten question. Preserve the visible words and \
             abbreviations; shorthand and fragments are valid annotations and need not form \
             a complete sentence. Read the written letters without requiring an expanded \
             meaning. Do not expand or guess them from printed context. Do not answer \
             the question. If an essential handwritten word is uncertain, overstruck or \
             unreadable, or there is no handwritten question, return only NONE. Otherwise \
             return one line: TRANSCRIPTION: [exact handwritten text].",
        );
        self.llm
            .add_image_content(&self.workflow.screenshot.base64()?);
        for detail in self.workflow.screenshot.detail_images_base64()? {
            self.llm.add_image_content(&detail);
        }
        let reading = match self.llm.execute() {
            Ok(reading) => reading,
            Err(err) => {
                log::warn!("Question verification unavailable: {}", err);
                return Ok(false);
            }
        };
        info!("Independent question reading: {}", reading);
        Ok(Self::transcriptions_agree(&result.question, &reading))
    }

    fn transcriptions_agree(question: &str, reading: &str) -> bool {
        let independent = Self::extract_field(reading, "TRANSCRIPTION:");
        let normalize = |text: &str| -> String {
            let chars: Vec<char> = text.to_lowercase().chars().collect();
            let mut normalized = String::new();
            for (i, &c) in chars.iter().enumerate() {
                let numeric_separator = matches!(c, '.' | ',')
                    && i > 0
                    && chars[i - 1].is_ascii_digit()
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit);
                if c.is_alphanumeric() || numeric_separator {
                    normalized.push(c);
                } else if c.is_whitespace() || ".?,;:".contains(c) {
                    normalized.push(' ');
                } else {
                    // Keep operators/grouping distinct, regardless of spacing.
                    normalized.push(' ');
                    normalized.push(c);
                    normalized.push(' ');
                }
            }
            normalized.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let a = normalize(question);
        let b = normalize(&independent);
        !a.is_empty() && !b.is_empty() && b != "none" && a == b
    }

    /// Extract a field value from the response
    fn extract_field(text: &str, field_name: &str) -> String {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix(field_name) {
                return value.trim().to_string();
            }
        }
        "".to_string()
    }

    /// Parse bounding box from "x,y,width,height" format
    fn parse_bounding_box(text: &str) -> Option<BoundingBox> {
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() == 4 {
            if let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
                parts[0].trim().parse::<i32>(),
                parts[1].trim().parse::<i32>(),
                parts[2].trim().parse::<i32>(),
                parts[3].trim().parse::<i32>(),
            ) {
                return Some(BoundingBox {
                    x,
                    y,
                    width: w,
                    height: h,
                });
            }
        }
        None
    }

    /// Render the answer on the next page
    ///
    /// Simplified flow:
    /// 1. Store original page screenshot for later comparison
    /// 2. Navigate right to next page  
    /// 3. Compare to original to verify we actually moved
    /// 4. Check if page is valid (blank or existing QA page)
    /// 5. If not valid or didn't move → ensure we're on original and draw X
    /// 6. If valid → render Q&A on that page
    fn render_answer(&mut self, result: &AnalysisResult) -> Result<()> {
        info!("Attempting to render Q&A on next page");

        // Step 1: Store original page screenshot for comparison
        let original_img = self.workflow.capture_page()?;

        // Step 2: Attempt to navigate to next page
        self.workflow.navigate_to_next_page()?;
        std::thread::sleep(std::time::Duration::from_millis(800));

        if self.workflow.verify_navigation_to(&original_img)? {
            info!("No page movement detected; drawing X on original");
            self.workflow.draw_failure_x()?;
            return Ok(());
        }

        // Step 4: Check if the page we navigated to is valid (blank or QA)
        let page_type = self.workflow.is_valid_answer_page()?;

        match page_type {
            AnswerPageType::Invalid => {
                // Page exists but is not suitable - return to original
                info!(
                    "Next page is not valid (not blank and not a QA page) - returning to original"
                );

                // Navigate back and verify we're on original
                self.workflow.return_to_original_page(&original_img)?;
                self.workflow.draw_failure_x()?;
                return Ok(());
            }
            AnswerPageType::Blank => {
                // Step 5a: Blank page - render header first, then Q&A
                info!("Blank page found, rendering header and Q&A");

                // Switch to body text mode once before all rendering
                self.workflow.set_body_text_mode()?;

                // Header with two blank lines before first Q&A block
                self.workflow
                    .render_text("=== Reader Buddy Answers ===\n\n\n")?;

                // Save header pattern for future detection (only on first blank page)
                std::thread::sleep(std::time::Duration::from_millis(500));
                self.workflow.screenshot.take_screenshot()?;
                let new_png = self.workflow.screenshot.get_image_data();
                if let Ok(new_img) = image::load_from_memory(new_png) {
                    const HEADER_HEIGHT: u32 = 150; // Capture full header region from top
                    let header_img = new_img.crop_imm(
                        0,
                        0,
                        new_img.width(),
                        HEADER_HEIGHT.min(new_img.height()),
                    );
                    if let Err(e) = self.workflow.save_header_pattern(&header_img) {
                        log::warn!("Failed to save header pattern: {}", e);
                    }
                }
            }
            AnswerPageType::ExistingQA => {
                // Step 5b: Existing QA page - just append Q&A content (no header)
                info!("Existing QA page found, appending Q&A (no header needed)");

                // Switch to body text mode
                self.workflow.set_body_text_mode()?;
            }
        }

        // Render the Q&A
        let formatted_output = Workflow::compose_qa(&result.question, &result.answer);

        self.workflow.render_text(&formatted_output)?;

        info!("Q&A rendered successfully");
        Ok(())
    }

    /// Run the main loop
    pub fn run_loop(&mut self) -> Result<()> {
        info!("Starting Reader Buddy main loop");

        loop {
            match self.run_iteration() {
                Ok(_) => info!("Iteration completed successfully"),
                Err(e) => {
                    error!("Error in iteration: {}", e);
                    // Try to show error to user
                    let _ = self.workflow.set_body_text_mode();
                    let _ = self.workflow.render_text(&format!("Error: {}\n", e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Orchestrator;

    #[test]
    fn independent_transcription_requires_the_same_words() {
        assert!(Orchestrator::transcriptions_agree(
            "G unc.?",
            "TRANSCRIPTION: g unc?"
        ));
        assert!(!Orchestrator::transcriptions_agree(
            "why not atom?",
            "TRANSCRIPTION: why rot. attr.?"
        ));
        assert!(!Orchestrator::transcriptions_agree("G unc?", "NONE"));
        assert!(!Orchestrator::transcriptions_agree(
            "G unc?",
            "TRANSCRIPTION: NONE"
        ));
        assert!(!Orchestrator::transcriptions_agree(
            "G unc?",
            "TRANSCRIPTION: G value?"
        ));
        assert!(!Orchestrator::transcriptions_agree("", "TRANSCRIPTION:"));
        assert!(Orchestrator::transcriptions_agree(
            "Is G=5?",
            "TRANSCRIPTION: is G = 5?"
        ));
        for (first, second) in [
            ("2+2?", "2-2?"),
            ("Is G=1.0?", "Is G=10?"),
            ("why not able?", "why notable?"),
            ("(2+3)*4?", "2+3*4?"),
        ] {
            assert!(!Orchestrator::transcriptions_agree(
                first,
                &format!("TRANSCRIPTION: {second}")
            ));
        }
    }

    #[test]
    fn abstention_and_malformed_responses_never_render() {
        for response in [
            "NONE",
            "QUESTION: NONE\nQUESTION_BOX: 0,0,0,0\nOUTLINE_BOX: 0,0,0,0\n---\nANSWER: NONE",
            "QUESTION: readable?\n---\nANSWER: none",
            "QUESTION: NONE\n---\nANSWER: A plausible summary",
            "A plausible summary without a question",
            "QUESTION: readable?\n---\nAn unlabelled answer",
            "QUESTION_BOX: 1,2,3,4\n---\nANSWER: 4",
        ] {
            assert!(Orchestrator::parse_analysis_response(response, vec![]).is_none());
        }
    }

    #[test]
    fn valid_indented_response_preserves_question_and_complete_answer() {
        let response = "  QUESTION: G unc.?\n  QUESTION_BOX: 1,2,3,4\n  OUTLINE_BOX: 5,6,7,8\n---\n  ANSWER: G = (6.674215 +/- 0.000092) * 10^-11 m^3 kg^-1 s^-2.\n---\nExtra answer line.";
        let result = Orchestrator::parse_analysis_response(response, vec![]).unwrap();
        assert_eq!(result.question, "G unc.?");
        assert!(result.answer.ends_with("---\nExtra answer line."));
        assert!(result._question_box.is_some());
    }
}
