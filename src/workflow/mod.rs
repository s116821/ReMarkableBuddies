pub mod orchestrator;
pub mod symbol_pool;
pub mod xochitl_integration;

use anyhow::Result;
use log::{debug, info, warn};

use crate::device::{
    keyboard::Keyboard,
    pen::Pen,
    screenshot::{Screenshot, SCREENSHOT_VIRTUAL_HEIGHT, SCREENSHOT_VIRTUAL_WIDTH},
    touch::Touch,
};

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

/// Cache directory for Reader Buddy (standard Linux location for cache files)
const CACHE_DIR: &str = "/var/cache/reader-buddy";

/// Path to the cached header pattern image
const HEADER_PATTERN_PATH: &str = "/var/cache/reader-buddy/header-pattern.png";

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
    screenshot: Screenshot,
    pen: Pen,
    keyboard: Keyboard,
    touch: Touch,
    debug_dump: bool,
    iteration_count: u32,
}

impl Workflow {
    pub fn new(
        no_draw: bool,
        trigger_corner: crate::device::touch::TriggerCorner,
        debug_dump: bool,
    ) -> Result<Self> {
        // Initialize cache directory (creates if needed, clears old files)
        Self::init_cache()?;

        Ok(Self {
            screenshot: Screenshot::new()?,
            pen: Pen::new(no_draw),
            keyboard: Keyboard::new(no_draw, false),
            touch: Touch::new(no_draw, trigger_corner),
            debug_dump,
            iteration_count: 0,
        })
    }

    /// Initialize the cache directory
    /// Creates the directory if it doesn't exist
    /// Note: We intentionally preserve cached files (like header patterns) across restarts
    /// so that existing QA pages can still be recognized after a service restart
    fn init_cache() -> Result<()> {
        use std::fs;

        info!("Initializing cache directory: {}", CACHE_DIR);

        // Create cache directory (and parent directories if needed)
        match fs::create_dir_all(CACHE_DIR) {
            Ok(_) => info!("Cache directory created/verified: {}", CACHE_DIR),
            Err(e) => {
                // Log error but don't fail - cache is optional
                warn!("Failed to create cache directory {}: {}", CACHE_DIR, e);
                return Ok(());
            }
        }

        info!("Cache initialized successfully");
        Ok(())
    }

    /// Wait for user to trigger the workflow (touch in corner)
    pub fn wait_for_trigger(&mut self) -> Result<()> {
        info!("Waiting for trigger...");
        self.touch.wait_for_trigger()?;
        self.touch.tap_middle_bottom()?;
        Ok(())
    }

    /// Take a screenshot and return the base64-encoded image
    pub fn capture_screenshot(&mut self) -> Result<String> {
        info!("Capturing screenshot...");
        self.screenshot.take_screenshot()?;
        self.screenshot.base64()
    }

    /// Take a screenshot and return both base64 and raw PNG data
    pub fn capture_screenshot_with_data(&mut self) -> Result<(String, Vec<u8>)> {
        info!("Capturing screenshot...");
        self.screenshot.take_screenshot()?;
        let base64 = self.screenshot.base64()?;
        let png_data = self.screenshot.get_image_data().to_vec();

        // Debug dump if enabled
        if self.debug_dump {
            self.iteration_count += 1;
            let filename = format!(
                "/tmp/reader-buddy-screenshot-{:03}.png",
                self.iteration_count
            );
            if let Err(e) = self.screenshot.save_image(&filename) {
                log::warn!("Failed to save debug screenshot: {}", e);
            } else {
                log::debug!("Saved debug screenshot to {}", filename);
            }
        }

        Ok((base64, png_data))
    }

    /// Show progress indicator to user
    pub fn show_progress(&mut self, message: &str) -> Result<()> {
        self.keyboard.progress(message)?;
        Ok(())
    }

    /// Clear progress indicator
    pub fn clear_progress(&mut self) -> Result<()> {
        self.keyboard.progress_end()?;
        Ok(())
    }

    /// Erase a region on the screen using the eraser tool
    pub fn erase_region(&mut self, region: &crate::analysis::BoundingBox) -> Result<()> {
        info!(
            "Erasing region at ({}, {}) size {}x{}",
            region.x, region.y, region.width, region.height
        );

        let top_left = (region.x, region.y);
        let bottom_right = (region.x + region.width, region.y + region.height);

        // Use the eraser tool to erase the rectangle
        self.pen.erase_rectangle(top_left, bottom_right)?;

        Ok(())
    }

