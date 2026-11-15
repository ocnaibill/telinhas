//! What the receiving side measured.

use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
struct Inner {
    started: Option<Instant>,
    decoded: u64,
    lost: u64,
    last_counter: Option<u32>,
    bytes: u64,
    latencies_ms: Vec<f64>,
}

/// Shared between the decoder thread and whoever prints the report.
#[derive(Default)]
pub struct Stats {
    inner: Mutex<Inner>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub decoded: u64,
    /// Frame numbers that never arrived, judged by gaps in the counter.
    pub lost: u64,
    pub seconds: f64,
    pub received_kbps: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_max_ms: f64,
}

impl Stats {
    /// Records one decoded frame: its counter, its measured latency and
    /// the size of the access unit that carried it.
    pub fn record(&self, counter: u32, latency_ms: f64, bytes: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner.started.get_or_insert_with(Instant::now);
        if let Some(last) = inner.last_counter {
            let gap = counter.wrapping_sub(last);
            // Ignore reordering; count only forward gaps as losses.
            if (2..1000).contains(&gap) {
                inner.lost += u64::from(gap - 1);
            }
        }
        inner.last_counter = Some(counter);
        inner.decoded += 1;
        inner.bytes += bytes as u64;
        inner.latencies_ms.push(latency_ms);
    }

    pub fn decoded(&self) -> u64 {
        self.inner.lock().unwrap().decoded
    }

    pub fn report(&self) -> Report {
        let inner = self.inner.lock().unwrap();
        let seconds = inner.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
        let mut sorted = inner.latencies_ms.clone();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| -> f64 {
            if sorted.is_empty() {
                return 0.0;
            }
            sorted[((sorted.len() - 1) as f64 * q).round() as usize]
        };
        Report {
            decoded: inner.decoded,
            lost: inner.lost,
            seconds,
            received_kbps: if seconds > 0.0 {
                inner.bytes as f64 * 8.0 / seconds / 1000.0
            } else {
                0.0
            },
            latency_p50_ms: at(0.5),
            latency_p95_ms: at(0.95),
            latency_max_ms: at(1.0),
        }
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frames={} lost={} over {:.1}s, {:.0} kbps, latency p50={:.1} ms p95={:.1} ms max={:.1} ms",
            self.decoded,
            self.lost,
            self.seconds,
            self.received_kbps,
            self.latency_p50_ms,
            self.latency_p95_ms,
            self.latency_max_ms,
        )
    }
}
