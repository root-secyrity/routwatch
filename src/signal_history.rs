//! The signal log: one row a minute, so a drop that happened while nobody was
//! watching still has a shape when it is read back.
//!
//! Same rules as the speed ledger: a sidecar lock for every write, `0600` on
//! creation, a torn tail repaired before the next line, and a cap that is
//! actually applied. What differs is the clock: the timestamp comes from the
//! file, not from memory, because `record_if_due` is called from separate
//! processes (`status --json` from cron is a new process every time).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Snapshot;

/// v2 adds the earfcn halves and the transmission mode. A v1 row still reads:
/// the three new fields carry `serde(default)`, so an old row arrives as a
/// legacy row with `None` instead of being counted damaged. Newer rows are the
/// only ones refused, because a row this build cannot name is not one it can
/// judge.
pub const VERSION: u32 = 2;
/// Three days at one row a minute. The drift guard needs two complete previous
/// days to build a baseline, and a cap of one day only supplies them when the
/// collector happened to sleep.
pub const CAP: usize = 4320;
/// A signal log is for trends. A row every poll would be thirty rows a minute
/// describing the same cell.
pub const SIGNAL_SAMPLE_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub v: u32,
    pub at_unix: u64,
    pub net_type: Option<String>,
    pub band: Option<String>,
    pub nr_band: Option<String>,
    pub rsrp_dbm: Option<f32>,
    pub rsrq_db: Option<f32>,
    pub sinr_db: Option<f32>,
    pub pci: Option<u32>,
    pub endc: Option<bool>,
    /// The downlink half of `Signal.earfcn`, split at write time: the firmware
    /// sends `DL:100 UL:18100`, and a cell key built from that string would only
    /// compare text. `None` on a legacy row, which is a fact about when it was
    /// written, not a lost value.
    #[serde(default)]
    pub earfcn_dl: Option<u32>,
    #[serde(default)]
    pub earfcn_ul: Option<u32>,
    /// `transmode` as the firmware writes it (`TM[4]`). A row without it is
    /// legacy, the same way.
    #[serde(default)]
    pub tm: Option<String>,
}

impl Row {
    /// A row exists only when the sample carried at least one signal figure.
    /// Every field is named: a struct literal has no `..` without a `Default`
    /// base, and `Row` deliberately has none (a half-filled row is a bug, not a
    /// default).
    pub fn from_snapshot(s: &Snapshot, at_unix: u64) -> Option<Row> {
        let sig = &s.signal;
        if sig.rsrp_dbm.is_none() && sig.sinr_db.is_none() && sig.rsrq_db.is_none() {
            return None;
        }
        let (earfcn_dl, earfcn_ul) = sig
            .earfcn
            .as_deref()
            .map(parse_earfcn)
            .unwrap_or((None, None));
        Some(Row {
            v: VERSION,
            at_unix,
            net_type: Some(s.connection.net_type.clone()),
            band: sig.band.clone(),
            nr_band: sig.nr.as_ref().and_then(|n| n.band.clone()),
            rsrp_dbm: sig.rsrp_dbm,
            rsrq_db: sig.rsrq_db,
            sinr_db: sig.sinr_db,
            pci: sig.pci,
            endc: Some(s.connection.endc),
            earfcn_dl,
            earfcn_ul,
            tm: sig.tm.clone(),
        })
    }
}

/// Split the firmware's composite `DL:100 UL:18100` into the two numbers.
///
/// A half that is present but not a number poisons the pair: `DL:abc UL:7` is
/// not evidence that the uplink is 7, it is evidence that the string is not the
/// shape this build expects.
pub fn parse_earfcn(text: &str) -> (Option<u32>, Option<u32>) {
    let mut dl = None;
    let mut ul = None;
    for token in text.split_whitespace() {
        let (half, value) = match token.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        match half {
            "DL" => match value.parse::<u32>() {
                Ok(n) => dl = Some(n),
                Err(_) => return (None, None),
            },
            "UL" => match value.parse::<u32>() {
                Ok(n) => ul = Some(n),
                Err(_) => return (None, None),
            },
            _ => {}
        }
    }
    (dl, ul)
}

