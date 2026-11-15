//! Per-IP limit on failed room joins (DEC-48).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Entries beyond this count trigger a sweep of idle IPs.
const SWEEP_THRESHOLD: usize = 10_000;

#[derive(Debug, Default)]
struct Entry {
    failures: VecDeque<Instant>,
    blocked_until: Option<Instant>,
}

/// Counts failed joins per IP. Once `max_failures` land inside `window`,
/// the IP is blocked for `block`.
#[derive(Debug)]
pub struct RateLimiter {
    max_failures: usize,
    window: Duration,
    block: Duration,
    entries: HashMap<IpAddr, Entry>,
}

impl RateLimiter {
    pub fn new(max_failures: usize, window: Duration, block: Duration) -> Self {
        Self {
            max_failures,
            window,
            block,
            entries: HashMap::new(),
        }
    }

    /// `Err(remaining)` while the IP is blocked.
    pub fn check(&mut self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let Some(entry) = self.entries.get_mut(&ip) else {
            return Ok(());
        };
        match entry.blocked_until {
            Some(until) if until > now => Err(until - now),
            Some(_) => {
                self.entries.remove(&ip);
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub fn record_failure(&mut self, ip: IpAddr, now: Instant) {
        if self.entries.len() > SWEEP_THRESHOLD {
            self.sweep(now);
        }
        let window = self.window;
        let entry = self.entries.entry(ip).or_default();
        while entry
            .failures
            .front()
            .is_some_and(|&t| now.duration_since(t) > window)
        {
            entry.failures.pop_front();
        }
        entry.failures.push_back(now);
        if entry.failures.len() >= self.max_failures {
            entry.blocked_until = Some(now + self.block);
            entry.failures.clear();
        }
    }

    fn sweep(&mut self, now: Instant) {
        let window = self.window;
        self.entries.retain(|_, e| {
            e.blocked_until.is_some_and(|until| until > now)
                || e.failures
                    .back()
                    .is_some_and(|&t| now.duration_since(t) <= window)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    fn limiter() -> RateLimiter {
        RateLimiter::new(10, Duration::from_secs(60), Duration::from_secs(300))
    }

    #[test]
    fn blocks_after_max_failures_within_the_window() {
        let mut l = limiter();
        let t0 = Instant::now();
        for i in 0..9 {
            l.record_failure(ip(1), t0 + Duration::from_secs(i));
            assert!(l.check(ip(1), t0 + Duration::from_secs(i)).is_ok());
        }
        l.record_failure(ip(1), t0 + Duration::from_secs(9));
        let remaining = l.check(ip(1), t0 + Duration::from_secs(10)).unwrap_err();
        assert!(remaining <= Duration::from_secs(300));
        assert!(remaining > Duration::from_secs(290));
    }

    #[test]
    fn other_ips_are_not_affected() {
        let mut l = limiter();
        let t0 = Instant::now();
        for _ in 0..10 {
            l.record_failure(ip(1), t0);
        }
        assert!(l.check(ip(1), t0).is_err());
        assert!(l.check(ip(2), t0).is_ok());
    }

    #[test]
    fn failures_outside_the_window_do_not_add_up() {
        let mut l = limiter();
        let t0 = Instant::now();
        for i in 0..9 {
            l.record_failure(ip(1), t0 + Duration::from_secs(i * 10));
        }
        // Spread over 80 s, so never 10 inside one 60 s window.
        assert!(l.check(ip(1), t0 + Duration::from_secs(81)).is_ok());
    }

    #[test]
    fn block_expires() {
        let mut l = limiter();
        let t0 = Instant::now();
        for _ in 0..10 {
            l.record_failure(ip(1), t0);
        }
        assert!(l.check(ip(1), t0 + Duration::from_secs(299)).is_err());
        assert!(l.check(ip(1), t0 + Duration::from_secs(301)).is_ok());
    }
}
