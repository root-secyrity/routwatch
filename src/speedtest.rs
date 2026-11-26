//! One throughput measurement from this host, over whatever path the host uses.
//! It never calls the router API: these numbers describe this machine's path,
//! not the device's radio. Plain HTTP only, because the binary has no TLS stack.

use crate::config::Speedtest;
use serde::Serialize;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use ureq::Agent;

pub const CHUNK: usize = 64 * 1024;

/// The phases the agent is allowed, in one place: the watchdog and the agent
/// read the same numbers, so a live thread is never reported dead.
pub const CONNECT_SECS: u64 = 5;
pub const RESPONSE_SECS: u64 = 10;
pub const READ_SLACK_SECS: u64 = 5;
/// Worst case for the two ping sets, the quiet window before them and the pause
/// around them. Added to the transfer budget so the watchdog never kills a live
/// run: four pings at `-w 6` plus the warm window fit inside this.
pub const PING_BUDGET_SECS: u64 = 20;

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
    down + up + 10 + PING_BUDGET_SECS
}

/// What one read window measured: the bytes, the window they arrived in, and
/// the error that stopped it early, if any.
pub struct ReadOutcome {
    pub bytes: u64,
    pub secs: f64,
    pub error: Option<std::io::Error>,
}

/// Read until the budget, the byte cap or a cancellation wins. The window runs
/// from just before the first read to the last payload byte: server think time
/// before the first read and a socket that closes slowly stay out of the rate,
/// while a slow first chunk is counted, because its bytes were transferred
/// inside the window. A read error keeps the bytes already measured and hands
/// the error back beside them.
pub fn read_for<R: Read>(
    reader: &mut R,
    budget: Duration,
    max_bytes: u64,
    cancel: &AtomicBool,
    mut tick: impl FnMut(SpeedProgress),
) -> ReadOutcome {
    let mut buf = vec![0u8; CHUNK];
    let deadline = Instant::now() + budget;
    // Opened here, not after the first read returns: a transfer that completes
    // in one chunk would otherwise divide its bytes by the microseconds between
    // the read returning and the timestamp being taken.
    let started = Instant::now();
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
        let start = *first.get_or_insert(started);
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

/// Replace the `{bytes}` placeholder with the byte cap, so the server is asked
/// for exactly what the reader will take. A URL without the placeholder is used
/// as written.
pub fn download_target(cfg: &Speedtest) -> String {
    cfg.download_url
        .replace("{bytes}", &(cfg.max_mb * 1_000_000).to_string())
}

/// The upload body, metered. It counts what left this host, stops when `Esc`
/// asks, and times itself from the first byte written to the last, so connect,
/// headers and the server's think time are not charged to the uplink.
pub struct UploadMeter<'a> {
    left: u64,
    sent: u64,
    cancel: &'a AtomicBool,
    first: Option<Instant>,
    last: Instant,
    last_tick: Instant,
    progress: &'a mut dyn FnMut(SpeedProgress),
}

impl<'a> UploadMeter<'a> {
    pub fn new(
        bytes: u64,
        cancel: &'a AtomicBool,
        progress: &'a mut dyn FnMut(SpeedProgress),
    ) -> Self {
        let now = Instant::now();
        UploadMeter {
            left: bytes,
            sent: 0,
            cancel,
            first: None,
            last: now,
            last_tick: now,
            progress,
        }
    }

    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// The measured window, not the time `send` took.
    pub fn secs(&self) -> f64 {
        self.first
            .map(|f| self.last.duration_since(f).as_secs_f64())
            .unwrap_or(0.0)
    }
}

impl Read for UploadMeter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) || self.left == 0 {
            return Ok(0);
        }
        let n = ((buf.len() as u64).min(self.left).min(CHUNK as u64)) as usize;
        buf[..n].fill(0);
        let now = Instant::now();
        let start = *self.first.get_or_insert(now);
        self.last = now;
        self.left -= n as u64;
        self.sent += n as u64;
        if self.last_tick.elapsed() >= Duration::from_millis(250) {
            self.last_tick = now;
            (self.progress)(SpeedProgress {
                phase: Phase::Upload,
                bytes: self.sent,
                rate_bps: rate_bps(self.sent, now.duration_since(start)),
            });
        }
        Ok(n)
    }
}

