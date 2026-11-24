//! Saved speed runs, one JSON object per line, 0600 under the data dir.
//!
//! The write path only appends: `append` opens with O_APPEND and issues one
//! `write_all`, so two writers cannot interleave halves of a line on a local
//! filesystem (network filesystems make no such promise). `compact_if_needed` is
//! the only rewrite, and it holds the lock before it reads.

use crate::config::Speedtest;
use crate::speedtest::SpeedResult;
use crate::worker::PingOutcome;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;
/// Runs the Network panel keeps for display.
pub const SHOWN: usize = 10;
/// Runs the file keeps.
pub const KEEP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Finished the full budget.
    Ok,
    /// Moved bytes, then stopped on an error: real numbers, short window.
    Partial,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Partial => "partial",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Tui,
    Cli,
}

/// The radio as it stood when the run finished. Every field is optional because
/// the firmware may omit any of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Radio {
    pub net_type: Option<String>,
    pub band: Option<String>,
    pub band_at_start: Option<String>,
    pub rsrp_dbm: Option<f32>,
    pub rsrq_db: Option<f32>,
    pub sinr_db: Option<f32>,
}

/// The two ICMP sets a run takes. The CLI pings nothing, so it passes `default`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pings {
    pub host: Option<String>,
    pub idle: Option<PingOutcome>,
    pub loaded: Option<PingOutcome>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub v: u32,
    pub started_unix: u64,
    pub ended_unix: u64,
    pub elapsed_ms: u64,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Bits per second, like `SpeedResult`.
    pub download_bps: f64,
    pub upload_bps: f64,
    pub ttfb_ms: f64,
    pub down_secs: f64,
    pub up_secs: f64,
    pub bytes_down: u64,
    pub bytes_up: u64,
    #[serde(default)]
    pub short_window: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_idle_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_loaded_ms: Option<f64>,
    /// `loaded - idle`, ICMP, same host. Not jitter and not MOS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_bloat_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_loss_idle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_loss_loaded: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band_at_start: Option<String>,
    #[serde(default)]
    pub band_changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rsrp_dbm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rsrq_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sinr_db: Option<f32>,
    pub source: Source,
    pub app_version: String,
    pub cfg_secs: u64,
    pub cfg_max_mb: u64,
    pub cfg_upload_mb: u64,
    #[serde(default)]
    pub cfg_download_only: bool,
    pub download_url: String,
    pub upload_url: String,
}

impl Entry {
    /// An empty row at the current format version, for callers that fill fields
    /// one by one (tests, fixtures).
    pub fn blank() -> Entry {
        Entry {
            v: VERSION,
            started_unix: 0,
            ended_unix: 0,
            elapsed_ms: 0,
            status: Status::Ok,
            note: None,
            download_bps: 0.0,
            upload_bps: 0.0,
            ttfb_ms: 0.0,
            down_secs: 0.0,
            up_secs: 0.0,
            bytes_down: 0,
            bytes_up: 0,
            short_window: false,
            ping_idle_ms: None,
            ping_loaded_ms: None,
            ping_bloat_ms: None,
            ping_host: None,
            ping_loss_idle: None,
            ping_loss_loaded: None,
            net_type: None,
            band: None,
            band_at_start: None,
            band_changed: false,
            rsrp_dbm: None,
            rsrq_db: None,
            sinr_db: None,
            source: Source::Tui,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            cfg_secs: 0,
            cfg_max_mb: 0,
            cfg_upload_mb: 0,
            cfg_download_only: false,
            download_url: String::new(),
            upload_url: String::new(),
        }
    }

