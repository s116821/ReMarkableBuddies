pub mod history;
pub mod indicator;
mod navigation;
pub mod orchestrator;
pub mod symbol_pool;
pub mod xochitl_integration;

pub use navigation::ReturnOutcome;

use anyhow::Result;
use log::{debug, info};

use crate::device::backend::{DeviceBackend, Frame, RealDevice};
use crate::device::screenshot::{SCREENSHOT_VIRTUAL_HEIGHT, SCREENSHOT_VIRTUAL_WIDTH};

/// Result of checking if a page is valid for rendering answers
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnswerPageType {
    /// Page is blank - needs header to be rendered
    Blank,
    /// Page already has QA header - just append Q&A content
    ExistingQA,
    /// Page is not valid for answers
    Invalid,
}

// Image comparison mask constants - skip UI elements that can change between screenshots
// These are in virtual coordinates (768x1024) and work for all devices since screenshots are normalized
/// Skip left toolbar area during image comparisons
pub const MASK_LEFT_OFFSET: u32 = 298;
/// Skip right side during image comparisons (scrollbar, UI elements)
pub const MASK_RIGHT_OFFSET: u32 = 125;
/// Skip top area during image comparisons (toolbar can be docked at top)
pub const MASK_TOP_OFFSET: u32 = 70;
/// Skip bottom HUD area during image comparisons
pub const MASK_BOTTOM_OFFSET: u32 = 70;

/// Default sampling rate for image comparisons (every Nth pixel)
const DEFAULT_SAMPLE_RATE: u32 = 5;
/// Sampling rate for blank page detection (more accurate, every 2nd pixel)
const BLANK_PAGE_SAMPLE_RATE: u32 = 2;

/// Main workflow coordinator
pub struct Workflow {
    device: Box<dyn DeviceBackend>,
    frame: Frame,
    debug_dump: bool,
    iteration_count: u32,
    indicator_eligible: bool,
    indicator_paths: Vec<indicator::Stroke>,
    indicator_stage: indicator::Stage,
    indicator_target: indicator::Stage,
    indicator_edges: u8,
    indicator_auxiliary: bool,
    indicator_deadline: Option<std::time::Duration>,
    failure_attempted: bool,
    indicator_cleanup_failed: bool,
    indicator_style_active: bool,
    history: history::History,
}

impl Workflow {
    pub fn new(
        no_draw: bool,
        trigger_corner: crate::device::touch::TriggerCorner,
        debug_dump: bool,
    ) -> Result<Self> {
        Ok(Self::with_device(
            Box::new(RealDevice::new(no_draw, trigger_corner, debug_dump)?),
            debug_dump,
        ))
    }

    pub fn with_device(device: Box<dyn DeviceBackend>, debug_dump: bool) -> Self {
        Self {
            device,
            frame: Frame::default(),
            debug_dump,
            iteration_count: 0,
            indicator_eligible: false,
            indicator_paths: Vec::new(),
            indicator_stage: indicator::Stage::Preparing,
            indicator_target: indicator::Stage::Preparing,
            indicator_edges: 0,
            indicator_auxiliary: false,
            indicator_deadline: None,
            failure_attempted: false,
            indicator_cleanup_failed: false,
            indicator_style_active: false,
            history: history::History::default(),
        }
    }

    pub fn delay(&mut self, duration: std::time::Duration) {
        self.device.delay(duration);
    }

    pub fn set_indicator_stage(&mut self, stage: indicator::Stage) {
        self.indicator_target = stage;
        self.indicator_auxiliary = false;
    }

    pub fn finish_indicator_stage(&mut self) -> Result<()> {
        if !self.indicator_eligible {
            return Ok(());
        }
        while self.indicator_eligible
            && (self.indicator_stage < self.indicator_target || self.indicator_edges < 3)
        {
            self.tick_indicator()?;
        }
        Ok(())
    }

    pub fn auxiliary_indicator(&mut self) -> Result<()> {
        self.finish_indicator_stage()?;
        self.indicator_auxiliary = true;
        Ok(())
    }

