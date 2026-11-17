//! Where the sender's pictures come from: the live screen or a generated
//! pattern, always as tightly packed BGRA with even width and height.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use telinhas_capture::{self as capture, Options as CaptureOptions};

use crate::capture_cmd::parse_source;

/// Bits in the stamp: 32 for the frame counter, 32 for the capture time.
const STAMP_BITS: usize = 64;
const BLOCK: usize = 8;
/// Narrowest picture the stamp fits in.
pub const STAMP_MIN_WIDTH: usize = STAMP_BITS * BLOCK;

#[derive(Debug, Clone)]
pub struct FrameSpec {
    /// `synthetic`, `monitor:N`, `window:HANDLE` or `window:TEXT`.
    pub source: String,
    pub fps: u32,
    /// Size of the synthetic picture.
    pub width: u32,
    pub height: u32,
    pub noise: u8,
    pub cursor: bool,
    /// Leave the system's capture border on.
    pub border: bool,
}

#[derive(Default)]
struct Latest {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

pub struct FrameSource {
    latest: Arc<Mutex<Latest>>,
    session: Option<capture::Session>,
    synthetic: Option<(u32, u32, u8, u32)>,
    note: String,
}

impl FrameSource {
    pub fn open(spec: &FrameSpec) -> Result<Self> {
        if spec.source == "synthetic" {
            return Ok(Self {
                latest: Arc::default(),
                session: None,
                synthetic: Some((spec.width & !1, spec.height & !1, spec.noise, 0x9E37_79B9)),
                note: "synthetic picture".to_owned(),
            });
        }
        capture::init_dpi_awareness();
        let source = parse_source(&spec.source)?;
        let latest: Arc<Mutex<Latest>> = Arc::default();
        let handler_latest = Arc::clone(&latest);
        let session = capture::start(
            source,
            CaptureOptions {
                cursor: spec.cursor,
                border: spec.border,
                max_fps: Some(spec.fps),
            },
            move |frame| {
                // Even sizes only: H.264 needs them.
                let (w, h) = (frame.width & !1, frame.height & !1);
                let mut latest = handler_latest.lock().unwrap();
                latest.width = w;
                latest.height = h;
                latest.bgra.clear();
                let row = w as usize * 4;
                for line in frame.bgra.chunks(frame.stride).take(h as usize) {
                    latest.bgra.extend_from_slice(&line[..row]);
                }
            },
        )?;
        let note = format!(
            "cursor: {}; border: {}",
            if session.cursor_applied() {
                "setting accepted"
            } else {
                "setting NOT accepted"
            },
            session.border_status()
        );
        let waited = Instant::now();
        while latest.lock().unwrap().bgra.is_empty() {
            if waited.elapsed() > Duration::from_secs(5) {
                bail!("no picture arrived in 5 s: the source is still or not capturable");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Self {
            latest,
            session: Some(session),
            synthetic: None,
            note,
        })
    }

    /// What the system said about the cursor and border, in words.
    pub fn note(&self) -> &str {
        &self.note
    }

    pub fn size(&self) -> (u32, u32) {
        match self.synthetic {
            Some((w, h, _, _)) => (w, h),
            None => {
                let latest = self.latest.lock().unwrap();
                (latest.width, latest.height)
            }
        }
    }

    /// Puts the current picture in `out` and returns its size. The screen is
    /// repeated when nothing changed, as a still screen delivers no frames.
    pub fn fill(&mut self, tick: u64, out: &mut Vec<u8>) -> (u32, u32) {
        if let Some((w, h, noise, rng)) = &mut self.synthetic {
            synthetic(*w, *h, tick, *noise, rng, out);
            return (*w, *h);
        }
        let latest = self.latest.lock().unwrap();
        out.clear();
        out.extend_from_slice(&latest.bgra);
        (latest.width, latest.height)
    }

    pub fn stop(self) {
        if let Some(session) = self.session {
            session.stop();
        }
    }
}

/// A moving gradient and square, with optional noise, as BGRA.
pub fn synthetic(width: u32, height: u32, index: u64, noise: u8, rng: &mut u32, out: &mut Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    out.clear();
    out.resize(w * h * 4, 255);
    let mask: u8 = if noise == 0 {
        0
    } else {
        (1u8 << noise.min(7)) - 1
    };
    let t = index as usize;
    let side = h / 6;
    let (sx, sy) = ((t * 7) % (w - side), h / 3 + (t * 3) % (h / 3));
    for y in 0..h {
        for x in 0..w {
            let mut v = 48 + (((x + y / 2 + t * 3) / 4) % 64) as u8 * 2;
            if x >= sx && x < sx + side && y >= sy && y < sy + side {
                v = 235;
            }
            if mask != 0 {
                *rng ^= *rng << 13;
                *rng ^= *rng >> 17;
                *rng ^= *rng << 5;
                v = v.saturating_add((*rng as u8) & mask);
            }
            let at = (y * w + x) * 4;
            out[at] = v;
            out[at + 1] = v;
            out[at + 2] = v;
        }
    }
}

/// Paints the frame counter and capture time into the top row, so the
/// receiver can read them back from the decoded picture (see `source.rs`).
/// Does nothing on a picture too narrow to hold the stamp.
pub fn paint_stamp_bgra(bgra: &mut [u8], width: u32, counter: u32, captured: u32) {
    let width = width as usize;
    if width < STAMP_MIN_WIDTH || bgra.len() < BLOCK * width * 4 {
        return;
    }
    let value = (u64::from(counter) << 32) | u64::from(captured);
    for bit in 0..STAMP_BITS {
        let shade = if (value >> (STAMP_BITS - 1 - bit)) & 1 == 1 {
            255
        } else {
            0
        };
        for row in 0..BLOCK {
            let start = (row * width + bit * BLOCK) * 4;
            for byte in &mut bgra[start..start + BLOCK * 4] {
                *byte = shade;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::read_stamp;
    use openh264::formats::{BgraSliceU8, YUVBuffer};

    #[test]
    fn a_painted_stamp_survives_the_color_conversion() {
        let (w, h) = (640u32, 64u32);
        let mut bgra = vec![128u8; w as usize * h as usize * 4];
        paint_stamp_bgra(&mut bgra, w, 123_456, 987_654);
        let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&bgra, (w as usize, h as usize)));
        assert_eq!(read_stamp(&yuv), (123_456, 987_654));
    }

    #[test]
    fn a_narrow_picture_is_left_alone() {
        let mut bgra = vec![7u8; 100 * 16 * 4];
        paint_stamp_bgra(&mut bgra, 100, 1, 2);
        assert!(bgra.iter().all(|&b| b == 7));
    }

    #[test]
    fn synthetic_source_fills_the_requested_size() {
        let mut source = FrameSource::open(&FrameSpec {
            source: "synthetic".into(),
            fps: 30,
            width: 641,
            height: 361,
            noise: 2,
            cursor: true,
            border: true,
        })
        .unwrap();
        let mut out = Vec::new();
        let (w, h) = source.fill(0, &mut out);
        assert_eq!((w, h), (640, 360));
        assert_eq!(out.len(), 640 * 360 * 4);
    }
}