    /// One row from a finished run. `now_unix` fills in for a result that carries
    /// no stamp of its own.
    pub fn from_result(
        result: &SpeedResult,
        cfg: &Speedtest,
        source: Source,
        radio: Option<&Radio>,
        pings: Pings,
        elapsed_ms: u64,
        now_unix: u64,
    ) -> Entry {
        let text = |f: fn(&Radio) -> Option<String>| {
            radio
                .and_then(f)
                .filter(|s| !s.is_empty() && s.as_str() != "-")
        };
        let band_at_start = text(|r| r.band_at_start.clone());
        let band = text(|r| r.band.clone());
        let num = |f: fn(&Radio) -> Option<f32>| radio.and_then(f);
        let idle = pings.idle.as_ref().and_then(|p| p.avg_ms);
        let loaded = pings.loaded.as_ref().and_then(|p| p.avg_ms);
        let ended_unix = if result.at_unix > 0 {
            result.at_unix
        } else {
            now_unix
        };
        Entry {
            v: VERSION,
            started_unix: ended_unix.saturating_sub(elapsed_ms / 1000),
            ended_unix,
            elapsed_ms,
            status: if result.partial {
                Status::Partial
            } else {
                Status::Ok
            },
            note: result.note.clone(),
            download_bps: result.download_bps,
            upload_bps: result.upload_bps,
            ttfb_ms: result.ttfb_ms,
            down_secs: result.down_secs,
            up_secs: result.up_secs,
            bytes_down: result.down_bytes,
            bytes_up: result.up_bytes,
            short_window: result.short_window,
            ping_idle_ms: idle,
            ping_loaded_ms: loaded,
            ping_bloat_ms: match (idle, loaded) {
                (Some(i), Some(l)) => Some(l - i),
                _ => None,
            },
            ping_host: pings
                .host
                .clone()
                .or_else(|| pings.idle.as_ref().map(|p| p.host.clone())),
            ping_loss_idle: pings.idle.as_ref().map(|p| p.loss_pct as f64),
            ping_loss_loaded: pings.loaded.as_ref().map(|p| p.loss_pct as f64),
            net_type: radio.and_then(|r| r.net_type.clone()),
            band_changed: matches!((&band_at_start, &band), (Some(a), Some(b)) if a != b),
            band,
            band_at_start,
            rsrp_dbm: num(|r| r.rsrp_dbm),
            rsrq_db: num(|r| r.rsrq_db),
            sinr_db: num(|r| r.sinr_db),
            source,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            cfg_secs: cfg.duration_secs,
            cfg_max_mb: cfg.max_mb,
            cfg_upload_mb: cfg.upload_mb,
            cfg_download_only: cfg.download_only,
            download_url: cfg.download_url.clone(),
            upload_url: cfg.upload_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    /// Newest first, truncated to `KEEP`.
    pub rows: Vec<Entry>,
    /// Rows the file held, before the cap was applied.
    pub total: usize,
    pub warn: Option<String>,
    pub writable: bool,
}

/// `None` when the platform has no data dir: history is then disabled rather than
/// written into the current directory.
pub fn path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("speed-history.jsonl"))
}

/// Rows, damaged count, newer count. The version is read from the raw JSON before
/// the full parse, so a row written by a newer routwatch (which may carry fields
/// this build does not know) is counted as newer instead of damaged.
fn parse(raw: &str) -> (Vec<Entry>, usize, usize) {
    let mut rows = Vec::new();
    let mut damaged = 0;
    let mut newer = 0;
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|j| j.get("v").and_then(|v| v.as_u64()));
        match v {
            Some(n) if n > VERSION as u64 => newer += 1,
            Some(_) => match serde_json::from_str::<Entry>(line) {
                Ok(e) => rows.push(e),
                Err(_) => damaged += 1,
            },
            None => damaged += 1,
        }
    }
    (rows, damaged, newer)
}

/// Newest first. Exact ties keep file order, because `sort_by` is stable and
/// appends are chronological.
fn newest_first(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    b.ended_unix
        .cmp(&a.ended_unix)
        .then(b.started_unix.cmp(&a.started_unix))
}

fn combine(parts: Vec<String>) -> Option<String> {
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn finish(mut rows: Vec<Entry>, damaged: usize, newer: usize) -> Loaded {
    let total = rows.len();
    rows.sort_by(newest_first);
    let mut notes = Vec::new();
    if damaged > 0 {
        notes.push(format!("{damaged} damaged speed history line(s) ignored"));
    }
    if newer > 0 {
        notes.push(format!("{newer} line(s) from a newer routwatch ignored"));
    }
    if rows.len() > KEEP {
        // Reading never writes: the caller decides whether to trim.
        notes.push("speed history is over its cap; it will be trimmed".into());
        rows.truncate(KEEP);
    }
    Loaded {
        rows,
        total,
        warn: combine(notes),
        writable: true,
    }
}

pub fn load(path: &Path) -> Loaded {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded {
                rows: Vec::new(),
                total: 0,
                warn: None,
                writable: true,
            }
        }
        Err(e) => {
            return Loaded {
                rows: Vec::new(),
                total: 0,
                warn: Some(format!("speed history not read: {e}")),
                writable: false,
            }
        }
    };
    let (rows, damaged, newer) = parse(&raw);
    finish(rows, damaged, newer)
}

