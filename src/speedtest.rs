//! One throughput measurement from this host, over whatever path the host uses.
//! It never calls the router API: these numbers describe this machine's path,
//! not the device's radio. Plain HTTP only, because the binary has no TLS stack.

use crate::config::Speedtest;
use serde::Serialize;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const CHUNK: usize = 64 * 1024;

/// The phases the agent is allowed, in one place: the watchdog and the agent
/// read the same numbers, so a live thread is never reported dead.
pub const CONNECT_SECS: u64 = 5;
pub const RESPONSE_SECS: u64 = 10;
pub const READ_SLACK_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Download,
    Upload,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedProgress {
    pub phase: Phase,
    pub bytes: u64,
    /// Bits per second. Never pass this to `widgets::bps`, which takes bytes.
    pub rate_bps: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpeedResult {
    /// Bits per second. Never pass this to `widgets::bps`, which takes bytes.
    pub download_bps: f64,
    /// Bits per second. Zero when the upload did not run or did not finish.
    pub upload_bps: f64,
    pub ttfb_ms: f64,
    pub down_bytes: u64,
    pub up_bytes: u64,
    /// Seconds from the first download byte to the last one.
    pub down_secs: f64,
    /// Seconds from the first upload byte written to the last one.
    pub up_secs: f64,
    pub download_url: String,
    pub upload_url: String,
    /// True when a transfer stopped on an error: the numbers are real but
    /// cover less than the requested budget.
    pub partial: bool,
    /// True when the window was shorter than three seconds, which on a fast
    /// link means TCP slow start dominated the average.
    pub short_window: bool,
    /// Why it is partial, or what the numbers are worth.
    pub note: Option<String>,
    /// Unix seconds when the run started, so the screen can say how old it is.
    pub at_unix: u64,
}

pub fn rate_bps(bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        0.0
    } else {
        bytes as f64 * 8.0 / secs
    }
}

/// The total budget for one upload body, not restarted per write
/// (`ureq-3.5.1/src/config.rs:884-892`). A 10 MB body gets 30 s, which floors a
/// measurable uplink near 2.7 Mbps; anything slower stops and reports the bytes
/// it did send.
pub fn upload_budget(cfg: &Speedtest) -> Duration {
    Duration::from_secs(20 + cfg.upload_mb)
}

/// Every phase of a run, worst case. `App`'s watchdog uses this, and so does
/// the agent's timeout wiring, so the two cannot drift apart.
pub fn worst_case_secs(cfg: &Speedtest) -> u64 {
    let down = CONNECT_SECS + RESPONSE_SECS + cfg.duration_secs + READ_SLACK_SECS;
    let up = if cfg.download_only {
        0
    } else {
        CONNECT_SECS + upload_budget(cfg).as_secs() + RESPONSE_SECS
    };
    down + up + 10
}

/// What one read window measured: the bytes, the window they arrived in, and
/// the error that stopped it early, if any.
pub struct ReadOutcome {
    pub bytes: u64,
    pub secs: f64,
    pub error: Option<std::io::Error>,
}