    pub fn tick_indicator(&mut self) -> Result<()> {
        self.invalidate_history();
        anyhow::ensure!(
            !self.indicator_cleanup_failed,
            "Status cleanup failed; further activity is stopped"
        );
        if !self.indicator_eligible {
            self.device.status_suppressed();
            return Ok(());
        }
        if !self.begin_indicator_style()? {
            return Ok(());
        }
        if let Some(deadline) = self.indicator_deadline {
            self.device
                .delay(deadline.saturating_sub(self.device.monotonic()));
        }
        if self.indicator_edges >= 3 && self.indicator_stage < self.indicator_target {
            self.indicator_stage = self.indicator_stage.next();
            self.indicator_edges = 0;
        }
        let stroke = if self.indicator_auxiliary {
            indicator::Stroke::Auxiliary(self.indicator_stage)
        } else {
            indicator::Stroke::Edge(self.indicator_stage, self.indicator_edges % 3)
        };
        // Record ownership before input: even a failed draw can leave ink.
        if !self.indicator_paths.contains(&stroke) {
            self.indicator_paths.push(stroke);
        }
        let started = self.device.monotonic();
        if let Err(error) = self.device.status_stroke(stroke) {
            self.clear_indicator()?;
            return Err(error);
        }
        self.indicator_deadline = Some(indicator::next_deadline(started, self.device.monotonic()));
        if !self.indicator_auxiliary {
            self.indicator_edges = if self.indicator_edges < 3 {
                self.indicator_edges + 1
            } else {
                3 + (self.indicator_edges + 1) % 3
            };
        }
        Ok(())
    }

    pub fn clear_indicator(&mut self) -> Result<()> {
        anyhow::ensure!(
            !self.indicator_cleanup_failed,
            "Status cleanup/restoration already failed; no further input permitted"
        );
        if !self.indicator_paths.is_empty() {
            self.invalidate_history();
            if let Err(error) = self.device.status_clear(&self.indicator_paths) {
                self.indicator_cleanup_failed = true;
                return Err(error.context("Clear owned activity paths"));
            }
            self.indicator_paths.clear();
        }
        self.end_indicator_style()?;
        self.indicator_deadline = None;
        self.indicator_edges = 0;
        self.indicator_stage = self.indicator_target;
        self.indicator_auxiliary = false;
        Ok(())
    }

    fn begin_indicator_style(&mut self) -> Result<bool> {
        if self.indicator_style_active {
            return Ok(true);
        }
        match self.device.status_style_begin() {
            Ok(true) => {
                self.indicator_style_active = true;
                Ok(true)
            }
            Ok(false) => {
                self.indicator_eligible = false;
                self.device.status_suppressed();
                Ok(false)
            }
            Err(error) => {
                self.indicator_cleanup_failed = true;
                Err(error)
            }
        }
    }

    fn end_indicator_style(&mut self) -> Result<()> {
        if self.indicator_style_active {
            if let Err(error) = self.device.status_style_end() {
                self.indicator_cleanup_failed = true;
                return Err(error);
            }
            self.indicator_style_active = false;
        }
        Ok(())
    }

    pub fn cleanup_failed(&self) -> bool {
        self.indicator_cleanup_failed
    }

    /// Conflicting navigation evidence cannot authorize even a failure mark.
    pub(super) fn invalidate_captured_viewport(&mut self) {
        self.indicator_eligible = false;
    }

    pub fn begin_iteration(&mut self) -> Result<()> {
        self.invalidate_history();
        anyhow::ensure!(
            !self.indicator_cleanup_failed,
            "Status cleanup failed; restart before another iteration"
        );
        self.clear_indicator()?;
        self.indicator_eligible = false;
        self.indicator_stage = indicator::Stage::Preparing;
        self.indicator_target = indicator::Stage::Preparing;
        self.failure_attempted = false;
        Ok(())
    }

    fn capture_clean(&mut self) -> Result<()> {
        self.clear_indicator()?;
        self.indicator_eligible = false;
        self.frame = self.device.capture()?;
        self.indicator_eligible =
            image::load_from_memory(&self.frame.png).is_ok_and(|image| indicator::eligible(&image));
        Ok(())
    }

