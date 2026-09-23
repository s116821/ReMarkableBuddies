pub mod backend;
#[cfg(any(target_os = "linux", test))]
mod capture_recovery;
pub mod contact_frames;
#[cfg(any(target_os = "linux", test))]
mod history_readiness;
#[cfg(target_os = "linux")]
pub mod input_observer;
pub mod interaction;
pub mod keyboard;
#[cfg(target_os = "linux")]
pub mod native_history;
pub mod native_page;
pub mod native_text;
#[cfg(any(target_os = "linux", test))]
mod navigation_completion;
#[cfg(any(target_os = "linux", test))]
mod owned_pen_window;
#[cfg(any(target_os = "linux", test))]
mod owned_touch_window;
pub mod pen;
pub mod screenshot;
#[cfg(any(target_os = "linux", test))]
mod status_readiness;
pub mod status_style;
pub mod touch;
#[cfg(any(target_os = "linux", test))]
mod trigger_dismiss;

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeviceModel {
    Remarkable2,
    RemarkablePaperPro,
    Unknown,
}

impl DeviceModel {
    pub fn detect() -> Self {
        if Path::new("/etc/hwrevision").exists() {
            if let Ok(hwrev) = std::fs::read_to_string("/etc/hwrevision") {
                if hwrev.contains("ferrari 1.0") {
                    return DeviceModel::RemarkablePaperPro;
                }
                if hwrev.contains("reMarkable2 1.0") {
                    return DeviceModel::Remarkable2;
                }
            }
        }

        // Nothing matched :shrug:
        DeviceModel::Unknown
    }

    pub fn name(&self) -> &str {
        match self {
            DeviceModel::Remarkable2 => "Remarkable2",
            DeviceModel::RemarkablePaperPro => "RemarkablePaperPro",
            DeviceModel::Unknown => "Unknown",
        }
    }
}
