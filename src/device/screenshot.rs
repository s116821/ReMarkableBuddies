use anyhow::Result;
use image::GrayImage;
use log::{debug, info};
use std::fs::File;
use std::io::Write;
use std::io::{Read, Seek};
use std::process;

use base64::{engine::general_purpose, Engine as _};
use image::ImageEncoder;

use super::DeviceModel;

/// Virtual screen width - all screenshots are normalized to this size
pub const SCREENSHOT_VIRTUAL_WIDTH: u32 = 768;
/// Virtual screen height - all screenshots are normalized to this size  
pub const SCREENSHOT_VIRTUAL_HEIGHT: u32 = 1024;

pub struct Screenshot {
    data: Vec<u8>,
    native_data: Vec<u8>,
    device_model: DeviceModel,
    rm2_bgra: bool,
}

impl Screenshot {
    pub fn new() -> Result<Screenshot> {
        let device_model = DeviceModel::detect();
        info!("Screen detected device: {}", device_model.name());
        let rm2_bgra = if device_model == DeviceModel::Remarkable2 {
            Self::rm2_uses_bgra(&std::fs::read_to_string("/etc/os-release")?)?
        } else {
            false
        };
        Ok(Screenshot {
            data: vec![],
            native_data: vec![],
            device_model,
            rm2_bgra,
        })
    }

    // Firmware layout change documented by awwaiid/ghostwriter commit dd48e60.
    fn rm2_uses_bgra(os_release: &str) -> Result<bool> {
        let version = os_release
            .lines()
            .find_map(|line| line.strip_prefix("IMG_VERSION="))
            .ok_or_else(|| {
                anyhow::anyhow!("Missing IMG_VERSION; cannot select RM2 framebuffer layout")
            })?;
        let mut parts = version.trim().trim_matches('"').split('.');
        let major: u32 = parts.next().unwrap_or("").parse()?;
        let minor: u32 = parts.next().unwrap_or("").parse()?;
        Ok((major, minor) >= (3, 24))
    }

    fn screen_width(&self) -> u32 {
        match self.device_model {
            DeviceModel::Remarkable2 => 1872,
            DeviceModel::RemarkablePaperPro => 1632,
            DeviceModel::Unknown => 1872, // Default to RM2
        }
    }

    fn screen_height(&self) -> u32 {
        match self.device_model {
            DeviceModel::Remarkable2 => 1404,
            DeviceModel::RemarkablePaperPro => 2154,
            DeviceModel::Unknown => 1404, // Default to RM2
        }
    }

    pub fn bytes_per_pixel(&self) -> usize {
        match self.device_model {
            DeviceModel::Remarkable2 => {
                if self.rm2_bgra {
                    4
                } else {
                    2
                }
            }
            DeviceModel::RemarkablePaperPro => 4,
            DeviceModel::Unknown => 2, // Default to RM2
        }
    }

    pub fn take_screenshot(&mut self) -> Result<()> {
        // Find xochitl's process
        debug!("screenshot: finding pid");
        let pid = Self::find_xochitl_pid()?;

        // Find framebuffer location in memory
        debug!("screenshot: finding address");
        let skip_bytes = self.find_framebuffer_address(&pid)?;
        info!(
            "Framebuffer address={:#x}, bytes_per_pixel={}",
            skip_bytes,
            self.bytes_per_pixel()
        );

        // Read the framebuffer data
        debug!("screenshot: reading data");
        let screenshot_data = self.read_framebuffer(&pid, skip_bytes)?;
        self.native_data = self.encode_png(&screenshot_data)?;
        // Process the image data (transpose, color correction, etc.)
        debug!("screenshot: processing image");
        let processed_data = self.process_image(screenshot_data)?;

        self.data = processed_data;

        Ok(())
    }

    fn find_xochitl_pid() -> Result<String> {
        let output = process::Command::new("pidof").arg("xochitl").output()?;
        let pids = String::from_utf8(output.stdout)?;
        if let Some(pid) = pids.split_whitespace().next() {
            return Ok(pid.to_string());
        }
        anyhow::bail!("No xochitl process found")
    }