    pub fn detail_images_base64(&self) -> Result<Vec<String>> {
        self.device.detail_images()
    }

    pub fn current_image_base64(&self) -> String {
        self.frame.base64()
    }

    /// Wait for user to trigger the workflow (touch in corner)
    pub fn wait_for_trigger(&mut self) -> Result<()> {
        info!("Waiting for trigger...");
        self.device.wait_for_trigger()?;
        Ok(())
    }

    /// Stay idle without clearing the last Q&A. Cross-device event order is
    /// unknown, so any loss of ownership wins over history gestures in a batch.
    pub fn wait_for_reader(&mut self) -> Result<()> {
        self.wait_for_reader_until(None)
    }

    /// Bounded offline diagnostics use the exact production idle dispatcher.
    pub fn wait_for_reader_bounded(&mut self, seconds: u64) -> Result<()> {
        anyhow::ensure!((1..=120).contains(&seconds), "Invalid observation duration");
        self.wait_for_reader_until(Some(
            std::time::Instant::now() + std::time::Duration::from_secs(seconds),
        ))
    }

    fn wait_for_reader_until(&mut self, deadline: Option<std::time::Instant>) -> Result<()> {
        use crate::device::interaction::Interaction;
        loop {
            let timeout =
                deadline.map(|limit| limit.saturating_duration_since(std::time::Instant::now()));
            let events = self.device.wait_for_interactions(timeout)?;
            let invalidated = events.contains(&Interaction::Invalidated);
            if invalidated {
                self.invalidate_history();
            }
            if events.contains(&Interaction::Reader) {
                self.invalidate_history();
                return Ok(());
            }
            if !invalidated {
                for event in events {
                    let action = match event {
                        Interaction::Undo => history::Action::Undo,
                        Interaction::Redo => history::Action::Redo,
                        _ => continue,
                    };
                    match self.history_action(action) {
                        Ok(true) => info!("Q&A history {:?}: {:?}", action, self.history.state()),
                        Ok(false) => debug!("Q&A history {:?}: unavailable or redundant", action),
                        Err(error) => log::warn!("Q&A history stopped: {error}"),
                    }
                }
            }
        }
    }

    /// Take a screenshot and return the base64-encoded image
    pub fn capture_screenshot(&mut self) -> Result<String> {
        info!("Capturing screenshot...");
        self.capture_clean()?;
        Ok(self.frame.base64())
    }

    /// Take a screenshot and return both base64 and raw PNG data
    pub fn capture_screenshot_with_data(&mut self) -> Result<(String, Vec<u8>)> {
        info!("Capturing screenshot...");
        self.capture_clean()?;
        let base64 = self.frame.base64();
        let png_data = self.frame.png.clone();

        // Debug dump if enabled
        if self.debug_dump {
            self.iteration_count += 1;
            let filename = format!(
                "/tmp/reader-buddy-screenshot-{:03}.png",
                self.iteration_count
            );
            if let Err(e) = std::fs::write(&filename, &self.frame.png) {
                log::warn!("Failed to save debug screenshot: {}", e);
            } else {
                log::debug!("Saved debug screenshot to {}", filename);
            }
        }

        Ok((base64, png_data))
    }

    /// Show progress indicator to user
    pub fn show_progress(&mut self, message: &str) -> Result<()> {
        self.invalidate_history();
        self.device.progress(Some(message))?;
        Ok(())
    }

    /// Clear progress indicator
    pub fn clear_progress(&mut self) -> Result<()> {
        self.invalidate_history();
        self.device.progress(None)?;
        Ok(())
    }

    /// Erase a region on the screen using the eraser tool
    pub fn erase_region(&mut self, region: &crate::analysis::BoundingBox) -> Result<()> {
        self.invalidate_history();
        info!(
            "Erasing region at ({}, {}) size {}x{}",
            region.x, region.y, region.width, region.height
        );

        let top_left = (region.x, region.y);
        let bottom_right = (region.x + region.width, region.y + region.height);

        // Use the eraser tool to erase the rectangle
        self.device.erase(top_left, bottom_right)?;

        Ok(())
    }