    /// Smart erase that only erases detected ink pixels within the region
    pub fn erase_region_smart(
        &mut self,
        region: &crate::analysis::BoundingBox,
        screenshot_data: &[u8],
    ) -> Result<()> {
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
                self.pen.erase_rectangle(top_left, bottom_right)?;
            }
        }

        Ok(())
    }

    /// Draw a reference symbol at a location using bitmap rendering
    pub fn draw_symbol(&mut self, x: i32, y: i32, symbol: &str) -> Result<()> {
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

        self.pen.draw_bitmap(&positioned_bitmap)?;

        Ok(())
    }

    /// Render text on the screen using the keyboard
    /// Note: The caller is responsible for including any desired newlines in the text
    pub fn render_text(&mut self, text: &str) -> Result<()> {
        info!("Rendering text: {}", text);
        self.keyboard.string_to_keypresses(text)?;
        Ok(())
    }

    /// Switch keyboard to body text mode (should be called once before rendering)
    pub fn set_body_text_mode(&mut self) -> Result<()> {
        self.keyboard.key_cmd_body()?;
        Ok(())
    }

    /// Get access to the keyboard for direct manipulation
    pub fn get_keyboard_mut(&mut self) -> &mut Keyboard {
        &mut self.keyboard
    }

    /// Get access to the pen for direct manipulation
    pub fn get_pen_mut(&mut self) -> &mut Pen {
        &mut self.pen
    }

    /// Get access to the touch device for direct manipulation
    pub fn get_touch_mut(&mut self) -> &mut Touch {
        &mut self.touch
    }

    /// Navigate to the next page (swipe left)
    pub fn navigate_to_next_page(&mut self) -> Result<()> {
        xochitl_integration::XochitlIntegration::navigate_to_page(
            &mut self.touch,
            xochitl_integration::NavigationDirection::Next,
        )?;
        Ok(())
    }

    /// Navigate back to the previous page (swipe right)
    pub fn navigate_to_previous_page(&mut self) -> Result<()> {
        xochitl_integration::XochitlIntegration::navigate_to_page(
            &mut self.touch,
            xochitl_integration::NavigationDirection::Previous,
        )?;
        Ok(())
    }

    /// Draw a failure X in the bottom-right corner (~75x75 px)
    /// Used to indicate that no valid answer page was found
    pub fn draw_failure_x(&mut self) -> Result<()> {
        info!("Drawing failure X in bottom-right corner");

        // Position: bottom-right corner with some margin
        // Screen is 768x1024, X should be ~75x75
        const X_SIZE: i32 = 75;
        const MARGIN: i32 = 20;

        let x_start = 768 - MARGIN - X_SIZE;
        let y_start = 1024 - MARGIN - X_SIZE;
        let x_end = 768 - MARGIN;
        let y_end = 1024 - MARGIN;

        // Draw two diagonal lines to form an X (using screen coordinates)
        // Line 1: top-left to bottom-right
        self.pen
            .draw_line_screen((x_start, y_start), (x_end, y_end))?;

        // Line 2: top-right to bottom-left
        self.pen
            .draw_line_screen((x_end, y_start), (x_start, y_end))?;

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
        info!("Checking if current page is valid for answers (blank or QA page)");

        // Take screenshot of current page
        std::thread::sleep(std::time::Duration::from_millis(500)); // Let page settle
        self.screenshot.take_screenshot()?;
        let png_data = self.screenshot.get_image_data();

        // Load image
        let img = match image::load_from_memory(png_data) {
            Ok(img) => img,
            Err(e) => {
                log::warn!("Failed to load screenshot for page check: {}", e);
                return Ok(AnswerPageType::Invalid);
            }
        };

        // Check 1: Is it a blank page?
        // Compare against a synthetic blank (white) image using masked similarity
        let blank_img = Self::create_blank_image();
        let blank_similarity = Self::compute_image_similarity_masked(
            &img,
            &blank_img,
            MASK_LEFT_OFFSET,
            MASK_RIGHT_OFFSET,
            MASK_TOP_OFFSET,
            MASK_BOTTOM_OFFSET,
            BLANK_PAGE_SAMPLE_RATE,
        );

        // Threshold for considering a page "blank" (99.8% similar to white)
        const BLANK_THRESHOLD: f32 = 0.998;

        info!(
            "Blank page check: similarity to blank {:.2}% (threshold: {:.1}%)",
            blank_similarity * 100.0,
            BLANK_THRESHOLD * 100.0
        );

        if blank_similarity >= BLANK_THRESHOLD {
            info!("Page is BLANK - VALID");
            return Ok(AnswerPageType::Blank);
        }

        info!("Page is not blank, checking for QA header...");

        // Check 2: Does it have our QA header pattern?
        const HEADER_HEIGHT: u32 = 150; // Capture full header region from top
        let header_img = img.crop_imm(0, 0, img.width(), HEADER_HEIGHT.min(img.height()));

        // Try fast pattern matching (if we have a saved pattern)
        if let Ok(saved_pattern_data) = std::fs::read(HEADER_PATTERN_PATH) {
            if let Ok(saved_pattern) = image::load_from_memory(&saved_pattern_data) {
                // Use masked similarity comparison
                // Apply same masking as full-page comparisons to skip toolbar/UI elements
                let similarity = Self::compute_image_similarity_masked(
                    &header_img,
                    &saved_pattern,
                    MASK_LEFT_OFFSET,
                    MASK_RIGHT_OFFSET,
                    MASK_TOP_OFFSET,
                    0,
                    DEFAULT_SAMPLE_RATE,
                );

                const SIMILARITY_THRESHOLD: f32 = 0.998;
                info!(
                    "QA header check: similarity {:.2}% (threshold: {:.1}%)",
                    similarity * 100.0,
                    SIMILARITY_THRESHOLD * 100.0
                );

                if similarity >= SIMILARITY_THRESHOLD {
                    info!("Page has QA header - VALID (existing QA page)");
                    return Ok(AnswerPageType::ExistingQA);
                }
            }
        } else {
            info!("QA header check: no saved pattern found (first run?)");
        }

        // Neither blank nor QA page
        info!("Page is NOT valid: failed both blank and QA header checks");
        Ok(AnswerPageType::Invalid)
    }

    /// Save the header pattern for future fast detection
    /// Should be called after successfully detecting an answer page via LLM
    pub fn save_header_pattern(&self, header_img: &image::DynamicImage) -> Result<()> {
        info!("Saving header pattern to {}", HEADER_PATTERN_PATH);

        header_img.save(HEADER_PATTERN_PATH)?;
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

        if gray1.dimensions() != gray2.dimensions() {
            return 0.0;
        }

        // Get dimensions for offset calculations (needed for cropped images)
        let width = gray1.width();
        let height = gray1.height();
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
