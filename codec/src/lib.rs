//! Video encoders behind one trait (DEC-39).
//!
//! Two backends: hardware H.264 through the FFmpeg libraries (NVENC, AMF and Quick Sync, behind
//! the `ffmpeg` feature) and software H.264 through `openh264`. The app asks
//! for [`Backend::Auto`], and when hardware is not available it gets the
//! software encoder together with the reason, so the interface can warn that
//! the stream may stutter (DEC-15, RF-14).
//!
//! The rate policy follows the measurements in `docs/PLANO-M3.md`: constant
//! quality with a bitrate ceiling, never constant bitrate, because constant
//! bitrate pads a still screen up to the full rate.

use std::fmt;

mod software;

#[cfg(feature = "ffmpeg")]
mod hardware;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The requested encoder does not exist on this machine.
    Unavailable(String),
    /// The encoder failed while working.
    Backend(String),
    /// The input does not match what the encoder was opened for.
    BadInput(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable(text) => write!(f, "encoder unavailable: {text}"),
            Error::Backend(text) => write!(f, "encoder error: {text}"),
            Error::BadInput(text) => write!(f, "bad input: {text}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// How many bits to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    /// Constant quality, lower is better (NVENC `cq`; 23 is high quality).
    pub quality: u32,
    /// The most the encoder may use on average, in kbps.
    pub max_kbps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub format: Format,
    pub rate: Rate,
    /// A keyframe every this many frames. `0` means on demand only (new
    /// viewer, picture loss), with a long safety interval.
    pub keyframe_every: u32,
}

/// One picture to encode: BGRA, as the capture delivers it.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    /// Bytes per row; at least `width * 4`.
    pub stride: usize,
    pub bgra: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    /// H.264 in Annex B form (start codes), ready for the RTP packetizer.
    pub data: Vec<u8>,
    pub keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    pub hardware: bool,
}

pub trait Encoder: Send {
    /// Encodes one picture. May return no packet (the encoder skipped the
    /// frame) or more than one.
    fn encode(&mut self, frame: &Frame<'_>) -> Result<Vec<EncodedPacket>>;

    /// The next picture will be a keyframe (a new viewer, or picture loss).
    fn request_keyframe(&mut self);

    /// Changes the bitrate ceiling, such as when congestion control moves.
    /// May cost a keyframe.
    fn set_max_kbps(&mut self, kbps: u32) -> Result<()>;

    fn info(&self) -> Info;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Hardware if it works, software otherwise.
    Auto,
    Hardware,
    Software,
}

pub struct Created {
    pub encoder: Box<dyn Encoder>,
    /// Set when hardware was wanted but software was used, and why.
    pub fallback_reason: Option<String>,
}

/// Opens an encoder.
pub fn create(backend: Backend, options: Options) -> Result<Created> {
    match backend {
        Backend::Software => Ok(Created {
            encoder: Box::new(software::Software::open(options)?),
            fallback_reason: None,
        }),
        Backend::Hardware => Ok(Created {
            encoder: open_hardware(options)?,
            fallback_reason: None,
        }),
        Backend::Auto => match open_hardware(options) {
            Ok(encoder) => Ok(Created {
                encoder,
                fallback_reason: None,
            }),
            Err(reason) => Ok(Created {
                encoder: Box::new(software::Software::open(options)?),
                fallback_reason: Some(reason.to_string()),
            }),
        },
    }
}

#[cfg(feature = "ffmpeg")]
fn open_hardware(options: Options) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(hardware::Hardware::open(options)?))
}

#[cfg(not(feature = "ffmpeg"))]
fn open_hardware(_options: Options) -> Result<Box<dyn Encoder>> {
    Err(Error::Unavailable(
        "this build has no hardware encoder (built without the `ffmpeg` feature)".to_owned(),
    ))
}

/// Whether an Annex B access unit holds an IDR picture (NAL unit type 5).
pub fn contains_idr(annex_b: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < annex_b.len() {
        if annex_b[i] == 0 && annex_b[i + 1] == 0 && annex_b[i + 2] == 1 {
            if annex_b[i + 3] & 0x1f == 5 {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// Checks the input against what the encoder was opened for, and returns a
/// tightly packed copy of the rows when the stride has padding.
pub(crate) fn packed_rows<'a>(
    frame: &Frame<'a>,
    format: Format,
    scratch: &'a mut Vec<u8>,
) -> Result<&'a [u8]> {
    if frame.width != format.width || frame.height != format.height {
        return Err(Error::BadInput(format!(
            "frame is {}x{} but the encoder was opened for {}x{}",
            frame.width, frame.height, format.width, format.height
        )));
    }
    let row = frame.width as usize * 4;
    let needed = row * frame.height as usize;
    if frame.stride == row {
        return frame
            .bgra
            .get(..needed)
            .ok_or_else(|| Error::BadInput("frame data is too short".to_owned()));
    }
    scratch.clear();
    for line in frame.bgra.chunks(frame.stride).take(frame.height as usize) {
        let line = line
            .get(..row)
            .ok_or_else(|| Error::BadInput("a row is too short".to_owned()))?;
        scratch.extend_from_slice(line);
    }
    if scratch.len() != needed {
        return Err(Error::BadInput("frame data is too short".to_owned()));
    }
    Ok(scratch.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_idr_nal_units() {
        let idr = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x65, 9, 9];
        let p_frame = [0, 0, 0, 1, 0x41, 1, 2, 3];
        assert!(contains_idr(&idr));
        assert!(!contains_idr(&p_frame));
        assert!(!contains_idr(&[]));
    }

    #[test]
    fn packs_rows_with_padding() {
        let format = Format {
            width: 2,
            height: 2,
            fps: 30,
        };
        // 2x2 BGRA with a 12-byte stride (4 bytes of padding per row).
        let data: Vec<u8> = (0..24).collect();
        let frame = Frame {
            width: 2,
            height: 2,
            stride: 12,
            bgra: &data,
        };
        let mut scratch = Vec::new();
        let packed = packed_rows(&frame, format, &mut scratch).unwrap();
        assert_eq!(
            packed,
            &[0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19]
        );
    }

    #[test]
    fn rejects_a_frame_of_the_wrong_size() {
        let format = Format {
            width: 4,
            height: 4,
            fps: 30,
        };
        let data = vec![0u8; 16];
        let frame = Frame {
            width: 2,
            height: 2,
            stride: 8,
            bgra: &data,
        };
        let mut scratch = Vec::new();
        assert!(matches!(
            packed_rows(&frame, format, &mut scratch),
            Err(Error::BadInput(_))
        ));
    }
}