    /// Smart erase that only erases detected ink pixels within the region
    pub fn erase_region_smart(
        &mut self,
        region: &crate::analysis::BoundingBox,
        screenshot_data: &[u8],
    ) -> Result<()> {
        self.invalidate_history();
        use image::Rgba;

        info!(
            "Smart erasing region at ({}, {}) size {}x{}",
            region.x, region.y, region.width, region.height
        );

        // Load the screenshot
        let img = image::load_from_memory(screenshot_data)?;
        let gray_img = img.to_luma8();

        // Define ink detection threshold (darker pixels are ink)
        const INK_THRESHOLD: u8 = 200; // Pixels darker than this are considered ink
        const MARGIN: i32 = 2; // Add margin around detected ink

        // Scan the region and identify rows with ink
        let mut rows_with_ink = Vec::new();
        for y in region.y..(region.y + region.height).min(1024) {
            if y < 0 || y >= gray_img.height() as i32 {
                continue;
            }

            let mut has_ink = false;
            for x in region.x..(region.x + region.width).min(768) {
                if x < 0 || x >= gray_img.width() as i32 {
                    continue;
                }

                let pixel = gray_img.get_pixel(x as u32, y as u32);
                if pixel[0] < INK_THRESHOLD {
                    has_ink = true;
                    break;
                }
            }

            if has_ink {
                rows_with_ink.push(y);
            }
        }

        debug!(
            "Found {} rows with ink out of {} total rows",
            rows_with_ink.len(),
            region.height
        );

        // Debug dump if enabled - show erase mask overlay
        if self.debug_dump {
            let mut debug_img = img.to_rgba8();
            // Draw red box around the region
            for x in region.x.max(0)..((region.x + region.width).min(768)) {
                if x >= 0 && x < debug_img.width() as i32 {
                    if region.y >= 0 && region.y < debug_img.height() as i32 {
                        debug_img.put_pixel(x as u32, region.y as u32, Rgba([255, 0, 0, 255]));
                    }
                    let bottom_y =
                        (region.y + region.height - 1).min(debug_img.height() as i32 - 1);
                    if bottom_y >= 0 && bottom_y < debug_img.height() as i32 {
                        debug_img.put_pixel(x as u32, bottom_y as u32, Rgba([255, 0, 0, 255]));
                    }
                }
            }
            // Highlight rows to be erased in yellow
            for &y in &rows_with_ink {
                for x in region.x.max(0)..((region.x + region.width).min(768)) {
                    if x >= 0
                        && x < debug_img.width() as i32
                        && y >= 0
                        && y < debug_img.height() as i32
                    {
                        debug_img.put_pixel(x as u32, y as u32, Rgba([255, 255, 0, 128]));
                    }
                }
            }
            let filename = format!(
                "/tmp/reader-buddy-erase-mask-{:03}.png",
                self.iteration_count
            );
            if let Err(e) = debug_img.save(&filename) {
                log::warn!("Failed to save debug erase mask: {}", e);
            } else {
                log::debug!("Saved debug erase mask to {}", filename);
            }
        }

        // Erase rows with ink (with margin)
        for &y in &rows_with_ink {
            let erase_y_start = (y - MARGIN).max(region.y).max(0);
            let erase_y_end = (y + MARGIN + 1).min(region.y + region.height).min(1024);

            for erase_y in erase_y_start..erase_y_end {
                let top_left = (region.x, erase_y);
                let bottom_right = ((region.x + region.width).min(768), erase_y + 1);
                self.device.erase(top_left, bottom_right)?;
            }
        }

        Ok(())
    }