/// The same read without a file: `preview --history-fixture` and the tests use it,
/// so a frame can show saved runs without a data dir.
pub fn load_from_str(raw: &str) -> Loaded {
    let (rows, damaged, newer) = parse(raw);
    finish(rows, damaged, newer)
}

fn to_line(e: &Entry) -> io::Result<String> {
    serde_json::to_string(e).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn ensure_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn fsync_dir(path: &Path) -> io::Result<()> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    std::fs::File::open(dir)?.sync_all()
}

/// Append one row: O_APPEND, one `write_all`, 0600 when the file is created.
/// An existing file keeps the mode the user chose.
pub fn append(path: &Path, e: &Entry) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut line = to_line(e)?;
    line.push('\n');
    ensure_dir(path)?;
    let created = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    if created {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        // A new directory entry is not durable until the directory is synced.
        fsync_dir(path)?;
    }
    f.write_all(line.as_bytes())?;
    f.sync_data()
}

pub(crate) fn lock_exclusive(path: &Path) -> io::Result<Option<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let lock = path.with_extension("jsonl.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock)?;
    // SAFETY: the fd stays open for the life of `f`; LOCK_NB reports contention
    // instead of blocking, so the TUI never waits on a CLI run. A lost attempt can
    // be a holder that lasts milliseconds rather than a writer, so it is retried
    // twice: a trim (131 ms) still ends as None, and the wait a caller sees stays
    // under 10 ms.
    for attempt in 0..3 {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(f));
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Ok(None)
}

/// Trim the file to its newest `keep` rows. Lock first, then read, then write, so
/// an append that lands mid-trim is either inside the lock or after the rename.
pub fn compact_if_needed(path: &Path, keep: usize) -> io::Result<usize> {
    let _lock = lock_exclusive(path)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "another routwatch holds it"))?;
    let raw = std::fs::read_to_string(path)?;
    let (mut rows, _, newer) = parse(&raw);
    if newer > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "newer rows present",
        ));
    }
    if rows.len() <= keep {
        return Ok(rows.len());
    }
    rows.sort_by(newest_first);
    rows.truncate(keep);
    let mut body = String::new();
    for r in &rows {
        body.push_str(&to_line(r)?);
        body.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    fsync_dir(path)?;
    Ok(rows.len())
}

