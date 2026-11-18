//! Audio capture: what the system or one application plays, as a stream of
//! 20 ms frames (48 kHz, stereo, 32-bit float) ready for the Opus encoder.
//!
//! Only Windows is implemented (WASAPI process loopback, Windows 10 version
//! 2004 or newer, DEC-65). Elsewhere every call returns [`Error::Unsupported`].
//!
//! Windows can capture "one process and its children" or "everything except
//! one process and its children", but not "everything except these several".
//! [`AudioSource::System`] with a list of names to leave out therefore
//! captures every other application separately and mixes them (DEC-16, DEC-41).

use crate::Result;

/// Samples per channel in one frame handed to the callback: 20 ms at 48 kHz.
pub const FRAME_SAMPLES: usize = 960;
pub const CHANNELS: usize = 2;
pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSource {
    /// Everything the system plays, except this app (its own playback would
    /// loop back to the people who listen) and the named programs, such as
    /// `Discord.exe` (names compare without case and without `.exe`).
    System { exclude: Vec<String> },
    /// One application and the processes it started.
    Process { pid: u32 },
}

/// A program that has an audio session, playing or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioApp {
    pub pid: u32,
    /// The program's file name, such as `chrome.exe`.
    pub process: String,
    /// Playing right now.
    pub active: bool,
}

/// What one capture did, to see whether it can be trusted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TapStats {
    /// A readable name: `system`, or the program and its process id.
    pub label: String,
    pub packets: u64,
    pub frames: u64,
    /// Loudest sample seen, 0 to 1.
    pub peak: f32,
    /// Longest silence between two packets, in milliseconds. Silence is not an
    /// error: Windows delivers nothing while an application is quiet.
    pub max_gap_ms: u32,
    /// Audio dropped because the mixer did not keep up.
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioStats {
    pub taps: Vec<TapStats>,
    /// 20 ms frames delivered to the callback.
    pub frames_out: u64,
}

/// Receives each 20 ms frame of captured audio.
pub(crate) type FrameSink = Box<dyn FnMut(&[f32]) + Send>;

#[cfg(windows)]
#[path = "audio_win.rs"]
mod imp;

#[cfg(not(windows))]
mod imp {
    use super::*;
    use crate::Error;

    pub fn list_apps() -> Result<Vec<AudioApp>> {
        Err(Error::Unsupported)
    }

    pub struct Capture;

    // Like the Windows one, it stops capturing when dropped (it has nothing to stop).
    impl Drop for Capture {
        fn drop(&mut self) {}
    }

    impl Capture {
        pub fn stats(&self) -> AudioStats {
            AudioStats::default()
        }
    }

    pub fn start(_source: AudioSource, _on_frame: FrameSink) -> Result<Capture> {
        Err(Error::Unsupported)
    }
}

/// A running capture. Dropping it stops everything.
pub struct AudioCapture {
    inner: imp::Capture,
}

impl AudioCapture {
    pub fn stats(&self) -> AudioStats {
        self.inner.stats()
    }
}

/// The programs that have an audio session on the default output device.
pub fn list_audio_apps() -> Result<Vec<AudioApp>> {
    imp::list_apps()
}

/// Starts capturing. `on_frame` gets `FRAME_SAMPLES * CHANNELS` interleaved
/// samples every 20 ms, from its own thread, silence included.
pub fn start_audio(
    source: AudioSource,
    on_frame: impl FnMut(&[f32]) + Send + 'static,
) -> Result<AudioCapture> {
    Ok(AudioCapture {
        inner: imp::start(source, Box::new(on_frame))?,
    })
}