    /// Draw a reference symbol at a location using bitmap rendering
    pub fn draw_symbol(&mut self, x: i32, y: i32, symbol: &str) -> Result<()> {
        self.invalidate_history();
        info!("Drawing reference symbol '{}' at ({}, {})", symbol, x, y);

        // Convert symbol to bitmap - larger size for better visibility
        let size = 40; // Symbol size in pixels (increased from 20)
        let bitmap = symbol_pool::SymbolPool::symbol_to_bitmap(symbol, size);

        // Draw the bitmap at the specified location
        // Note: This draws the full bitmap starting at (x, y)
        // For centered placement, we'd offset by -size/2
        let offset_x = x - (size as i32 / 2);
        let offset_y = y - (size as i32 / 2);

        // Create a positioned bitmap by building a temporary full-size bitmap
        // This is not optimal but works for MVP
        let mut positioned_bitmap = vec![vec![false; 768]; 1024];
        for (dy, row) in bitmap.iter().enumerate() {
            for (dx, &pixel) in row.iter().enumerate() {
                let px = offset_x + dx as i32;
                let py = offset_y + dy as i32;
                if (0..768).contains(&px) && (0..1024).contains(&py) {
                    positioned_bitmap[py as usize][px as usize] = pixel;
                }
            }
        }

        self.device.bitmap(&positioned_bitmap)?;

        Ok(())
    }

    pub fn invalidate_history(&mut self) {
        self.history.discard();
        self.device.history_discard();
    }

    pub fn history_state(&self) -> history::State {
        self.history.state()
    }

    /// Registers only a completely persisted Q&A after all status/header/body
    /// operations. Observation failure does not retroactively fail rendered text.
    pub fn render_qa(&mut self, text: &str) -> Result<()> {
        self.invalidate_history();
        self.clear_indicator()?;
        let before = self.device.history_snapshot(None).unwrap_or_else(|error| {
            log::warn!("Q&A history preparation unavailable: {error}");
            None
        });
        self.indicator_eligible = false;
        if let Err(error) = self.device.render_text(text) {
            self.invalidate_history();
            return Err(error);
        }
        if let Some(before) = before {
            let expected = format!("{}{text}", before.content.text());
            match self.device.history_snapshot(Some(&expected)) {
                Ok(Some(applied)) => {
                    if !self.history.arm(before, applied, text) {
                        self.invalidate_history();
                    }
                }
                result => {
                    if let Err(error) = result {
                        log::warn!("Q&A history persistence unavailable: {error}");
                    }
                    self.invalidate_history();
                }
            }
        } else {
            self.device.history_discard();
        }
        Ok(())
    }

    /// History failures never type error text, draw an X or retry a native edit.
    pub fn history_action(&mut self, action: history::Action) -> Result<bool> {
        if self.history.state() == history::State::Empty {
            return Ok(false);
        }
        let current = match self.device.history_snapshot(None) {
            Ok(Some(state)) => state,
            Ok(None) => {
                self.invalidate_history();
                return Ok(false);
            }
            Err(error) => {
                self.invalidate_history();
                return Err(error);
            }
        };
        let Some(command) = self.history.begin(action, &current) else {
            if self.history.state() == history::State::Empty {
                self.device.history_discard();
            }
            return Ok(false);
        };
        let expected = self
            .history
            .pending_text()
            .expect("begin returned a pending command");
        let result = self.device.history_mutate(command, &expected);
        if let Err(error) = self.history.finish(result) {
            self.invalidate_history();
            return Err(error);
        }
        Ok(true)
    }

    /// Render text on the screen using the keyboard
    /// Note: The caller is responsible for including any desired newlines in the text
    pub fn render_text(&mut self, text: &str) -> Result<()> {
        self.invalidate_history();
        self.clear_indicator()?;
        // Typing can change the status region, including on a partial failure.
        self.indicator_eligible = false;
        info!("Rendering text: {}", text);
        self.device.render_text(text)?;
        Ok(())
    }

    /// Switch keyboard to body text mode (should be called once before rendering)
    pub fn set_body_text_mode(&mut self) -> Result<()> {
        self.invalidate_history();
        self.clear_indicator()?;
        // Even a partial mode change can alter the viewport.
        self.indicator_eligible = false;
        self.device.body_mode()?;
        Ok(())
    }

