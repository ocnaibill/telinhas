//! Capture sources: monitors, windows and the GPUs that will encode them.
//!
//! Only Windows is implemented (Windows Graphics Capture and DXGI, DEC-58).
//! On other systems every query returns [`Error::Unsupported`], so the
//! workspace still builds everywhere.

use std::fmt;

mod pacer;
pub use pacer::Pacer;

pub mod audio;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(not(any(windows, target_os = "linux")))]
mod stub;
#[cfg(not(any(windows, target_os = "linux")))]
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
    /// The process that owns the window, 0 if unknown. Its audio is the
    /// window's audio (with the processes it started).
    pub pid: u32,
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

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A monitor, by the `index` of [`list_monitors`].
    Monitor(usize),
    /// A window, by the `handle` of [`list_windows`].
    Window(isize),
    /// Whatever the person picks in the desktop's own dialog (Linux, DEC-29).
    /// The portal does not list screens or windows to the app, so there is
    /// nothing to choose from before asking.
    Portal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Draw the mouse cursor into the picture.
    pub cursor: bool,
    /// Leave the system's capture border on (the yellow outline).
    pub border: bool,
    /// Deliver about this many frames per second, picked at an even rhythm
    /// from a capture running at the monitor's full rate (see [`Pacer`]).
    /// `None` delivers every frame the system produces.
    pub max_fps: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cursor: true,
            border: true,
            max_fps: None,
        }
    }
}

/// One captured picture, borrowed for the duration of the callback.
pub struct FrameRef<'a> {
    pub width: u32,
    pub height: u32,
    /// Bytes per row; at least `width * 4`.
    pub stride: usize,
    /// Pixels as BGRA, `stride * height` bytes.
    pub bgra: &'a [u8],
    /// When the system produced the frame, in microseconds on a clock that
    /// only means something relative to other frames of the same session.
    pub system_micros: i64,
    /// How long copying the frame from the GPU took, in microseconds.
    pub copy_micros: u32,
}

/// A running capture. Dropping it stops the capture.
pub struct Session {
    inner: imp::Session,
}

impl Session {
    /// Stops the capture now. Dropping the session does the same.
    pub fn stop(self) {
        drop(self.inner);
    }

    /// Size of the source when the capture began.
    pub fn size(&self) -> (u32, u32) {
        self.inner.size()
    }

    /// What happened when the capture border was asked to go away, in words.
    pub fn border_status(&self) -> String {
        self.inner.border_status()
    }

    /// How many frames the pacer left out so far.
    pub fn frames_paced_out(&self) -> u64 {
        self.inner.frames_paced_out()
    }

    /// What happened when a frame rate cap was asked for, in words.
    pub fn max_fps_status(&self) -> String {
        self.inner.max_fps_status()
    }

    /// Whether the system accepted the cursor setting.
    pub fn cursor_applied(&self) -> bool {
        self.inner.cursor_applied()
    }
}

/// Starts capturing `source`. `on_frame` runs on a system thread for every
/// frame, so it should return quickly.
pub fn start(
    source: Source,
    options: Options,
    on_frame: impl FnMut(FrameRef<'_>) + Send + 'static,
) -> Result<Session> {
    Ok(Session {
        inner: imp::start(source, options, Box::new(on_frame))?,
    })
}

/// CPU time used so far by this process (user and kernel), in seconds.
pub fn process_cpu_seconds() -> Option<f64> {
    imp::process_cpu_seconds()
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

    #[cfg(not(any(windows, target_os = "linux")))]
    #[test]
    fn other_systems_report_unsupported() {
        assert_eq!(list_monitors(), Err(Error::Unsupported));
        assert_eq!(list_windows(), Err(Error::Unsupported));
        assert_eq!(list_gpus(), Err(Error::Unsupported));
        assert_eq!(capture_support(), Err(Error::Unsupported));
    }
}