/// Read until the budget, the byte cap or a cancellation wins. The window runs
/// from the first payload byte to the last one, so connect time, server think
/// time and a socket that closes slowly stay out of the rate. A read error
/// keeps the bytes already measured and hands the error back beside them.
pub fn read_for<R: Read>(
    reader: &mut R,
    budget: Duration,
    max_bytes: u64,
    cancel: &AtomicBool,
    mut tick: impl FnMut(SpeedProgress),
) -> ReadOutcome {
    let mut buf = vec![0u8; CHUNK];
    let deadline = Instant::now() + budget;
    let mut bytes: u64 = 0;
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    let mut last_tick = Instant::now();
    let mut error = None;
    loop {
        if cancel.load(Ordering::Relaxed) || bytes >= max_bytes || Instant::now() >= deadline {
            break;
        }
        let want = ((max_bytes - bytes) as usize).min(CHUNK);
        let n = match reader.read(&mut buf[..want]) {
            Ok(n) => n,
            Err(e) => {
                error = Some(e);
                break;
            }
        };
        if n == 0 {
            break;
        }
        let now = Instant::now();
        let start = *first.get_or_insert(now);
        last = Some(now);
        bytes += n as u64;
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = now;
            tick(SpeedProgress {
                phase: Phase::Download,
                bytes,
                rate_bps: rate_bps(bytes, now.duration_since(start)),
            });
        }
    }
    let secs = match (first, last) {
        (Some(f), Some(l)) => l.duration_since(f).as_secs_f64(),
        _ => 0.0,
    };
    // One chunk can measure as zero; a floor keeps the division finite without
    // pretending the read was long.
    let secs = if bytes > 0 && secs < 0.001 {
        0.001
    } else {
        secs
    };
    if first.is_some() {
        // One last tick, so the panel does not freeze on the previous sample
        // for the whole upload.
        tick(SpeedProgress {
            phase: Phase::Download,
            bytes,
            rate_bps: rate_bps(bytes, Duration::from_secs_f64(secs.max(0.001))),
        });
    }
    ReadOutcome { bytes, secs, error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rate_is_bits_per_second() {
        assert_eq!(rate_bps(1_000_000, Duration::from_secs(1)), 8_000_000.0);
        assert_eq!(rate_bps(500_000, Duration::from_millis(500)), 8_000_000.0);
        assert_eq!(rate_bps(10, Duration::from_secs(0)), 0.0);
    }

    #[test]
    fn the_byte_cap_stops_the_read_exactly() {
        let mut src = Cursor::new(vec![0u8; 2 * 1024 * 1024]);
        let open = AtomicBool::new(false);
        let out = read_for(&mut src, Duration::from_secs(60), 1_000_000, &open, |_| {});
        assert_eq!(out.bytes, 1_000_000, "the cap is a limit, not a hint");
        assert!(out.error.is_none());
    }

    #[test]
    fn a_zero_budget_reads_nothing() {
        let mut src = Cursor::new(vec![7u8; 4096]);
        let open = AtomicBool::new(false);
        let out = read_for(&mut src, Duration::ZERO, 1_000_000, &open, |_| {});
        assert_eq!(out.bytes, 0);
    }

    #[test]
    fn a_cancelled_read_stops_without_an_error() {
        let mut src = Cursor::new(vec![0u8; 1024 * 1024]);
        let cancel = AtomicBool::new(true);
        let out = read_for(
            &mut src,
            Duration::from_secs(60),
            1_000_000,
            &cancel,
            |_| {},
        );
        assert_eq!(out.bytes, 0);
        assert!(out.error.is_none());
    }

    #[test]
    fn bytes_read_before_an_error_are_kept() {
        struct FailsAfter(usize);
        impl Read for FailsAfter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::other("link dropped"));
                }
                let n = buf.len().min(self.0);
                self.0 -= n;
                Ok(n)
            }
        }
        let mut src = FailsAfter(32 * 1024);
        let open = AtomicBool::new(false);
        let out = read_for(&mut src, Duration::from_secs(60), 1_000_000, &open, |_| {});
        assert_eq!(out.bytes, 32 * 1024);
        assert!(
            out.error.is_some(),
            "the caller must know the number is partial"
        );
    }

    #[test]
    fn a_stall_after_the_last_byte_is_not_in_the_window() {
        // The server closing the socket slowly is not a slow link: the window
        // ends at the last payload byte, not when read() finally returns zero.
        struct SlowEof {
            sent: bool,
        }
        impl Read for SlowEof {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if !self.sent {
                    self.sent = true;
                    buf[..4096].fill(0);
                    return Ok(4096);
                }
                std::thread::sleep(Duration::from_millis(200));
                Ok(0)
            }
        }
        let mut src = SlowEof { sent: false };
        let open = AtomicBool::new(false);
        let out = read_for(&mut src, Duration::from_secs(60), 1_000_000, &open, |_| {});
        assert_eq!(out.bytes, 4096);
        assert!(
            out.secs < 0.15,
            "the 200 ms tail entered the window: {}",
            out.secs
        );
    }

    #[test]
    fn the_worst_case_covers_every_phase() {
        // The watchdog reads this, so it must be the sum of the phases the
        // agent is given, not a guess.
        let cfg = Speedtest::default();
        let floor = CONNECT_SECS * 2 + RESPONSE_SECS * 2 + cfg.duration_secs + cfg.upload_mb + 20;
        assert!(
            worst_case_secs(&cfg) > floor,
            "{} <= {}",
            worst_case_secs(&cfg),
            floor
        );
    }
}