    /// Navigate to the next page (swipe left)
    pub fn navigate_to_next_page(
        &mut self,
    ) -> Result<crate::device::backend::NavigationCompletion> {
        self.invalidate_history();
        self.clear_indicator()?;
        self.indicator_eligible = false;
        self.device
            .navigate(xochitl_integration::NavigationDirection::Next)
    }

    /// Navigate back to the previous page (swipe right)
    pub fn navigate_to_previous_page(
        &mut self,
    ) -> Result<crate::device::backend::NavigationCompletion> {
        self.invalidate_history();
        self.clear_indicator()?;
        self.indicator_eligible = false;
        self.device
            .navigate(xochitl_integration::NavigationDirection::Previous)
    }

    /// Draw a guarded failure X in the same 50x50 status area.
    /// Used to indicate that no valid answer page was found
    pub fn draw_failure(&mut self, failure: indicator::Failure) -> Result<()> {
        if self.failure_attempted {
            return Ok(());
        }
        self.failure_attempted = true;
        self.invalidate_history();
        info!("Drawing failure X in bottom-right corner");

        self.clear_indicator()?;
        if !self.indicator_eligible || self.indicator_cleanup_failed {
            self.device.status_suppressed();
            return Ok(());
        }
        if !self.begin_indicator_style()? {
            return Ok(());
        }
        self.indicator_eligible = false;
        let (x_start, y_start, x_end, y_end) = (
            indicator::LEFT + indicator::X_INSET,
            indicator::TOP + indicator::X_INSET,
            indicator::RIGHT - indicator::X_INSET,
            indicator::BOTTOM - indicator::X_INSET,
        );

        // Draw two diagonal lines to form an X (using screen coordinates)
        // Line 1: top-left to bottom-right
        let result = (|| -> Result<()> {
            self.device.line((x_start, y_start), (x_end, y_end))?;

            // Line 2: top-right to bottom-left
            self.device.line((x_end, y_start), (x_start, y_end))?;
            let (from, to) = failure.segment();
            self.device.line(from, to)?;
            Ok(())
        })();
        self.end_indicator_style()?;
        result?;

        debug!(
            "Failure X drawn at ({}, {}) to ({}, {})",
            x_start, y_start, x_end, y_end
        );
        Ok(())
    }

    /// Check if the current page is valid for rendering answers
    /// A page is valid if it is either:
    /// 1. A blank page (very few ink pixels) - returns Blank
    /// 2. An existing Reader Buddy answer page (has our header pattern) - returns ExistingQA
    ///
    /// Returns the page type: Blank, ExistingQA, or Invalid
    pub fn is_valid_answer_page(&mut self) -> Result<AnswerPageType> {
        self.delay(std::time::Duration::from_millis(500));
        self.capture_clean()?;
        let img = match image::load_from_memory(&self.frame.png) {
            Ok(img) => img,
            Err(error) => {
                log::warn!("Failed to load screenshot for page check: {}", error);
                return Ok(AnswerPageType::Invalid);
            }
        };
        let saved = self.device.load_header();
        let page_type = Self::classify_answer_page(&img, saved.as_ref());
        info!("Answer page classification: {:?}", page_type);
        Ok(page_type)
    }

    pub fn capture_page(&mut self) -> Result<image::DynamicImage> {
        Ok(image::load_from_memory(&self.capture_page_data()?)?)
    }

    pub fn capture_page_data(&mut self) -> Result<Vec<u8>> {
        self.capture_clean()?;
        Ok(self.frame.png.clone())
    }

    pub fn verify_navigation_to(&mut self, original: &image::DynamicImage) -> Result<bool> {
        let current = self.capture_page()?;
        Ok(Self::is_same_page(original, &current))
    }

    /// Check the source before moving, then attempt at most one verified return.
    pub fn return_to_original_page(
        &mut self,
        original: &image::DynamicImage,
    ) -> Result<ReturnOutcome> {
        navigation::return_to_original_page(self, original)
    }

    pub fn is_same_page(original: &image::DynamicImage, current: &image::DynamicImage) -> bool {
        Self::compute_image_similarity_masked(
            original,
            current,
            MASK_LEFT_OFFSET,
            MASK_RIGHT_OFFSET,
            MASK_TOP_OFFSET,
            MASK_BOTTOM_OFFSET,
            DEFAULT_SAMPLE_RATE,
        ) >= 0.999
    }