/// What a read of the log gives back. `rows` is in file order, oldest first, so
/// the caller takes the tail it wants to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    pub rows: Vec<Row>,
    pub total: usize,
    pub damaged: usize,
    pub warn: Option<String>,
}

/// `None` when the platform has no data dir: the log is then off rather than
/// written into the current directory.
pub fn path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("signal-history.jsonl"))
}

/// `2026-11-17 04:20` in the local zone, through the same libc path the usage
/// panel uses, so there is no new dependency and no timezone guess. A host
/// without `localtime_r` falls back to UTC.
pub fn local_stamp(unix: u64) -> String {
    #[cfg(unix)]
    unsafe {
        let t = unix as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut tm).is_null() {
            return format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                i64::from(tm.tm_year) + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min
            );
        }
    }
    let (y, m, d) = crate::usage::civil_from_days((unix / 86_400) as i64);
    let s = unix % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        s / 3600,
        (s % 3600) / 60
    )
}

fn fig(v: Option<f32>, unit: &str) -> String {
    match v {
        Some(x) => format!("{x:<5.1}{unit}"),
        None => format!("{:<5}", "-"),
    }
}

/// One text row: local time, band, RSRP, SINR. A missing figure prints as `-`,
/// never as a carried-forward value.
pub fn line(r: &Row) -> String {
    format!(
        "{}  {:<4} {:<5} rsrp {} sinr {}",
        local_stamp(r.at_unix),
        r.band.clone().unwrap_or_else(|| "-".into()),
        r.net_type.clone().unwrap_or_else(|| "-".into()),
        fig(r.rsrp_dbm, "dBm"),
        fig(r.sinr_db, "dB")
    )
}

pub fn table(rows: &[Row]) -> String {
    let mut out = String::new();
    for r in rows {
        out.push_str(&line(r));
        out.push('\n');
    }
    out
}

/// Rows, damaged count, newer count. The version is read from the raw JSON first,
/// so a row written by a newer routwatch is counted as newer instead of damaged.
fn parse(raw: &str) -> (Vec<Row>, usize, usize) {
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
            Some(_) => match serde_json::from_str::<Row>(line) {
                Ok(r) => rows.push(r),
                Err(_) => damaged += 1,
            },
            None => damaged += 1,
        }
    }
    (rows, damaged, newer)
}

pub fn load(path: &Path) -> Loaded {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::default(),
        Err(e) => {
            return Loaded {
                warn: Some(format!("signal history not read: {e}")),
                ..Loaded::default()
            }
        }
    };
    let (rows, damaged, newer) = parse(&raw);
    let total = rows.len();
    let mut notes = Vec::new();
    if damaged > 0 {
        notes.push(format!("{damaged} damaged signal line(s) ignored"));
    }
    if newer > 0 {
        notes.push(format!(
            "{newer} line(s) from a newer routwatch ignored: this build reads v{VERSION}"
        ));
    }
    Loaded {
        rows,
        total,
        damaged,
        warn: if notes.is_empty() {
            None
        } else {
            Some(notes.join("; "))
        },
    }
}

pub(crate) fn ensure_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(crate) fn open_log(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let created = !path.exists();
    let f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    if created {
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        if let Some(dir) = path.parent() {
            std::fs::File::open(dir)?.sync_all()?;
        }
    }
    Ok(f)
}

/// A log whose last byte is not a newline ends in a torn row. Put the newline
/// back first, so this write starts its own line and the damaged row stays one
/// damaged row instead of swallowing the next one.
pub(crate) fn repair_tail(f: &mut std::fs::File) -> std::io::Result<()> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    f.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8; 1];
    f.read_exact(&mut last)?;
    if last[0] != b'\n' {
        f.write_all(b"\n")?;
    }
    Ok(())
}