/// One full test: download to the byte cap or the time budget, then one upload
/// unless the config skips it. Runs on a worker thread; `progress` is called
/// from this thread. A transfer that fails after some bytes returns `Ok` with
/// `partial: true` and the reason in `note`, because a fifteen second
/// measurement is still a measurement.
pub fn run(
    cfg: &Speedtest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(SpeedProgress),
) -> Result<SpeedResult, String> {
    let cfg = cfg.sanitised();
    let at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let agent: Agent = Agent::config_builder()
        .timeout_resolve(Some(Duration::from_secs(CONNECT_SECS)))
        .timeout_connect(Some(Duration::from_secs(CONNECT_SECS)))
        .timeout_recv_response(Some(Duration::from_secs(RESPONSE_SECS)))
        .timeout_recv_body(Some(Duration::from_secs(
            cfg.duration_secs + READ_SLACK_SECS,
        )))
        .timeout_send_body(Some(upload_budget(&cfg)))
        // A dead socket must not hold the whole window: this is the per-read
        // cut, and the totals above are the outer bound.
        .timeout_per_read(Some(Duration::from_secs(3)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();

    let target = download_target(&cfg);
    let wanted = cfg.max_mb * 1_000_000;
    let started = Instant::now();
    let res = agent.get(&target).call().map_err(|e| {
        format!("download request failed: {e} (redirects and TLS are refused; plain HTTP only)")
    })?;
    let status = res.status();
    if !status.is_success() {
        return Err(format!(
            "download answered {status}; a 3xx means the endpoint moved, and this binary refuses redirects"
        ));
    }
    // A captive portal or an out-of-credit proxy answers 200 with a small page.
    // Without this check such a page reads in a millisecond and reports a
    // spectacular speed that never crossed the link.
    if let Some(declared) = res
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        if declared < wanted {
            return Err(format!(
                "download answered with {declared} bytes, {wanted} were asked: a portal or a proxy answered instead"
            ));
        }
    }
    if res.headers().get("content-encoding").is_some() {
        return Err(
            "download is content-encoded: the byte count would not be the wire cost".into(),
        );
    }
    let ttfb = started.elapsed();
    let mut body = res.into_body();
    let mut reader = body.as_reader();
    let out = read_for(
        &mut reader,
        Duration::from_secs(cfg.duration_secs),
        wanted,
        cancel,
        &mut progress,
    );
    drop(reader);
    drop(body);
    let down_bytes = out.bytes;
    let down_secs = out.secs;
    if down_bytes == 0 || down_secs <= 0.0 {
        return Err(match out.error {
            Some(e) => format!("download read failed: {e}"),
            None => "download produced no bytes".into(),
        });
    }
    let download_bps = rate_bps(down_bytes, Duration::from_secs_f64(down_secs));
    let mut result = SpeedResult {
        download_bps,
        upload_bps: 0.0,
        ttfb_ms: ttfb.as_secs_f64() * 1000.0,
        down_bytes,
        up_bytes: 0,
        down_secs,
        up_secs: 0.0,
        download_url: target,
        upload_url: cfg.upload_url.clone(),
        partial: false,
        short_window: down_secs < 3.0,
        note: None,
        at_unix,
    };
    if out.error.is_some() {
        result.partial = true;
        result.note = out.error.map(|e| format!("download stopped: {e}"));
        return Ok(result);
    }
    if cfg.download_only || cancel.load(Ordering::Relaxed) {
        return Ok(result);
    }

    let upload_bytes = cfg.upload_mb * 1_000_000;
    let (up, sent, up_secs) = {
        let mut meter = UploadMeter::new(upload_bytes, cancel, &mut progress);
        // A reader body has no known size, so ureq frames it chunked
        // (ureq-3.5.1/src/run.rs:339). Both the mock and the default endpoint
        // accept that; measured 2026-11-15 on speed.cloudflare.com/__up.
        let body = ureq::SendBody::from_reader(&mut meter);
        let res = agent.post(&cfg.upload_url).send(body);
        (res, meter.sent(), meter.secs())
    };
    result.up_bytes = sent;
    result.up_secs = up_secs;
    let up = match up {
        Ok(up) => up,
        Err(e) => {
            result.partial = true;
            result.note = Some(format!("upload stopped after {sent}: {e}"));
            return Ok(result);
        }
    };
    if !up.status().is_success() {
        result.partial = true;
        result.note = Some(format!("upload answered {}", up.status()));
        return Ok(result);
    }
    if up_secs <= 0.0 || sent == 0 {
        result.partial = true;
        result.note = Some("upload finished too fast to measure".into());
        return Ok(result);
    }
    result.upload_bps = rate_bps(sent, Duration::from_secs_f64(up_secs));
    if cancel.load(Ordering::Relaxed) && sent < upload_bytes {
        // The reader stopped early on `Esc`; the number is real but it covers
        // part of the body, and saying so is the difference between a
        // measurement and a guess.
        result.partial = true;
        result.note = Some(format!("cancelled after {sent} bytes up"));
    }
    Ok(result)
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
    fn the_first_read_counts_toward_the_rate() {
        struct SlowFirst {
            sent: bool,
        }
        impl Read for SlowFirst {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.sent {
                    return Ok(0);
                }
                self.sent = true;
                std::thread::sleep(Duration::from_millis(120));
                let n = buf.len().min(1024);
                Ok(n)
            }
        }
        let mut src = SlowFirst { sent: false };
        let open = AtomicBool::new(false);
        let out = read_for(&mut src, Duration::from_secs(60), 1_000_000, &open, |_| {});
        assert_eq!(out.bytes, 1024);
        assert!(
            out.secs >= 0.1,
            "the first read is in the window: {}",
            out.secs
        );
        let bps = rate_bps(out.bytes, Duration::from_secs_f64(out.secs));
        assert!(bps < 1_000_000.0, "one slow chunk is not a gigabit: {bps}");
    }

    #[test]
    fn the_watchdog_budget_covers_the_pings() {
        let cfg = Speedtest::default();
        assert!(worst_case_secs(&cfg) >= cfg.duration_secs + PING_BUDGET_SECS);
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

    /// Loopback, so it costs no data and runs in CI: the mock answers
    /// `/__down` and counts a chunked `/__up`, and both numbers come back.
    #[test]
    fn run_measures_a_download_and_an_upload_against_the_mock() {
        use crate::mock_cpe::{MockCpe, Profile};
        let cpe = MockCpe::start(Profile::H122373);
        let cfg = Speedtest {
            download_url: format!("http://{}/__down?bytes={{bytes}}", cpe.addr()),
            upload_url: format!("http://{}/__up", cpe.addr()),
            duration_secs: 1,
            max_mb: 5,
            upload_mb: 1,
            ..Speedtest::default()
        };
        let out = run(&cfg, &AtomicBool::new(false), |_| {}).expect("a result");
        assert!(out.download_bps > 0.0);
        assert!(out.upload_bps > 0.0, "the metered upload reports itself");
        assert_eq!(out.up_bytes, 1_000_000, "the whole body was read back");
        assert!(!out.partial, "{:?}", out.note);
    }

    #[test]
    #[ignore = "needs the network and spends data"]
    fn a_real_test_reports_both_directions() {
        let r = run(
            &Speedtest {
                max_mb: 5,
                upload_mb: 1,
                duration_secs: 3,
                ..Speedtest::default()
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("network");
        assert!(r.download_bps > 0.0);
        assert!(r.upload_bps > 0.0);
    }
}