    pub fn classify_answer_page(
        img: &image::DynamicImage,
        saved: Option<&image::DynamicImage>,
    ) -> AnswerPageType {
        let blank = Self::create_blank_image();
        if Self::compute_image_similarity_masked(
            img,
            &blank,
            MASK_LEFT_OFFSET,
            MASK_RIGHT_OFFSET,
            MASK_TOP_OFFSET,
            MASK_BOTTOM_OFFSET,
            BLANK_PAGE_SAMPLE_RATE,
        ) >= 0.998
        {
            return AnswerPageType::Blank;
        }
        if let Some(saved) = saved {
            let header = img.crop_imm(0, 0, img.width(), 150.min(img.height()));
            if Self::compute_image_similarity_masked(
                &header,
                saved,
                MASK_LEFT_OFFSET,
                MASK_RIGHT_OFFSET,
                MASK_TOP_OFFSET,
                0,
                DEFAULT_SAMPLE_RATE,
            ) >= 0.998
            {
                return AnswerPageType::ExistingQA;
            }
        }
        AnswerPageType::Invalid
    }

    pub fn compose_qa(
        question: &str,
        answer: &str,
        center: crate::analysis::SelectionCenter,
    ) -> String {
        format!("<Start of Q-A block for Q @ {center}>\nQ: {question}\n\nA: {answer}\n<End of Q-A block for Q @ {center}>\n")
    }

    /// Save the header pattern for future fast detection
    /// Should be called after successfully detecting an answer page via LLM
    pub fn save_header_pattern(&mut self, header_img: &image::DynamicImage) -> Result<()> {
        info!("Saving header pattern");

        self.device.save_header(header_img)?;
        debug!("Header pattern saved successfully");

        Ok(())
    }

    /// Compute similarity between two images with masking (returns 0.0-1.0, where 1.0 is identical)
    /// Skips left/right toolbar areas, top area (for docked toolbar), and bottom HUD area
    /// Note: For full-page comparisons, images are always 768x1024 (virtual coordinates)
    ///
    /// # Arguments
    /// * `sample_rate` - Sample every Nth pixel (lower = more accurate but slower)
    pub fn compute_image_similarity_masked(
        img1: &image::DynamicImage,
        img2: &image::DynamicImage,
        left_offset: u32,
        right_offset: u32,
        top_offset: u32,
        bottom_offset: u32,
        sample_rate: u32,
    ) -> f32 {
        let gray1 = img1.to_luma8();
        let gray2 = img2.to_luma8();

        if sample_rate == 0 || gray1.dimensions() != gray2.dimensions() {
            return 0.0;
        }

        // Get dimensions for offset calculations (needed for cropped images)
        let width = gray1.width();
        let height = gray1.height();
        if left_offset.saturating_add(right_offset) >= width
            || top_offset.saturating_add(bottom_offset) >= height
        {
            return 0.0;
        }
        let mut total_diff: u64 = 0;
        let mut pixel_count: u64 = 0;

        // Sample every Nth pixel for speed, with masking
        for (y, row) in gray1.enumerate_rows() {
            if y % sample_rate != 0 {
                continue;
            }
            for (x, _, pixel1) in row {
                if x % sample_rate != 0 {
                    continue;
                }
                // Skip left side
                if x < left_offset {
                    continue;
                }
                // Skip right side
                if x >= width - right_offset {
                    continue;
                }
                // Skip top area (toolbar can be docked at top)
                if y < top_offset {
                    continue;
                }
                // Skip bottom HUD area
                if y >= height - bottom_offset {
                    continue;
                }

                let pixel2 = gray2.get_pixel(x, y);
                let diff = (pixel1[0] as i32 - pixel2[0] as i32).unsigned_abs() as u64;
                total_diff += diff * diff;
                pixel_count += 1;
            }
        }

        if pixel_count == 0 {
            return 0.0;
        }

        let mse = total_diff as f32 / pixel_count as f32;
        let max_mse = 255.0 * 255.0;
        1.0 - (mse / max_mse).min(1.0)
    }

