//! Placeholder backend for systems without capture support.

use crate::{CaptureSupport, Error, FrameRef, Gpu, Monitor, Options, Result, Source, Window};

pub fn init_dpi_awareness() {}

pub fn list_monitors() -> Result<Vec<Monitor>> {
    Err(Error::Unsupported)
}

pub fn list_windows() -> Result<Vec<Window>> {
    Err(Error::Unsupported)
}

pub fn list_gpus() -> Result<Vec<Gpu>> {
    Err(Error::Unsupported)
}

pub fn capture_support() -> Result<CaptureSupport> {
    Err(Error::Unsupported)
}

pub struct Session;

impl Session {
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }

    pub fn border_status(&self) -> String {
        String::new()
    }

    pub fn cursor_applied(&self) -> bool {
        false
    }
}

pub fn start(
    _source: Source,
    _options: Options,
    _on_frame: Box<dyn FnMut(FrameRef<'_>) + Send>,
) -> Result<Session> {
    Err(Error::Unsupported)
}

pub fn process_cpu_seconds() -> Option<f64> {
    None
}

impl Drop for Session {
    /// Nothing to stop; mirrors the Windows session, which ends capture here.
    fn drop(&mut self) {}
}
