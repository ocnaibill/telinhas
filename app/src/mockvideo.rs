//! Test pictures standing in for the decoded video until the media layer is
//! connected: scrolling bars in I420, made by copying rows of a wide base
//! picture, so producing a frame is cheap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::video::Frame;

pub type Latest = Arc<Mutex<Arc<Frame>>>;

const SCROLL: usize = 256;

struct Base {
    width: u32,
    height: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

fn make_base(width: u32, height: u32, tint: u8) -> Base {
    let (w, h) = (width as usize, height as usize);
    let (yw, cw, ch) = (w + SCROLL, w / 2 + SCROLL / 2, h / 2);
    let mut y = vec![0u8; yw * h];
    for row in 0..h {
        for col in 0..yw {
            let bars = if (col / 64) % 2 == 0 { 30 } else { 0 };
            let grad = (row * 90 / h) as u8;
            y[row * yw + col] = 36 + bars + grad + ((col % SCROLL) / 5) as u8;
        }
    }
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];
    for row in 0..ch {
        for col in 0..cw {
            u[row * cw + col] = 105u8
                .wrapping_add(tint.wrapping_mul(34))
                .wrapping_add((col % 64) as u8 / 2);
            v[row * cw + col] = 145u8
                .wrapping_sub(tint.wrapping_mul(22))
                .wrapping_add((row * 70 / ch.max(1)) as u8);
        }
    }
    Base {
        width,
        height,
        y,
        u,
        v,
    }
}

fn make_frame(base: &Base, seq: u64) -> Frame {
    let (w, h) = (base.width as usize, base.height as usize);
    let shift = (seq as usize * 3) % SCROLL;
    let (yw, cw) = (w + SCROLL, w / 2 + SCROLL / 2);
    let mut y = Vec::with_capacity(w * h);
    for row in 0..h {
        y.extend_from_slice(&base.y[row * yw + shift..row * yw + shift + w]);
    }
    let mut u = Vec::with_capacity(w * h / 4);
    let mut v = Vec::with_capacity(w * h / 4);
    for row in 0..h / 2 {
        let start = row * cw + shift / 2;
        u.extend_from_slice(&base.u[start..start + w / 2]);
        v.extend_from_slice(&base.v[start..start + w / 2]);
    }
    Frame {
        seq,
        width: base.width,
        height: base.height,
        y,
        u,
        v,
    }
}

/// A running test picture: read `latest` for the newest frame.
pub struct MockSource {
    pub latest: Latest,
    stop: Arc<AtomicBool>,
}

impl MockSource {
    pub fn start(width: u32, height: u32, fps: u32, tint: u8) -> Self {
        let base = make_base(width, height, tint);
        let latest: Latest = Arc::new(Mutex::new(Arc::new(make_frame(&base, 0))));
        let stop = Arc::new(AtomicBool::new(false));
        let (handler_latest, handler_stop) = (Arc::clone(&latest), Arc::clone(&stop));
        std::thread::spawn(move || {
            let period = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
            let mut next = Instant::now();
            let mut seq = 1u64;
            while !handler_stop.load(Ordering::Relaxed) {
                *handler_latest.lock().unwrap() = Arc::new(make_frame(&base, seq));
                seq += 1;
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else if now - next > period * 5 {
                    next = now;
                }
            }
        });
        Self { latest, stop }
    }

    pub fn frame(&self) -> Arc<Frame> {
        Arc::clone(&self.latest.lock().unwrap())
    }
}

impl Drop for MockSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
