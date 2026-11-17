//! Frame pacing: picks the frames that land closest to an even rhythm.
//!
//! Windows Graphics Capture only delivers frames aligned to the monitor's
//! refresh, and rounds a requested minimum interval *up* to a multiple of it.
//! On a 144 Hz monitor a 60 fps request therefore becomes 48 fps (every
//! third refresh). Capturing at the full rate and choosing frames here gives
//! a true 60 fps instead (DEC-59).

/// Decides which of a stream of timestamped frames to keep.
#[derive(Debug, Clone)]
pub struct Pacer {
    period_us: i64,
    next_tick: Option<i64>,
    last: Option<i64>,
    last_gap_us: i64,
}

impl Pacer {
    /// Keeps about `fps` frames per second.
    pub fn new(fps: u32) -> Self {
        Self {
            period_us: 1_000_000 / i64::from(fps.max(1)),
            next_tick: None,
            last: None,
            last_gap_us: 0,
        }
    }

    /// `true` when the frame stamped `micros` should be kept. Timestamps only
    /// need to increase; their absolute value does not matter.
    pub fn accept(&mut self, micros: i64) -> bool {
        if let Some(last) = self.last {
            let gap = micros - last;
            if gap > 0 {
                self.last_gap_us = gap;
            }
        }
        self.last = Some(micros);

        let tick = *self.next_tick.get_or_insert(micros);
        // A frame within half a source interval of the tick is as close as
        // any later one can be, so waiting for the next would only be worse.
        if micros + self.last_gap_us / 2 < tick {
            return false;
        }
        let mut next = tick + self.period_us;
        if micros > next {
            // The source ran slower than the target (or paused): start over
            // from here instead of trying to catch up.
            next = micros + self.period_us;
        }
        self.next_tick = Some(next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of a source at `hz`, for `seconds`, as microsecond stamps.
    fn source(hz: f64, seconds: f64) -> Vec<i64> {
        (0..(hz * seconds) as usize)
            .map(|n| (n as f64 * 1_000_000.0 / hz) as i64)
            .collect()
    }

    fn kept(hz: f64, fps: u32, seconds: f64) -> Vec<i64> {
        let mut pacer = Pacer::new(fps);
        source(hz, seconds)
            .into_iter()
            .filter(|&t| pacer.accept(t))
            .collect()
    }

    #[test]
    fn sixty_from_a_144_hz_source_is_really_sixty() {
        let frames = kept(144.0, 60, 10.0);
        assert!((599..=601).contains(&frames.len()), "{}", frames.len());
        // Gaps are two or three refreshes (13.9 or 20.8 ms), never worse.
        for pair in frames.windows(2) {
            let gap = pair[1] - pair[0];
            assert!((13_000..=21_500).contains(&gap), "gap {gap} us");
        }
    }

    #[test]
    fn thirty_from_a_144_hz_source() {
        let frames = kept(144.0, 30, 10.0);
        assert!((299..=301).contains(&frames.len()), "{}", frames.len());
    }

    #[test]
    fn a_source_slower_than_the_target_passes_everything() {
        let frames = kept(48.0, 60, 10.0);
        assert_eq!(frames.len(), 480);
    }

    #[test]
    fn sixty_from_a_60_hz_source_keeps_every_frame() {
        let frames = kept(60.0, 60, 10.0);
        assert!((598..=600).contains(&frames.len()), "{}", frames.len());
    }

    #[test]
    fn sixty_from_a_72_hz_source() {
        let frames = kept(72.0, 60, 10.0);
        assert!((595..=605).contains(&frames.len()), "{}", frames.len());
    }

    #[test]
    fn resumes_after_a_pause() {
        let mut pacer = Pacer::new(60);
        assert!(pacer.accept(0));
        // Nothing for 5 s (a still screen), then motion again.
        assert!(pacer.accept(5_000_000));
        assert!(pacer.accept(5_017_000));
    }
}