    /// Create a synthetic blank (white) image for comparison
    fn create_blank_image() -> image::DynamicImage {
        let white_img = image::GrayImage::from_pixel(
            SCREENSHOT_VIRTUAL_WIDTH,
            SCREENSHOT_VIRTUAL_HEIGHT,
            image::Luma([255u8]),
        );
        image::DynamicImage::ImageLuma8(white_img)
    }
}

#[cfg(test)]
mod page_tests {
    use super::*;
    use image::{DynamicImage, GenericImage, Rgba};

    fn ink(mut page: DynamicImage, left: u32, top: u32, width: u32, height: u32) -> DynamicImage {
        for y in top..top + height {
            for x in left..left + width {
                page.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        page
    }

    #[test]
    fn page_identity_ignores_toolbar_but_detects_changed_content() {
        let blank = Workflow::create_blank_image();
        let toolbar = ink(blank.clone(), 0, 100, 100, 500);
        assert!(Workflow::is_same_page(&blank, &toolbar));
        let content = ink(blank.clone(), 320, 200, 200, 300);
        assert!(!Workflow::is_same_page(&blank, &content));
        assert!(!Workflow::is_same_page(
            &blank,
            &DynamicImage::new_luma8(10, 10)
        ));
    }

    #[test]
    fn blank_header_and_occupied_pages_have_distinct_decisions() {
        let blank = Workflow::create_blank_image();
        assert_eq!(
            Workflow::classify_answer_page(&blank, None),
            AnswerPageType::Blank
        );
        let headed = ink(blank.clone(), 320, 90, 200, 30);
        let pattern = headed.crop_imm(0, 0, 768, 150);
        let answered = ink(headed, 320, 200, 200, 300);
        assert_eq!(
            Workflow::classify_answer_page(&answered, Some(&pattern)),
            AnswerPageType::ExistingQA
        );
        assert_eq!(
            Workflow::classify_answer_page(&answered, None),
            AnswerPageType::Invalid
        );
        let different_header = ink(blank, 320, 70, 200, 70);
        assert_eq!(
            Workflow::classify_answer_page(&different_header, Some(&pattern)),
            AnswerPageType::Invalid
        );
    }

    #[test]
    fn invalid_comparison_regions_never_panic_or_match() {
        let tiny = DynamicImage::new_luma8(10, 10);
        assert_eq!(
            Workflow::compute_image_similarity_masked(&tiny, &tiny, 298, 125, 70, 70, 5),
            0.0
        );
        assert_eq!(
            Workflow::compute_image_similarity_masked(&tiny, &tiny, 0, 0, 0, 0, 0),
            0.0
        );
    }

    #[test]
    fn qa_format_preserves_values_lines_and_matching_delimiters() {
        assert_eq!(
            Workflow::compose_qa(
                "G unc.?",
                "G = (6.674215 +/- 0.000092) * 10^-11\nunits",
                crate::analysis::SelectionCenter::from_pixels(384.0, 512.0, 768, 1024).unwrap()
            ),
            "<Start of Q-A block for Q @ (0.5, 0.5)>\nQ: G unc.?\n\nA: G = (6.674215 +/- 0.000092) * 10^-11\nunits\n<End of Q-A block for Q @ (0.5, 0.5)>\n"
        );
    }

    #[test]
    fn qa_delimiters_share_normalized_endpoint_coordinates() {
        for (x, y, expected) in [(0.0, 1024.0, "(0, 1)"), (768.0, 0.0, "(1, 0)")] {
            let block = Workflow::compose_qa(
                "Why?",
                "Because.",
                crate::analysis::SelectionCenter::from_pixels(x, y, 768, 1024).unwrap(),
            );
            assert_eq!(block, format!("<Start of Q-A block for Q @ {expected}>\nQ: Why?\n\nA: Because.\n<End of Q-A block for Q @ {expected}>\n"));
            assert_eq!(block.bytes().filter(|b| *b == b'\n').count(), 5);
        }
    }
}