/// Median download of the rows on screen: partial runs included, so the caller
/// labels it as the median of what is shown.
pub fn median_down(rows: &[Entry]) -> f64 {
    let mut v: Vec<f64> = rows.iter().map(|r| r.download_bps).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch ledger whose directory removes itself, so a test run leaves
    /// nothing behind in `$TMPDIR`. It derefs to the file, not the directory.
    struct Scratch {
        dir: PathBuf,
        file: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rw-hist-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("speed-history.jsonl");
            Scratch { dir, file }
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = std::path::Path;

        fn deref(&self) -> &std::path::Path {
            &self.file
        }
    }

    #[test]
    fn a_lock_this_process_holds_is_contention_with_a_bounded_wait() {
        let d = Scratch::new("locktries");
        let held = lock_exclusive(&d).unwrap().expect("the first lock");
        let started = std::time::Instant::now();
        assert!(
            lock_exclusive(&d).unwrap().is_none(),
            "a lock already held is contention, never a free path"
        );
        let waited = started.elapsed();
        assert!(
            waited >= std::time::Duration::from_millis(9),
            "two retries, 5 ms apart, take at least 10 ms: {waited:?}"
        );
        assert!(
            waited < std::time::Duration::from_secs(1),
            "the wait is bounded, it never blocks on another routwatch: {waited:?}"
        );
        drop(held);
        assert!(
            lock_exclusive(&d).unwrap().is_some(),
            "the lock is free once its holder drops it"
        );
    }

    impl AsRef<std::path::Path> for Scratch {
        fn as_ref(&self) -> &std::path::Path {
            &self.file
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            // A test may have made the directory read-only to prove a failure
            // path; an owner can always chmod its own directory back.
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn scratch(name: &str) -> Scratch {
        Scratch::new(name)
    }

    fn entry(ended: u64, down: f64) -> Entry {
        let mut e = Entry::blank();
        e.ended_unix = ended;
        e.started_unix = ended.saturating_sub(18);
        e.download_bps = down;
        e
    }

    fn cfg() -> Speedtest {
        Speedtest {
            duration_secs: 8,
            max_mb: 60,
            upload_mb: 10,
            download_only: false,
            ping_host: "1.1.1.1".into(),
            ..Default::default()
        }
    }

    fn result() -> SpeedResult {
        SpeedResult {
            download_bps: 200_000_000.0,
            upload_bps: 20_000_000.0,
            ttfb_ms: 12.0,
            down_bytes: 60_000_000,
            up_bytes: 10_000_000,
            down_secs: 2.4,
            up_secs: 4.0,
            download_url: "http://x/down".into(),
            upload_url: "http://x/up".into(),
            partial: false,
            short_window: false,
            note: None,
            at_unix: 1_763_300_500,
        }
    }

    fn ping(avg: f64, loss: u32) -> PingOutcome {
        PingOutcome {
            host: "1.1.1.1".into(),
            sent: 4,
            received: 4,
            loss_pct: loss,
            avg_ms: Some(avg),
            ok: true,
            detail: String::new(),
        }
    }

    #[test]
    fn a_line_round_trips_through_json() {
        let p = scratch("rt");
        let mut e = entry(1_763_300_000, 42_000_000.0);
        e.status = Status::Partial;
        append(&p, &e).unwrap();
        let l = load(&p);
        assert_eq!(l.rows, vec![e]);
        assert_eq!(l.total, 1);
        assert!(l.warn.is_none());
        assert!(l.writable);
    }

    #[test]
    fn a_torn_last_line_is_ignored_and_counted() {
        let p = scratch("torn");
        append(&p, &entry(1_763_300_000, 1.0)).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        std::io::Write::write_all(&mut f, b"{\"v\":1,").unwrap();
        let l = load(&p);
        assert_eq!(l.rows.len(), 1);
        assert!(l.warn.unwrap().contains("damaged"));
    }

    #[test]
    fn a_newer_row_is_ignored_but_the_file_stays_usable() {
        let p = scratch("newer");
        append(&p, &entry(1_763_300_000, 1.0)).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        std::io::Write::write_all(
            &mut f,
            b"{\"v\":99,\"started_unix\":1,\"ended_unix\":2,\"elapsed_ms\":1,\"status\":\"ok\",\
              \"download_bps\":1.0,\"upload_bps\":0.0,\"ttfb_ms\":0.0,\"down_secs\":0.0,\
              \"up_secs\":0.0,\"bytes_down\":0,\"bytes_up\":0,\"short_window\":false,\
              \"source\":\"tui\",\"app_version\":\"9\",\"cfg_secs\":1,\"cfg_max_mb\":1,\
              \"cfg_upload_mb\":1,\"cfg_download_only\":false,\"download_url\":\"u\",\
              \"upload_url\":\"v\",\"future_field\":{\"x\":1}}\n",
        )
        .unwrap();
        let l = load(&p);
        assert_eq!(
            l.rows.len(),
            1,
            "the newer row is dropped, the older one is kept"
        );
        assert!(l.writable, "a newer row must not freeze the file");
        assert!(l.warn.unwrap().contains("newer"));
    }

    #[test]
    fn load_never_writes() {
        let p = scratch("readonly");
        for i in 0..KEEP + 3 {
            append(&p, &entry(1_763_300_000 + i as u64, 1.0)).unwrap();
        }
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        let l = load(&p);
        let after = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(before, after);
        assert_eq!(l.rows.len(), KEEP);
        assert_eq!(l.total, KEEP + 3);
    }

    #[test]
    fn compaction_keeps_the_newest_and_the_file_stays_0600() {
        let p = scratch("compact");
        for i in 0..KEEP + 5 {
            append(&p, &entry(1_763_300_000 + i as u64, 1.0)).unwrap();
        }
        let kept = compact_if_needed(&p, KEEP).unwrap();
        assert_eq!(kept, KEEP);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), KEEP);
        assert!(text.contains(&format!(
            "\"ended_unix\":{}",
            1_763_300_000 + KEEP as u64 + 4
        )));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn an_exact_tie_keeps_file_order() {
        let p = scratch("tie");
        append(&p, &entry(1_763_300_000, 1.0)).unwrap();
        append(&p, &entry(1_763_300_000, 2.0)).unwrap();
        let l = load(&p);
        assert_eq!(
            l.rows[0].download_bps, 1.0,
            "stable sort: the first line stays first"
        );
    }

    #[test]
    fn the_median_averages_the_two_middle_values() {
        let rows: Vec<Entry> = [10.0, 20.0, 30.0, 40.0]
            .iter()
            .enumerate()
            .map(|(i, d)| entry(1_763_300_000 + i as u64, *d))
            .collect();
        assert_eq!(median_down(&rows), 25.0);
        assert_eq!(median_down(&rows[..3]), 20.0);
        assert_eq!(median_down(&[]), 0.0);
    }

    #[test]
    fn from_result_carries_the_radio_and_both_pings() {
        let radio = Radio {
            net_type: Some("LTE".into()),
            band: Some("3".into()),
            band_at_start: Some("3".into()),
            rsrp_dbm: Some(-95.0),
            rsrq_db: Some(-9.0),
            sinr_db: Some(12.5),
        };
        let pings = Pings {
            host: Some("1.1.1.1".into()),
            idle: Some(ping(18.0, 0)),
            loaded: Some(ping(64.0, 25)),
        };
        let e = Entry::from_result(
            &result(),
            &cfg(),
            Source::Tui,
            Some(&radio),
            pings,
            9_000,
            0,
        );
        assert_eq!(e.ended_unix, 1_763_300_500, "the run's own stamp wins");
        assert_eq!(e.started_unix, 1_763_300_491);
        assert_eq!(e.ping_bloat_ms, Some(46.0));
        assert_eq!(e.ping_loss_loaded, Some(25.0));
        assert!(!e.band_changed);
        assert_eq!(e.status, Status::Ok);
        assert_eq!(e.app_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn from_result_uses_now_when_the_run_has_no_stamp() {
        let mut r = result();
        r.at_unix = 0;
        let e = Entry::from_result(
            &r,
            &cfg(),
            Source::Cli,
            None,
            Pings::default(),
            2_000,
            1_763_400_000,
        );
        assert_eq!(e.ended_unix, 1_763_400_000);
        assert!(e.ping_idle_ms.is_none());
        assert!(e.ping_host.is_none());
    }

    #[test]
    fn band_change_is_flagged_only_when_both_ends_are_known() {
        let mut r = Radio {
            band_at_start: Some("3".into()),
            band: Some("20".into()),
            ..Default::default()
        };
        assert!(
            Entry::from_result(
                &result(),
                &cfg(),
                Source::Tui,
                Some(&r),
                Pings::default(),
                1,
                0
            )
            .band_changed
        );
        r.band = None;
        assert!(
            !Entry::from_result(
                &result(),
                &cfg(),
                Source::Tui,
                Some(&r),
                Pings::default(),
                1,
                0
            )
            .band_changed
        );
        let r2 = Radio {
            band_at_start: None,
            band: Some("20".into()),
            ..Default::default()
        };
        assert!(
            !Entry::from_result(
                &result(),
                &cfg(),
                Source::Tui,
                Some(&r2),
                Pings::default(),
                1,
                0
            )
            .band_changed
        );
    }

    #[test]
    fn a_partial_result_is_marked_and_the_source_is_lowercase() {
        let mut r = result();
        r.partial = true;
        r.note = Some("upload stopped early".into());
        let e = Entry::from_result(&r, &cfg(), Source::Cli, None, Pings::default(), 1, 0);
        assert_eq!(e.status, Status::Partial);
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"status\":\"partial\""));
        assert!(json.contains("\"source\":\"cli\""));
        assert!(
            !json.contains("ping_idle_ms"),
            "absent values are not written"
        );
    }

    #[test]
    fn the_path_is_optional_not_a_dot() {
        if let Some(p) = path() {
            assert!(p.ends_with("routwatch/speed-history.jsonl"));
        }
    }
}