/// The newest `at_unix` in the file, read from the last 8 KiB only: one write
/// decision must not read the whole multi-day log.
fn last_at_unix(f: &mut std::fs::File) -> std::io::Result<Option<u64>> {
    const TAIL: u64 = 8 * 1024;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().rev() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(at) = j.get("at_unix").and_then(|v| v.as_u64()) {
                return Ok(Some(at));
            }
        }
        // A damaged tail line is not a timestamp: keep looking backwards.
    }
    Ok(None)
}

/// Append `row` when the newest row is at least a minute older, under the same
/// sidecar lock the compaction takes, so two `status --json` runs a second
/// apart cannot both decide the minute is due.
fn record_row(path: &Path, row: &Row, now_unix: u64) -> std::io::Result<bool> {
    // The sidecar lock lives beside the log, so the directory must exist before
    // the lock is taken: a fresh data home has no `routwatch/` yet.
    ensure_dir(path)?;
    let _lock = crate::history::lock_exclusive(path)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another routwatch holds the log",
        )
    })?;
    let mut f = open_log(path)?;
    repair_tail(&mut f)?;
    if let Some(last) = last_at_unix(&mut f)? {
        if now_unix.saturating_sub(last) < SIGNAL_SAMPLE_SECS {
            return Ok(false);
        }
    }
    let mut line = serde_json::to_string(row)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    f.sync_data()?;
    Ok(true)
}

/// Trim the log to its newest `keep` rows, locked like every other write to it.
/// A trim that cannot take the lock is an error the caller may ignore: the new
/// rows are already on disk, the old ones are just not trimmed yet.
pub fn compact_if_needed(path: &Path, keep: usize) -> std::io::Result<usize> {
    let _lock = crate::history::lock_exclusive(path)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another routwatch holds the log",
        )
    })?;
    let raw = std::fs::read_to_string(path)?;
    let (mut rows, _, newer) = parse(&raw);
    if newer > 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "newer rows present",
        ));
    }
    if rows.len() <= keep {
        return Ok(rows.len());
    }
    // File order is chronological, so the tail is the newest rows.
    let drop = rows.len() - keep;
    rows.drain(..drop);
    let mut body = String::new();
    for r in &rows {
        // A trim drops rows, it never relabels the ones it keeps: a legacy row
        // stays v1 with its nulls, so the version keeps saying what was asked
        // for when the row was written.
        body.push_str(
            &serde_json::to_string(r)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        );
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
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(rows.len())
}