    fn find_framebuffer_address(&self, pid: &str) -> Result<u64> {
        if self.rm2_bgra {
            return self.find_rm2_bgra_allocation(pid);
        }
        match self.device_model {
            DeviceModel::RemarkablePaperPro => {
                // For RMPP (arm64), we need to use the approach from pointer_arm64.go
                let start_address = self.get_memory_range(pid)?;
                let frame_pointer = self.calculate_frame_pointer(pid, start_address)?;
                Ok(frame_pointer)
            }
            _ => {
                // Original RM2 approach
                let output = process::Command::new("sh")
                    .arg("-c")
                    .arg(format!(
                        "grep -C1 '/dev/fb0' /proc/{}/maps | tail -n1 | sed 's/-.*$//'",
                        pid
                    ))
                    .output()?;
                let address_hex = String::from_utf8(output.stdout)?.trim().to_string();
                let address = u64::from_str_radix(&address_hex, 16)?;
                Ok(address + if self.rm2_bgra { 2_629_632 + 8 } else { 7 })
            }
        }
    }

    fn find_rm2_bgra_allocation(&self, pid: &str) -> Result<u64> {
        // 3.28 can place the mmap-backed pixel allocation away from fb0.
        // Validate glibc's 32-bit mmap chunk header instead of reading through
        // unrelated mappings at the historical fixed offset.
        let bytes = 1404_u64 * 1872 * 4;
        let allocation_size = (bytes + 8 + 4095) & !4095;
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps"))?;
        let mut mem = File::open(format!("/proc/{pid}/mem"))?;
        let mut candidates = Vec::new();
        for line in maps.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 5 || fields[1] != "rw-p" || fields[4] != "0" {
                continue;
            }
            let Some((start, end)) = fields[0].split_once('-') else {
                continue;
            };
            let start = u64::from_str_radix(start, 16)?;
            let end = u64::from_str_radix(end, 16)?;
            if end - start < allocation_size {
                continue;
            }
            mem.seek(std::io::SeekFrom::Start(start))?;
            let mut header = [0u8; 8];
            mem.read_exact(&mut header)?;
            let previous = u32::from_le_bytes(header[..4].try_into()?);
            let size = u32::from_le_bytes(header[4..].try_into()?) as u64;
            if previous == 0 && size & 7 == 2 && size & !7 == allocation_size {
                candidates.push(start + 8);
            }
        }
        anyhow::ensure!(
            candidates.len() == 1,
            "Expected one RM2 framebuffer allocation, found {}; refusing ambiguous capture",
            candidates.len()
        );
        Ok(candidates[0])
    }

    // Get memory range for RMPP based on goMarkableStream/pointer_arm64.go
    fn get_memory_range(&self, pid: &str) -> Result<u64> {
        let maps_file_path = format!("/proc/{}/maps", pid);
        debug!("screenshot: reading memory range from {}", maps_file_path);
        let maps_content = std::fs::read_to_string(&maps_file_path)?;

        let mut memory_range = String::new();
        debug!("Scanning for '/dev/dri/card0' in memory");
        for line in maps_content.lines() {
            if line.contains("/dev/dri/card0") {
                memory_range = line.to_string();
                debug!("Found memory range: {}", memory_range);
            }
        }

        if memory_range.is_empty() {
            anyhow::bail!("No mapping found for /dev/dri/card0");
        }

        debug!("Final memory range: {}", memory_range);
        let fields: Vec<&str> = memory_range.split_whitespace().collect();
        let range_field = fields[0];
        let start_end: Vec<&str> = range_field.split('-').collect();

        if start_end.len() != 2 {
            anyhow::bail!("Invalid memory range format");
        }

        let end = u64::from_str_radix(start_end[1], 16)?;
        debug!(
            "range_field: {}\nstart_end: {}\nend: {}",
            range_field, start_end[1], end
        );
        Ok(end)
    }

    // Calculate frame pointer for RMPP based on goMarkableStream/pointer_arm64.go
    fn calculate_frame_pointer(&self, pid: &str, start_address: u64) -> Result<u64> {
        let mem_file_path = format!("/proc/{}/mem", pid);
        let mut file = std::fs::File::open(mem_file_path)?;

        let screen_size_bytes = self.screen_width() as u64
            * self.screen_height() as u64
            * self.bytes_per_pixel() as u64;

        let mut offset: u64 = 0;
        let mut length: u64 = 2;

        while length < screen_size_bytes {
            offset += length - 2;

            file.seek(std::io::SeekFrom::Start(start_address + offset + 8))?;
            let mut header = [0u8; 8];
            file.read_exact(&mut header)?;
            debug!("  ... header: {:?}", header);

            length = (header[0] as u64)
                | ((header[1] as u64) << 8)
                | ((header[2] as u64) << 16)
                | ((header[3] as u64) << 24);
            debug!("  ... length: {}", length);
            if length < 2 {
                anyhow::bail!("Invalid header length");
            }
        }

        Ok(start_address + offset)
    }

    fn read_framebuffer(&self, pid: &str, skip_bytes: u64) -> Result<Vec<u8>> {
        let window_bytes =
            self.screen_width() as usize * self.screen_height() as usize * self.bytes_per_pixel();
        let mut buffer = vec![0u8; window_bytes];
        let mut file = std::fs::File::open(format!("/proc/{}/mem", pid))?;
        file.seek(std::io::SeekFrom::Start(skip_bytes))?;
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    fn process_image(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        // Encode the raw data to PNG
        debug!("Encoding raw image data to PNG");
        let png_data = self.encode_png(&data)?;

        // Resize the PNG to SCREENSHOT_VIRTUAL_WIDTH x SCREENSHOT_VIRTUAL_HEIGHT
        debug!(
            "Resizing image to {}x{}",
            SCREENSHOT_VIRTUAL_WIDTH, SCREENSHOT_VIRTUAL_HEIGHT
        );
        let img = image::load_from_memory(&png_data)?;
        let resized_img = img.resize_exact(
            SCREENSHOT_VIRTUAL_WIDTH,
            SCREENSHOT_VIRTUAL_HEIGHT,
            image::imageops::FilterType::Nearest,
        );

        // Encode the resized image back to PNG
        debug!("Re-encoding resized image");
        let mut resized_png_data = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut resized_png_data);

        // Handle different color types based on device
        match self.device_model {
            DeviceModel::RemarkablePaperPro => {
                encoder.write_image(
                    resized_img.as_rgba8().unwrap().as_raw(),
                    SCREENSHOT_VIRTUAL_WIDTH,
                    SCREENSHOT_VIRTUAL_HEIGHT,
                    image::ExtendedColorType::Rgba8,
                )?;
            }
            _ => {
                encoder.write_image(
                    resized_img.as_luma8().unwrap().as_raw(),
                    SCREENSHOT_VIRTUAL_WIDTH,
                    SCREENSHOT_VIRTUAL_HEIGHT,
                    image::ExtendedColorType::L8,
                )?;
            }
        }

        Ok(resized_png_data)
    }

    fn encode_png(&self, raw_data: &[u8]) -> Result<Vec<u8>> {
        match self.device_model {
            DeviceModel::RemarkablePaperPro => {
                // RMPP uses 32-bit RGBA format
                self.encode_png_rmpp(raw_data)
            }
            _ => {
                // RM2 uses 16-bit grayscale
                self.encode_png_rm2(raw_data)
            }
        }
    }

    fn encode_png_rm2(&self, raw_data: &[u8]) -> Result<Vec<u8>> {
        anyhow::ensure!(
            raw_data.len() == 1404 * 1872 * self.bytes_per_pixel(),
            "Invalid RM2 framebuffer length"
        );
        if self.rm2_bgra {
            // RM2 is monochrome: the blue channel carries the grayscale value.
            // New firmware stores portrait BGRA with full-range gray values.
            let pixels: Vec<u8> = raw_data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[0])
                .collect();
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png).write_image(
                &pixels,
                1404,
                1872,
                image::ExtendedColorType::L8,
            )?;
            return Ok(png);
        }
        let raw_u8: Vec<u8> = raw_data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u8::from_le_bytes([chunk[1]]))
            .collect();
        let width = self.screen_width();
        let height = self.screen_height();
        let processed: Vec<u8> = raw_u8
            .iter()
            .map(|&value| Self::apply_curves(value))
            .collect();

        let img = GrayImage::from_raw(width, height, processed)
            .ok_or_else(|| anyhow::anyhow!("Failed to create image from raw data"))?;
        let rotated_img = image::imageops::rotate270(&img);
        let final_image = image::imageops::flip_horizontal(&rotated_img);
        let mut png_data = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
        encoder.write_image(
            final_image.as_raw(),
            final_image.width(),
            final_image.height(),
            image::ExtendedColorType::L8,
        )?;

        Ok(png_data)
    }

    fn encode_png_rmpp(&self, raw_data: &[u8]) -> Result<Vec<u8>> {
        let width = self.screen_width();
        let height = self.screen_height();
        let mut png_data = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
        debug!("Encoding {}x{} image", width, height);
        encoder.write_image(raw_data, width, height, image::ExtendedColorType::Rgba8)?;
        Ok(png_data)
    }

    fn apply_curves(value: u8) -> u8 {
        let normalized = value as f32 / 255.0;
        let adjusted = if normalized < 0.045 {
            0.0
        } else if normalized < 0.06 {
            (normalized - 0.045) / (0.06 - 0.045)
        } else {
            1.0
        };
        (adjusted * 255.0) as u8
    }

    pub fn save_image(&self, filename: &str) -> Result<()> {
        let mut png_file = File::create(filename)?;
        png_file.write_all(&self.data)?;
        debug!("PNG image saved to {}", filename);
        Ok(())
    }

    pub fn base64(&self) -> Result<String> {
        let base64_image = general_purpose::STANDARD.encode(&self.data);
        Ok(base64_image)
    }

    /// Overlapping full-width strips preserve small PDF text without splitting
    /// questions or text lines across left/right crops. Order: top to bottom.
    /// Navigation still uses the normalized overview.
    pub fn detail_images_base64(&self) -> Result<Vec<String>> {
        let img = image::load_from_memory(&self.native_data)?;
        let (w, h) = (img.width(), img.height());
        let th = h * 2 / 5;
        let mut tiles = Vec::new();
        for y in [0, (h - th) / 2, h - th] {
            let tile = img.crop_imm(0, y, w, th);
            let mut out = std::io::Cursor::new(Vec::new());
            tile.write_to(&mut out, image::ImageFormat::Png)?;
            tiles.push(general_purpose::STANDARD.encode(out.into_inner()));
        }
        Ok(tiles)
    }

    pub fn get_image_data(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_boundary_and_invalid_versions() {
        for (version, expected) in [
            ("3.23.0.1", false),
            ("3.24.0.1", true),
            ("3.28.0.172", true),
            ("4.0.0", true),
        ] {
            assert_eq!(
                Screenshot::rm2_uses_bgra(&format!("IMG_VERSION=\"{version}\"\n")).unwrap(),
                expected
            );
        }
        assert!(Screenshot::rm2_uses_bgra("VERSION=3.28").is_err());
        assert!(Screenshot::rm2_uses_bgra("IMG_VERSION=broken").is_err());
    }

    #[test]
    fn bgra_capture_preserves_portrait_positions_and_gray_levels() {
        let screenshot = Screenshot {
            data: vec![],
            native_data: vec![],
            device_model: DeviceModel::Remarkable2,
            rm2_bgra: true,
        };
        let mut raw = vec![255; 1404 * 1872 * 4];
        raw[0] = 17;
        raw[1404 * 4] = 128;
        let png = screenshot.encode_png_rm2(&raw).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_luma8();
        assert_eq!(img.dimensions(), (1404, 1872));
        assert_eq!(img.get_pixel(0, 0).0, [17]);
        assert_eq!(img.get_pixel(0, 1).0, [128]);
        assert_eq!(img.get_pixel(1, 0).0, [255]);
        assert!(screenshot.encode_png_rm2(&raw[..raw.len() - 1]).is_err());
        let normalized = image::load_from_memory(&screenshot.process_image(raw).unwrap()).unwrap();
        assert_eq!((normalized.width(), normalized.height()), (768, 1024));
    }

    #[test]
    fn detail_tiles_preserve_native_pixels_and_overlap() {
        let mut img = GrayImage::from_pixel(200, 300, image::Luma([255]));
        img.put_pixel(0, 0, image::Luma([17]));
        img.put_pixel(199, 299, image::Luma([23]));
        img.put_pixel(100, 100, image::Luma([128]));
        img.put_pixel(100, 200, image::Luma([64]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let screenshot = Screenshot {
            data: vec![],
            native_data: png.into_inner(),
            device_model: DeviceModel::Remarkable2,
            rm2_bgra: true,
        };
        let tiles: Vec<_> = screenshot
            .detail_images_base64()
            .unwrap()
            .iter()
            .map(|tile| {
                image::load_from_memory(&general_purpose::STANDARD.decode(tile).unwrap())
                    .unwrap()
                    .to_luma8()
            })
            .collect();
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].get_pixel(0, 0).0, [17]);
        assert_eq!(tiles[2].get_pixel(199, 119).0, [23]);
        for tile in &tiles {
            assert_eq!(tile.dimensions(), (200, 120));
        }
        assert_eq!(tiles[0].get_pixel(100, 100).0, [128]);
        assert_eq!(tiles[1].get_pixel(100, 10).0, [128]);
        assert_eq!(tiles[1].get_pixel(100, 110).0, [64]);
        assert_eq!(tiles[2].get_pixel(100, 20).0, [64]);
    }
}
