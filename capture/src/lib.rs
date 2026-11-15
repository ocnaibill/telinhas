//! Capture sources: monitors, windows and the GPUs that will encode them.
//!
//! Only Windows is implemented (Windows Graphics Capture and DXGI, DEC-58).
//! On other systems every query returns [`Error::Unsupported`], so the
//! workspace still builds everywhere.

use std::fmt;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as imp;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
use stub as imp;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Capture is not implemented for this operating system yet.
    Unsupported,
    /// An operating system call failed.
    Os(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported => write!(f, "capture is not supported on this system yet"),
            Error::Os(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

/// A display, in real pixels (not scaled by the Windows display scaling).
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub index: usize,
    /// Device name, such as `\\.\DISPLAY1`.
    pub device: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Current refresh rate in Hz.
    pub refresh_hz: u32,
    pub primary: bool,
}

/// A top-level window a person would pick from Alt-Tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The native window handle (`HWND` on Windows).
    pub handle: isize,
    pub title: String,
    /// Executable name, such as `chrome.exe`; empty if it cannot be read.
    pub process: String,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Microsoft,
    Other(u32),
}

impl GpuVendor {
    /// Maps a PCI vendor id.
    pub fn from_pci(id: u32) -> Self {
        match id {
            0x10DE => GpuVendor::Nvidia,
            0x1002 | 0x1022 => GpuVendor::Amd,
            0x8086 => GpuVendor::Intel,
            0x1414 => GpuVendor::Microsoft,
            other => GpuVendor::Other(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor: GpuVendor,
    pub vram_mb: u64,
}

/// What the system's capture API can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureSupport {
    /// Windows Graphics Capture is available at all.
    pub graphics_capture: bool,
    /// The yellow capture border can be turned off (`IsBorderRequired`).
    pub border_toggle: bool,
    /// The mouse cursor can be left out (`IsCursorCaptureEnabled`).
    pub cursor_toggle: bool,
}

/// Makes pixel sizes real instead of scaled. Call once, early.
pub fn init_dpi_awareness() {
    imp::init_dpi_awareness();
}

pub fn list_monitors() -> Result<Vec<Monitor>> {
    imp::list_monitors()
}

pub fn list_windows() -> Result<Vec<Window>> {
    imp::list_windows()
}

pub fn list_gpus() -> Result<Vec<Gpu>> {
    imp::list_gpus()
}

pub fn capture_support() -> Result<CaptureSupport> {
    imp::capture_support()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors_map_from_pci_ids() {
        assert_eq!(GpuVendor::from_pci(0x10DE), GpuVendor::Nvidia);
        assert_eq!(GpuVendor::from_pci(0x1002), GpuVendor::Amd);
        assert_eq!(GpuVendor::from_pci(0x8086), GpuVendor::Intel);
        assert_eq!(GpuVendor::from_pci(0x1414), GpuVendor::Microsoft);
        assert_eq!(GpuVendor::from_pci(0x1234), GpuVendor::Other(0x1234));
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_report_unsupported() {
        assert_eq!(list_monitors(), Err(Error::Unsupported));
        assert_eq!(list_windows(), Err(Error::Unsupported));
        assert_eq!(list_gpus(), Err(Error::Unsupported));
        assert_eq!(capture_support(), Err(Error::Unsupported));
    }
}