/// Append a row when the last one is older than a minute. Never fails the caller:
/// a log that cannot be written is not a reason to lose the snapshot the user
/// asked for.
pub fn record_if_due(s: &Snapshot, now_unix: u64, path: &Path) -> Result<bool, std::io::Error> {
    let Some(row) = Row::from_snapshot(s, now_unix) else {
        return Ok(false);
    };
    let wrote = record_row(path, &row, now_unix)?;
    if wrote {
        // The cap is applied here, not declared and forgotten. The lock is free
        // again by now: `compact_if_needed` takes its own. A failed trim leaves
        // the new rows in place and the old ones untrimmed, which is why it does
        // not turn into a failed sample.
        let _ = compact_if_needed(path, CAP);
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Nr;

    const PATH_NAME: &str = "signal-history.jsonl";

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!(
                "routwatch-signal-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&p).expect("temp dir");
            TempDir(p)
        }

        fn log(&self) -> PathBuf {
            self.0.join(PATH_NAME)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot(rsrp: Option<f32>, sinr: Option<f32>) -> Snapshot {
        let mut s = Snapshot::default();
        s.connection.net_type = "LTE".into();
        s.connection.endc = false;
        s.signal.rsrp_dbm = rsrp;
        s.signal.sinr_db = sinr;
        s.signal.band = Some("3".into());
        s.signal.pci = Some(481);
        s
    }

    fn row_at(at: u64) -> Row {
        Row::from_snapshot(&snapshot(Some(-95.0), Some(3.0)), at).expect("row")
    }

    #[test]
    fn a_missing_data_directory_is_made_before_the_lock_is_taken() {
        // The sidecar lock lives beside the log, so a data home that does not
        // exist yet must be created first, not after the lock fails.
        let dir = TempDir::new("fresh");
        let deep = dir.0.join("routwatch").join("nested").join(PATH_NAME);
        let wrote = record_if_due(&snapshot(Some(-95.0), Some(3.0)), 1_000, &deep).expect("write");
        assert!(wrote);
        assert!(deep.exists());
    }

    #[test]
    fn a_sample_without_any_signal_figure_makes_no_row() {
        assert_eq!(
            Row::from_snapshot(&snapshot(None, None), 1_700_000_000),
            None
        );
    }

    #[test]
    fn one_figure_is_enough_for_a_row_and_every_field_is_copied_whole() {
        let mut s = snapshot(None, Some(-2.5));
        s.signal.rsrp_dbm = None;
        s.signal.rsrq_db = Some(-7.5);
        s.signal.nr = Some(Nr {
            band: Some("78".into()),
            ..Nr::default()
        });
        let r = Row::from_snapshot(&s, 1_700_000_060).expect("row");
        assert_eq!(r.v, VERSION);
        assert_eq!(r.at_unix, 1_700_000_060);
        assert_eq!(r.net_type.as_deref(), Some("LTE"));
        assert_eq!(r.band.as_deref(), Some("3"));
        assert_eq!(r.nr_band.as_deref(), Some("78"));
        assert_eq!(r.rsrq_db, Some(-7.5));
        assert_eq!(r.sinr_db, Some(-2.5));
        assert_eq!(r.pci, Some(481));
        assert_eq!(r.endc, Some(false));
        assert_eq!(r.rsrp_dbm, None);
    }

    #[test]
    fn a_new_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let d = TempDir::new("mode");
        assert!(record_row(&d.log(), &row_at(100), 100).unwrap());
        let mode = std::fs::metadata(d.log()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode {:o}", mode & 0o777);
    }

    #[test]
    fn a_second_row_inside_the_minute_is_not_written() {
        let d = TempDir::new("minute");
        assert!(record_row(&d.log(), &row_at(1000), 1000).unwrap());
        assert!(!record_row(&d.log(), &row_at(1030), 1030).unwrap());
        assert!(!record_row(&d.log(), &row_at(1059), 1059).unwrap());
        assert!(record_row(&d.log(), &row_at(1060), 1060).unwrap());
        let l = load(&d.log());
        assert_eq!(l.total, 2, "{:?}", l);
        assert_eq!(l.rows[1].at_unix, 1060);
    }

    #[test]
    fn an_append_never_rewrites_what_is_already_there() {
        let d = TempDir::new("append");
        assert!(record_row(&d.log(), &row_at(1000), 1000).unwrap());
        let first = std::fs::read_to_string(d.log()).unwrap();
        assert!(record_row(&d.log(), &row_at(2000), 2000).unwrap());
        let both = std::fs::read_to_string(d.log()).unwrap();
        assert!(both.starts_with(&first), "the earlier bytes moved");
        assert_eq!(both.lines().count(), 2);
    }

    #[test]
    fn a_torn_tail_stays_one_damaged_row() {
        let d = TempDir::new("torn");
        assert!(record_row(&d.log(), &row_at(1000), 1000).unwrap());
        // Simulate a kill mid-write: the last line loses its newline and half its
        // body.
        let raw = std::fs::read_to_string(d.log()).unwrap();
        let cut = &raw[..raw.len() - 12];
        std::fs::write(d.log(), cut).unwrap();
        assert!(!cut.ends_with('\n'));
        assert!(record_row(&d.log(), &row_at(2000), 2000).unwrap());
        let l = load(&d.log());
        assert_eq!(l.damaged, 1, "the torn row must stay exactly one: {:?}", l);
        assert_eq!(l.total, 1, "{:?}", l);
        assert_eq!(l.rows[0].at_unix, 2000);
    }

    #[test]
    fn the_cap_keeps_the_newest_rows() {
        let d = TempDir::new("cap");
        for i in 0..5u64 {
            assert!(record_row(&d.log(), &row_at(1000 + i * 60), 1000 + i * 60).unwrap());
        }
        assert_eq!(compact_if_needed(&d.log(), 3).unwrap(), 3);
        let l = load(&d.log());
        assert_eq!(l.total, 3);
        let ats: Vec<u64> = l.rows.iter().map(|r| r.at_unix).collect();
        assert_eq!(ats, vec![1120, 1180, 1240], "the oldest rows go first");
    }

    #[test]
    fn a_record_past_the_cap_trims_itself() {
        let d = TempDir::new("selfcap");
        for i in 0..(CAP as u64 + 2) {
            assert!(record_row(&d.log(), &row_at(1000 + i * 60), 1000 + i * 60).unwrap());
        }
        let mut s = snapshot(Some(-90.0), None);
        s.signal.rsrp_dbm = Some(-90.0);
        let now = 1000 + (CAP as u64 + 5) * 60;
        assert!(record_if_due(&s, now, &d.log()).unwrap());
        let l = load(&d.log());
        assert_eq!(l.total, CAP, "the log holds its cap, not more");
        assert_eq!(l.rows.last().map(|r| r.at_unix), Some(now));
    }

    #[test]
    fn a_row_from_a_newer_version_is_refused_with_one_warning() {
        let d = TempDir::new("newer");
        let mut line = serde_json::to_string(&row_at(1000)).unwrap();
        line.push('\n');
        let newer = line.replace("\"v\":2", "\"v\":3");
        std::fs::write(d.log(), format!("{line}{newer}")).unwrap();
        let l = load(&d.log());
        assert_eq!(l.total, 1, "only the row this build understands");
        let warn = l.warn.expect("a warning");
        assert!(warn.contains("newer routwatch"), "{warn}");
    }

    #[test]
    fn a_damaged_line_does_not_lose_the_file() {
        let d = TempDir::new("damaged");
        let mut line = serde_json::to_string(&row_at(1000)).unwrap();
        line.push('\n');
        std::fs::write(d.log(), format!("{line}not json\n{line}")).unwrap();
        let l = load(&d.log());
        assert_eq!(l.damaged, 1);
        assert_eq!(l.total, 2);
        assert!(l.warn.unwrap().contains("damaged"));
    }

    #[test]
    fn one_process_holding_the_lock_keeps_the_other_out() {
        let d = TempDir::new("lock");
        let _held = crate::history::lock_exclusive(&d.log())
            .unwrap()
            .expect("lock");
        let err = record_row(&d.log(), &row_at(1000), 1000).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(!d.log().exists(), "nothing was written without the lock");
    }

    #[test]
    fn a_snapshot_without_signal_writes_no_file() {
        let d = TempDir::new("nosignal");
        let wrote = record_if_due(&snapshot(None, None), 1000, &d.log()).unwrap();
        assert!(!wrote);
        assert!(!d.log().exists());
    }

    const LEGACY_V1: &str = r#"{"v":1,"at_unix":1000,"net_type":"LTE","band":"3","nr_band":null,"rsrp_dbm":-95.0,"rsrq_db":null,"sinr_db":3.0,"pci":481,"endc":false}"#;

    fn legacy_at(at: u64) -> String {
        LEGACY_V1.replace("\"at_unix\":1000", &format!("\"at_unix\":{at}"))
    }

    #[test]
    fn the_earfcn_string_is_split_into_its_two_halves() {
        assert_eq!(parse_earfcn("DL:100 UL:18100"), (Some(100), Some(18100)));
        assert_eq!(parse_earfcn("DL:100"), (Some(100), None));
        assert_eq!(parse_earfcn("UL:18100"), (None, Some(18100)));
        assert_eq!(parse_earfcn(""), (None, None));
        assert_eq!(
            parse_earfcn("DL:abc UL:7"),
            (None, None),
            "one bad half is not evidence for the other"
        );
        assert_eq!(
            parse_earfcn("DL:100 UL:18100 junk"),
            (Some(100), Some(18100)),
            "a token this build does not know is skipped, not fatal"
        );
    }

    #[test]
    fn a_new_row_carries_the_cell_key_and_the_transmission_mode() {
        let mut s = snapshot(Some(-95.0), Some(3.0));
        s.signal.earfcn = Some("DL:1650 UL:19650".into());
        s.signal.tm = Some("TM[4]".into());
        let r = Row::from_snapshot(&s, 1000).expect("row");
        assert_eq!(r.v, VERSION);
        assert_eq!(r.earfcn_dl, Some(1650));
        assert_eq!(r.earfcn_ul, Some(19650));
        assert_eq!(r.tm.as_deref(), Some("TM[4]"));
        let round: Row = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(round, r, "a v2 row survives its own write");
    }

    #[test]
    fn a_row_with_no_earfcn_writes_nulls_not_a_guess() {
        let r = row_at(1000);
        assert_eq!(r.earfcn_dl, None);
        assert_eq!(r.earfcn_ul, None);
        assert_eq!(r.tm, None);
    }

    #[test]
    fn a_v1_row_reads_as_legacy_without_counting_as_damaged() {
        let d = TempDir::new("legacy");
        std::fs::write(d.log(), format!("{LEGACY_V1}\n")).unwrap();
        let l = load(&d.log());
        assert_eq!(l.total, 1, "{:?}", l);
        assert_eq!(l.damaged, 0, "an old row is old, not broken: {:?}", l);
        assert_eq!(l.warn, None, "{:?}", l);
        assert_eq!(
            l.rows[0].v, 1,
            "the row keeps the version it was written with"
        );
        assert_eq!(l.rows[0].earfcn_dl, None);
        assert_eq!(l.rows[0].tm, None);
    }

    #[test]
    fn a_read_never_rewrites_the_file() {
        let d = TempDir::new("readonly");
        std::fs::write(d.log(), format!("{LEGACY_V1}\n")).unwrap();
        let before = std::fs::read(d.log()).unwrap();
        let _ = load(&d.log());
        let after = std::fs::read(d.log()).unwrap();
        assert_eq!(before, after, "a read is a read");
    }

    #[test]
    fn a_rewrite_keeps_each_row_s_own_version_and_the_legacy_nulls() {
        let d = TempDir::new("rewrite");
        let mut oldest = serde_json::to_string(&row_at(1000)).unwrap();
        oldest.push('\n');
        let mut newest = serde_json::to_string(&row_at(1120)).unwrap();
        newest.push('\n');
        // A v2 row, then a legacy row, then a v2 row. Trimmed to the newest two,
        // the legacy row survives the rewrite and so do its nulls and its version.
        std::fs::write(d.log(), format!("{oldest}{}\n{newest}", legacy_at(1060))).unwrap();
        assert_eq!(compact_if_needed(&d.log(), 2).unwrap(), 2);
        let l = load(&d.log());
        assert_eq!(l.total, 2, "{:?}", l);
        assert_eq!(l.rows[0].at_unix, 1060);
        assert_eq!(l.rows[0].v, 1, "a trim does not relabel what is asked for");
        assert_eq!(l.rows[0].earfcn_dl, None, "absence is not lost by a trim");
        assert_eq!(l.rows[1].at_unix, 1120);
        assert_eq!(l.rows[1].v, VERSION);
    }
}
