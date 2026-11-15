//! Synthetic video source: a moving pattern with a frame counter and a
//! capture timestamp painted into the top row, so the receiver can read both
//! back from decoded pixels and measure glass-to-glass latency.

use std::time::{SystemTime, UNIX_EPOCH};

use openh264::formats::{YUVBuffer, YUVSource};

/// Bits in the stamp: 32 for the frame counter, 32 for the capture time.
const STAMP_BITS: usize = 64;
/// Each bit is a square block of this many pixels, to survive compression.
const BLOCK: usize = 8;
/// Smallest width that fits the stamp.
pub const MIN_WIDTH: usize = STAMP_BITS * BLOCK;

/// Wall-clock microseconds, wrapped to 32 bits (about 71 minutes).
pub fn now_micros() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u32)
}

/// Signed difference `now - then`, in milliseconds, tolerant of wrap-around.
pub fn elapsed_ms(then: u32, now: u32) -> f64 {
    f64::from(now.wrapping_sub(then) as i32) / 1000.0
}

pub struct Source {
    width: usize,
    height: usize,
    counter: u32,
    /// Per-pixel noise mask: 0 is none. More noise means more bits to
    /// encode, which stands in for busy real content (the pattern alone is
    /// trivial to compress).
    noise_mask: u8,
    rng: u32,
}

impl Source {
    /// `noise_level` runs from 0 (none) to 5 (heavy); each step doubles the
    /// amplitude, and so roughly the bitrate the encoder wants.
    pub fn new(width: usize, height: usize, noise_level: u8) -> Self {
        assert!(width >= MIN_WIDTH, "width must be at least {MIN_WIDTH}");
        assert!(width.is_multiple_of(2) && height.is_multiple_of(2) && height > BLOCK);
        Self {
            width,
            height,
            counter: 0,
            noise_mask: match noise_level {
                0 => 0,
                n => (1u8 << (n.min(5) - 1)) | ((1u8 << (n.min(5) - 1)) - 1),
            },
            rng: 0x9E37_79B9,
        }
    }

    /// Next frame, stamped with its number and the current time.
    pub fn next_frame(&mut self) -> Frame {
        let counter = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let captured = now_micros();

        let (w, h) = (self.width, self.height);
        let mut yuv = vec![128u8; w * h * 3 / 2];
        let t = counter as usize;

        let (luma, _) = yuv.split_at_mut(w * h);
        for (y, row) in luma.chunks_exact_mut(w).enumerate() {
            for (x, px) in row.iter_mut().enumerate() {
                *px = 48 + (((x + y / 2 + t * 3) / 4) % 64) as u8 * 2;
            }
        }
        if self.noise_mask != 0 {
            let mut state = self.rng;
            for px in luma.iter_mut() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *px = px.saturating_add((state as u8) & self.noise_mask);
            }
            self.rng = state;
        }
        // A moving square, so there is real motion to encode.
        let side = h / 6;
        let sx = (t * 7) % (w - side);
        let sy = h / 3 + ((t * 3) % (h / 3));
        for row in luma.chunks_exact_mut(w).skip(sy).take(side) {
            row[sx..sx + side].fill(235);
        }
        paint_stamp(luma, w, counter, captured);

        Frame {
            counter,
            captured,
            yuv: YUVBuffer::from_vec(yuv, w, h),
        }
    }
}

pub struct Frame {
    pub counter: u32,
    pub captured: u32,
    pub yuv: YUVBuffer,
}

fn paint_stamp(luma: &mut [u8], stride: usize, counter: u32, captured: u32) {
    let value = (u64::from(counter) << 32) | u64::from(captured);
    for bit in 0..STAMP_BITS {
        let on = (value >> (STAMP_BITS - 1 - bit)) & 1 == 1;
        let shade = if on { 235 } else { 16 };
        for row in 0..BLOCK {
            let start = row * stride + bit * BLOCK;
            luma[start..start + BLOCK].fill(shade);
        }
    }
}

/// Reads `(counter, captured)` back from a decoded luma plane.
pub fn read_stamp(yuv: &impl YUVSource) -> (u32, u32) {
    let luma = yuv.y();
    let (stride, _, _) = yuv.strides();
    let mut value = 0u64;
    for bit in 0..STAMP_BITS {
        // Average the centre of the block, away from compression ringing.
        let mut sum = 0u32;
        for row in 2..BLOCK - 2 {
            for col in 2..BLOCK - 2 {
                sum += u32::from(luma[row * stride + bit * BLOCK + col]);
            }
        }
        let mean = sum / ((BLOCK as u32 - 4) * (BLOCK as u32 - 4));
        value = (value << 1) | u64::from(mean > 125);
    }
    ((value >> 32) as u32, value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_survives_a_round_trip_through_the_luma_plane() {
        let mut source = Source::new(640, 360, 2);
        for _ in 0..5 {
            let frame = source.next_frame();
            assert_eq!(
                read_stamp(&frame.yuv),
                (frame.counter, frame.captured),
                "frame {}",
                frame.counter
            );
        }
    }

    #[test]
    fn elapsed_handles_wrap_around() {
        assert!((elapsed_ms(u32::MAX - 999, 1000) - 2.0).abs() < 1e-9);
        assert!((elapsed_ms(5_000, 3_000) + 2.0).abs() < 1e-9);
    }
}
