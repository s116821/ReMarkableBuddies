use anyhow::{Context, Result};
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
        let _timing = crate::measurement::Span::new("capture.total");
        let native = self.capture_fresh(Self::capture_native)?;
        let native_data = Self::serialize(&native, "capture.native_png")?;
        let normalized = Self::normalize_timed(&native)?;
        let data = Self::serialize(&normalized, "capture.overview_png")?;
        // Publish only a complete capture. A failed attempt cannot expose old
        // overview pixels paired with a different native frame.
        self.native_data = native_data;
        self.data = data;
        Ok(())
    }

    /// Fresh owned pixels for guards that do not consume encoded images.
    pub fn take_image(&mut self) -> Result<image::DynamicImage> {
        let _timing = crate::measurement::Span::new("capture.total");
        self.capture_fresh(|capture| capture.normalized_image(&capture.capture_raw()?))
    }

    fn capture_fresh(
        &mut self,
        read: impl FnOnce(&Self) -> Result<image::DynamicImage>,
    ) -> Result<image::DynamicImage> {
        self.data.clear();
        self.native_data.clear();
        read(self)
    }

    fn normalize_timed(native: &image::DynamicImage) -> Result<image::DynamicImage> {
        let _timing = crate::measurement::Span::new("capture.resize");
        Self::normalize_native(native)
    }

    fn serialize(image: &image::DynamicImage, phase: &'static str) -> Result<Vec<u8>> {
        let _timing = crate::measurement::Span::new(phase);
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        )?;
        Ok(bytes)
    }

    fn capture_native(&self) -> Result<image::DynamicImage> {
        self.native_image(&self.capture_raw()?)
    }

    fn capture_raw(&self) -> Result<Vec<u8>> {
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
        self.read_framebuffer(&pid, skip_bytes)
    }

    fn normalized_image(&self, raw: &[u8]) -> Result<image::DynamicImage> {
        if self.device_model != DeviceModel::Remarkable2 || !self.rm2_bgra {
            return Self::normalize_timed(&self.native_image(raw)?);
        }
        let _timing = crate::measurement::Span::new("capture.fused_conversion");
        anyhow::ensure!(raw.len() == 1404 * 1872 * 4, "Invalid framebuffer length");
        // Identical Nearest coordinates and luminance arithmetic to the native
        // conversion then resize path. Only selected pixels need conversion.
        // Raw bytes still come from a complete fresh framebuffer read.
        let xs = Self::nearest_positions(1404, SCREENSHOT_VIRTUAL_WIDTH);
        let ys = Self::nearest_positions(1872, SCREENSHOT_VIRTUAL_HEIGHT);
        Ok(image::DynamicImage::ImageLuma8(GrayImage::from_fn(
            SCREENSHOT_VIRTUAL_WIDTH,
            SCREENSHOT_VIRTUAL_HEIGHT,
            |x, y| {
                let offset = ((ys[y as usize] * 1404 + xs[x as usize]) * 4) as usize;
                let pixel = &raw[offset..offset + 4];
                image::Luma([((77 * u32::from(pixel[2])
                    + 150 * u32::from(pixel[1])
                    + 29 * u32::from(pixel[0])
                    + 128)
                    >> 8) as u8])
            },
        )))
    }

    fn native_image(&self, raw: &[u8]) -> Result<image::DynamicImage> {
        let _timing = crate::measurement::Span::new("capture.raw_conversion");
        anyhow::ensure!(
            raw.len()
                == self.screen_width() as usize
                    * self.screen_height() as usize
                    * self.bytes_per_pixel(),
            "Invalid framebuffer length"
        );
        if self.device_model == DeviceModel::RemarkablePaperPro {
            return Ok(image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(self.screen_width(), self.screen_height(), raw.to_vec())
                    .ok_or_else(|| anyhow::anyhow!("Invalid RGBA framebuffer"))?,
            ));
        }
        if self.rm2_bgra {
            let pixels = raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    ((77 * u32::from(pixel[2])
                        + 150 * u32::from(pixel[1])
                        + 29 * u32::from(pixel[0])
                        + 128)
                        >> 8) as u8
                })
                .collect();
            return Ok(image::DynamicImage::ImageLuma8(
                GrayImage::from_raw(1404, 1872, pixels)
                    .ok_or_else(|| anyhow::anyhow!("Invalid BGRA framebuffer"))?,
            ));
        }
        let pixels = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pixel| Self::apply_curves(pixel[1]))
            .collect();
        let image = GrayImage::from_raw(self.screen_width(), self.screen_height(), pixels)
            .ok_or_else(|| anyhow::anyhow!("Invalid legacy framebuffer"))?;
        Ok(image::DynamicImage::ImageLuma8(
            image::imageops::flip_horizontal(&image::imageops::rotate270(&image)),
        ))
    }

    fn find_xochitl_pid() -> Result<String> {
        let _timing = crate::measurement::Span::new("capture.pid");
        let output = process::Command::new("pidof").arg("xochitl").output()?;
        let pids = String::from_utf8(output.stdout)?;
        if let Some(pid) = pids.split_whitespace().next() {
            return Ok(pid.to_string());
        }
        anyhow::bail!("No xochitl process found")
    }

    fn find_framebuffer_address(&self, pid: &str) -> Result<u64> {
        let _timing = crate::measurement::Span::new("capture.address");
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
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps"))?;
        let mut mem = File::open(format!("/proc/{pid}/mem"))?;
        Self::locate_rm2_allocation(&maps, |address| {
            mem.seek(std::io::SeekFrom::Start(address))
                .with_context(|| {
                    format!("Seek RM2 allocation header pid={pid} address={address:#x}")
                })?;
            let mut header = [0u8; 8];
            mem.read_exact(&mut header).with_context(|| {
                // Failure evidence only: no second memory read, retry, cached
                // address, or success inferred from a later mapping snapshot.
                let current = std::fs::read_to_string(format!("/proc/{pid}/maps"));
                let details = match current {
                    Ok(current) => {
                        let region = current.lines().find(|line| {
                            let Some(range) = line.split_whitespace().next() else { return false; };
                            let Some((start,end)) = range.split_once('-') else { return false; };
                            matches!((u64::from_str_radix(start,16),u64::from_str_radix(end,16)), (Ok(start),Ok(end)) if start <= address && address < end)
                        }).map(|line| line.split_whitespace().take(2).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "unmapped".into());
                        format!("maps_changed={} current_region={region}", current != maps)
                    }
                    Err(error) => format!("current_maps_unavailable={error}"),
                };
                format!("Read RM2 allocation header pid={pid} address={address:#x} bytes=8; {details}")
            })?;
            Ok(header)
        })
    }

    fn locate_rm2_allocation(
        maps: &str,
        mut read_header: impl FnMut(u64) -> Result<[u8; 8]>,
    ) -> Result<u64> {
        const PAGE_SIZE: u64 = 4096;
        let bytes = 1404_u64 * 1872 * 4;
        let allocation_size = (bytes + 8 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
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
            let Some(last_start) = end.checked_sub(allocation_size) else {
                continue;
            };
            if start > last_start || start % PAGE_SIZE != 0 || end % PAGE_SIZE != 0 {
                continue;
            }
            // Linux may coalesce adjacent mmap chunks into one VMA. Check only
            // page-aligned headers whose complete allocation stays in this map.
            for address in (start..=last_start).step_by(PAGE_SIZE as usize) {
                let header = read_header(address).with_context(|| format!(
                    "RM2 allocation discovery candidate address={address:#x} sampled_region={start:#x}-{end:#x} permissions={}", fields[1]
                ))?;
                let previous = u32::from_le_bytes(header[..4].try_into()?);
                let size = u32::from_le_bytes(header[4..].try_into()?) as u64;
                if previous == 0 && size & 7 == 2 && size & !7 == allocation_size {
                    candidates.push(address + 8);
                }
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
        let _timing = crate::measurement::Span::new("capture.read");
        let window_bytes =
            self.screen_width() as usize * self.screen_height() as usize * self.bytes_per_pixel();
        let mut buffer = vec![0u8; window_bytes];
        let mut file = std::fs::File::open(format!("/proc/{}/mem", pid))?;
        file.seek(std::io::SeekFrom::Start(skip_bytes))?;
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    #[cfg(test)]
    fn process_image(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        let _timing = crate::measurement::Span::new("capture.overview");
        // Encode the raw data to PNG
        debug!("Encoding raw image data to PNG");
        let png_data = self.encode_png(&data)?;

        // Resize the PNG to SCREENSHOT_VIRTUAL_WIDTH x SCREENSHOT_VIRTUAL_HEIGHT
        debug!(
            "Resizing image to {}x{}",
            SCREENSHOT_VIRTUAL_WIDTH, SCREENSHOT_VIRTUAL_HEIGHT
        );
        let decode = crate::measurement::Span::new("capture.decode");
        let img = image::load_from_memory(&png_data)?;
        drop(decode);
        let resize = crate::measurement::Span::new("capture.resize");
        let resized_img = Self::normalize_native(&img)?;

        // Encode the resized image back to PNG
        drop(resize);
        let _serialize = crate::measurement::Span::new("capture.overview_png");
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

    #[cfg(test)]
    fn encode_png(&self, raw_data: &[u8]) -> Result<Vec<u8>> {
        let _timing = crate::measurement::Span::new("capture.native_png");
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

    /// Exact image0.25 Nearest sampling for the two native eight-bit formats.
    /// Its zero-support kernel selects floor((out+0.5)*ratio), using f32.
    /// Keep that arithmetic (not an integer approximation) and copy channels
    /// directly instead of constructing an intermediate RGBA float image.
    fn normalize_native(img: &image::DynamicImage) -> Result<image::DynamicImage> {
        let (width, height) = (img.width(), img.height());
        anyhow::ensure!(width > 0 && height > 0, "Empty native capture");
        let xs = Self::nearest_positions(width, SCREENSHOT_VIRTUAL_WIDTH);
        let ys = Self::nearest_positions(height, SCREENSHOT_VIRTUAL_HEIGHT);
        match img {
            image::DynamicImage::ImageLuma8(source) => {
                Ok(image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(
                    SCREENSHOT_VIRTUAL_WIDTH,
                    SCREENSHOT_VIRTUAL_HEIGHT,
                    |x, y| *source.get_pixel(xs[x as usize], ys[y as usize]),
                )))
            }
            image::DynamicImage::ImageRgba8(source) => {
                Ok(image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(
                    SCREENSHOT_VIRTUAL_WIDTH,
                    SCREENSHOT_VIRTUAL_HEIGHT,
                    |x, y| *source.get_pixel(xs[x as usize], ys[y as usize]),
                )))
            }
            _ => anyhow::bail!("Unexpected native capture pixel format"),
        }
    }

    fn nearest_positions(source: u32, destination: u32) -> Vec<u32> {
        (0..destination)
            .map(|position| {
                (((position as f32 + 0.5) * (source as f32 / destination as f32)).floor() as u32)
                    .min(source - 1)
            })
            .collect()
    }

    #[cfg(test)]
    fn encode_png_rm2(&self, raw_data: &[u8]) -> Result<Vec<u8>> {
        anyhow::ensure!(
            raw_data.len() == 1404 * 1872 * self.bytes_per_pixel(),
            "Invalid RM2 framebuffer length"
        );
        if self.rm2_bgra {
            // The monochrome RM2 still stores colored highlights in portrait BGRA.
            let conversion = crate::measurement::Span::new("capture.raw_conversion");
            // Luminance preserves neutral gray exactly and keeps yellow marks light.
            let pixels: Vec<u8> = raw_data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    ((77 * u32::from(pixel[2])
                        + 150 * u32::from(pixel[1])
                        + 29 * u32::from(pixel[0])
                        + 128)
                        >> 8) as u8
                })
                .collect();
            let mut png = Vec::new();
            drop(conversion);
            let _serialize = crate::measurement::Span::new("capture.native_serialize");
            image::codecs::png::PngEncoder::new(&mut png).write_image(
                &pixels,
                1404,
                1872,
                image::ExtendedColorType::L8,
            )?;
            return Ok(png);
        }
        let conversion = crate::measurement::Span::new("capture.raw_conversion");
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
        drop(conversion);
        let _serialize = crate::measurement::Span::new("capture.native_serialize");
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

    #[cfg(test)]
    fn encode_png_rmpp(&self, raw_data: &[u8]) -> Result<Vec<u8>> {
        let _serialize = crate::measurement::Span::new("capture.native_serialize");
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
        let _timing = crate::measurement::Span::new("capture.detail_strips");
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
    fn failed_fresh_capture_invalidates_both_previous_encoded_views() {
        let mut screenshot = Screenshot {
            data: vec![1, 2, 3],
            native_data: vec![4, 5, 6],
            device_model: DeviceModel::Remarkable2,
            rm2_bgra: true,
        };
        assert!(screenshot
            .capture_fresh(|_| anyhow::bail!("read failed"))
            .is_err());
        assert!(screenshot.get_image_data().is_empty());
        assert!(screenshot.detail_images_base64().is_err());
        screenshot.data = vec![1];
        screenshot.native_data = vec![2];
        assert!(screenshot
            .capture_fresh(|capture| capture.native_image(&[0; 4]))
            .is_err());
        assert!(screenshot.get_image_data().is_empty());
        assert!(screenshot.detail_images_base64().is_err());
    }

    #[test]
    fn owned_pixels_match_previous_png_pipeline_for_all_native_layouts() {
        for (device_model, rm2_bgra) in [
            (DeviceModel::Remarkable2, true),
            (DeviceModel::Remarkable2, false),
            (DeviceModel::RemarkablePaperPro, false),
        ] {
            let screenshot = Screenshot {
                data: vec![],
                native_data: vec![],
                device_model,
                rm2_bgra,
            };
            let len = screenshot.screen_width() as usize
                * screenshot.screen_height() as usize
                * screenshot.bytes_per_pixel();
            // Vary all channels (including alpha and both legacy bytes), spatial
            // orientation and every grayscale curve boundary across a full frame.
            let raw: Vec<u8> = (0..len)
                .map(|i| {
                    let n = (i as u32).wrapping_mul(1664525).wrapping_add(1013904223);
                    (n ^ (n >> 13) ^ (n >> 24)) as u8
                })
                .collect();
            let old_native = screenshot.encode_png(&raw).unwrap();
            let oracle = image::load_from_memory(&old_native).unwrap();
            let actual = screenshot.native_image(&raw).unwrap();
            assert_eq!(
                (actual.width(), actual.height(), actual.color()),
                (oracle.width(), oracle.height(), oracle.color())
            );
            assert_eq!(actual.as_bytes(), oracle.as_bytes());
            // Byte-identical native serialization also preserves the existing
            // overlapping-strip consumer's input, including Paper Pro alpha.
            assert_eq!(
                Screenshot::serialize(&actual, "test.native").unwrap(),
                old_native
            );
            let overview = Screenshot::normalize_native(&actual).unwrap();
            let direct = screenshot.normalized_image(&raw).unwrap();
            let library_oracle = oracle.resize_exact(
                SCREENSHOT_VIRTUAL_WIDTH,
                SCREENSHOT_VIRTUAL_HEIGHT,
                image::imageops::FilterType::Nearest,
            );
            assert_eq!(direct.color(), library_oracle.color());
            assert_eq!(direct.as_bytes(), library_oracle.as_bytes());
            assert_eq!(
                Screenshot::serialize(&overview, "test.overview").unwrap(),
                screenshot.process_image(raw.clone()).unwrap()
            );
            for bad in [&raw[..0], &raw[..len - 1]] {
                assert!(screenshot.native_image(bad).is_err());
                assert!(screenshot.normalized_image(bad).is_err());
            }
            let mut oversized = raw;
            oversized.push(0);
            assert!(screenshot.native_image(&oversized).is_err());
            assert!(screenshot.normalized_image(&oversized).is_err());
        }
    }

    #[test]
    fn direct_nearest_matches_library_pixels_and_alpha() {
        // Actual normalized-native dimensions for modern/rotated legacy RM2 and
        // Paper Pro, plus small/upscaled and equal-size boundaries. Varied alpha
        // must remain a copied channel, never premultiplied or discarded.
        for (width, height) in [(1404, 1872), (1632, 2154), (3, 7), (768, 1024)] {
            let rgba = image::RgbaImage::from_fn(width, height, |x, y| {
                let n = x
                    .wrapping_mul(1664525)
                    .wrapping_add(y.wrapping_mul(1013904223));
                image::Rgba([(n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8])
            });
            let gray = image::GrayImage::from_fn(width, height, |x, y| {
                image::Luma([(x.wrapping_mul(31).wrapping_add(y.wrapping_mul(17))) as u8])
            });
            for native in [
                image::DynamicImage::ImageLuma8(gray),
                image::DynamicImage::ImageRgba8(rgba),
            ] {
                let oracle = native.resize_exact(
                    SCREENSHOT_VIRTUAL_WIDTH,
                    SCREENSHOT_VIRTUAL_HEIGHT,
                    image::imageops::FilterType::Nearest,
                );
                let actual = Screenshot::normalize_native(&native).unwrap();
                assert_eq!(actual.color(), oracle.color());
                assert_eq!(actual.as_bytes(), oracle.as_bytes(), "{width}x{height}");
            }
        }
        assert!(Screenshot::normalize_native(&image::DynamicImage::new_luma8(0, 0)).is_err());
        assert!(Screenshot::normalize_native(&image::DynamicImage::new_rgb8(5, 5)).is_err());
    }

    fn mmap_header(previous: u32, size: u32) -> [u8; 8] {
        let mut header = [0; 8];
        header[..4].copy_from_slice(&previous.to_le_bytes());
        header[4..].copy_from_slice(&size.to_le_bytes());
        header
    }

    #[test]
    fn framebuffer_header_can_be_at_start_or_inside_merged_map() {
        for offset in [0, 0x282000] {
            let start = 0x1000;
            let end = start + offset + 0xa07000;
            let maps = format!("{start:x}-{end:x} rw-p 00000000 00:00 0\n");
            let mut reads = 0;
            let found = Screenshot::locate_rm2_allocation(&maps, |address| {
                reads += 1;
                assert_eq!(address % 4096, 0);
                assert!(address + 0xa07000 <= end);
                Ok(if address == start + offset {
                    mmap_header(0, 0xa07002)
                } else {
                    [0; 8]
                })
            })
            .unwrap();
            assert_eq!(found, start + offset + 8);
            assert_eq!(reads, offset / 4096 + 1);
        }
    }

    #[test]
    fn invalid_headers_and_ambiguous_allocations_fail_closed() {
        let maps = "1000-a09000 rw-p 00000000 00:00 0\n";
        for header in [
            mmap_header(1, 0xa07002),
            mmap_header(0, 0xa07003),
            mmap_header(0, 0xa06002),
            [0; 8],
        ] {
            assert!(Screenshot::locate_rm2_allocation(maps, |_| Ok(header)).is_err());
        }
        assert!(
            Screenshot::locate_rm2_allocation(maps, |_| Ok(mmap_header(0, 0xa07002)))
                .unwrap_err()
                .to_string()
                .contains("found 2")
        );
        let mut reads = 0;
        let error = Screenshot::locate_rm2_allocation(maps, |_| {
            reads += 1;
            Err(std::io::Error::from_raw_os_error(5).into())
        })
        .unwrap_err();
        assert_eq!(reads, 1, "Diagnostic context must not retry memory reads");
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        let context = error.to_string();
        assert!(context.contains("address=0x1000"));
        assert!(context.contains("sampled_region=0x1000-0xa09000"));
        assert!(context.contains("permissions=rw-p"));
    }

    #[test]
    fn mapping_bounds_permissions_and_alignment_exclude_unsafe_reads() {
        for maps in [
            "1000-a07000 rw-p 00000000 00:00 0", // Allocation would cross end.
            "1000-a08000 rw-p 00000000 00:00 0 [heap]",
            "1000-a08000 rw-s 00000000 00:00 0",
            "1000-a08000 rw-p 00000000 00:00 123 /file",
            "1001-a08000 rw-p 00000000 00:00 0",
            "1000-a08001 rw-p 00000000 00:00 0",
            "a08000-1000 rw-p 00000000 00:00 0",
            "0-0 rw-p 00000000 00:00 0",
        ] {
            assert!(
                Screenshot::locate_rm2_allocation(maps, |_| panic!("unexpected memory read"))
                    .is_err()
            );
        }
    }

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
        raw[..4].copy_from_slice(&[17, 17, 17, 255]);
        raw[1404 * 4..1404 * 4 + 4].copy_from_slice(&[128, 128, 128, 255]);
        for gray in 0u8..=255 {
            let offset = (usize::from(gray) + 2) * 4;
            raw[offset..offset + 4].copy_from_slice(&[gray, gray, gray, 255]);
        }
        // Actual native yellow highlight sample, then pure blue and red.
        raw[300 * 4..301 * 4].copy_from_slice(&[125, 254, 254, 255]);
        raw[301 * 4..302 * 4].copy_from_slice(&[255, 0, 0, 255]);
        raw[302 * 4..303 * 4].copy_from_slice(&[0, 0, 255, 255]);
        let png = screenshot.encode_png_rm2(&raw).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_luma8();
        assert_eq!(img.dimensions(), (1404, 1872));
        assert_eq!(img.get_pixel(0, 0).0, [17]);
        assert_eq!(img.get_pixel(0, 1).0, [128]);
        assert_eq!(img.get_pixel(1, 0).0, [255]);
        for gray in 0u8..=255 {
            assert_eq!(img.get_pixel(u32::from(gray) + 2, 0).0, [gray]);
        }
        assert_eq!(img.get_pixel(300, 0).0, [239]);
        assert_eq!(img.get_pixel(301, 0).0, [29]);
        assert_eq!(img.get_pixel(302, 0).0, [77]);
        assert!(screenshot.encode_png_rm2(&raw[..raw.len() - 1]).is_err());
        // Repeated gray ramps cover every neutral value after downsampling;
        // the first rows retain the independent known colored/position samples.
        for (i, pixel) in raw
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
            .skip(1404 * 2)
        {
            let gray = (i % 256) as u8;
            pixel.copy_from_slice(&[gray, gray, gray, (i % 251) as u8]);
        }
        let direct = screenshot.normalized_image(&raw).unwrap();
        let normalized = image::load_from_memory(&screenshot.process_image(raw).unwrap()).unwrap();
        assert_eq!((normalized.width(), normalized.height()), (768, 1024));
        assert_eq!(direct.as_bytes(), normalized.as_bytes());
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
