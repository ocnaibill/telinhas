//! Placeholder backend for systems without capture support.

use crate::{CaptureSupport, Error, Gpu, Monitor, Result, Window};

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
