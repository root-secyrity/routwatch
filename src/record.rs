//! Session recording: append snapshots to a JSONL file and replay them later.
//!
//! One file, one frame per line: `{"at": 1761000000, "snapshot": {...}}`.
//! The format is deliberately boring - `jq`, `grep` and a text editor all read it.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::model::Snapshot;

/// One recorded sample. `at` is unix seconds - the only place a clock is read,
/// so pruning and replay stay testable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordFrame {
    pub at: u64,
    pub snapshot: Snapshot,
}

impl RecordFrame {
    pub fn new(at: u64, snapshot: Snapshot) -> Self {
        Self { at, snapshot }
    }

    /// Capture "now" - the single impure entry point.
    pub fn capture(snapshot: Snapshot) -> Self {
        Self::new(now_unix(), snapshot)
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `30d`, `12h`, `90m`, `45s` - also accepts a bare number of seconds.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("empty duration".into());
    }
    let (digits, unit): (String, &str) = match t.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (t[..i].to_string(), &t[i..]),
        None => (t.to_string(), "s"),
    };
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("not a duration: {text:?}"))?;
    let scale = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" => 3600,
        "d" | "day" | "days" => 86_400,
        other => return Err(format!("unknown duration unit {other:?}")),
    };
    value
        .checked_mul(scale)
        .ok_or_else(|| format!("duration too large: {text:?}"))
}

/// Default recording location: `$XDG_DATA_HOME/routwatch/recordings/`.
pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("routwatch/recordings")
}

/// Append one frame, creating the parent directory (0700) on first use.
pub fn append(path: &Path, frame: &RecordFrame) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
            }
        }
    }
    // A recording can hold a client list; keep it to the owner.
    let existed = path.exists();
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if !existed {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
    }
    let line = serde_json::to_string(frame).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")
}

/// Read every frame, skipping lines that fail to parse (a truncated tail from a
/// killed recorder must not make the whole file unreadable).
pub fn load(path: &Path) -> std::io::Result<Vec<RecordFrame>> {
    let file = fs::File::open(path)?;
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(frame) = serde_json::from_str::<RecordFrame>(&line) {
            out.push(frame);
        }
    }
    Ok(out)
}

/// Drop frames older than `keep` seconds relative to `now`; returns how many
/// were kept. `now` is injected so the test does not depend on the wall clock.
pub fn prune(path: &Path, keep: u64, now: u64) -> std::io::Result<usize> {
    let frames = load(path)?;
    let cutoff = now.saturating_sub(keep);
    let kept: Vec<RecordFrame> = frames.into_iter().filter(|f| f.at >= cutoff).collect();
    let mut file = fs::File::create(path)?;
    for frame in &kept {
        let line = serde_json::to_string(frame).map_err(std::io::Error::other)?;
        writeln!(file, "{line}")?;
    }
    Ok(kept.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("routwatch-test-{name}-{}", now_unix()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn durations_parse_in_every_documented_form() {
        assert_eq!(parse_duration("45s").unwrap(), 45);
        assert_eq!(parse_duration("90m").unwrap(), 5400);
        assert_eq!(parse_duration("12h").unwrap(), 43_200);
        assert_eq!(parse_duration("30d").unwrap(), 2_592_000);
        assert_eq!(parse_duration("7").unwrap(), 7, "bare number is seconds");
        assert_eq!(
            parse_duration(" 2 H ").unwrap(),
            7200,
            "case and spaces are fine"
        );
        for bad in ["", "abc", "5y", "1.5h"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn frames_round_trip_through_the_file() {
        let dir = tmpdir("roundtrip");
        let path = dir.join("session.rwr");
        for i in 0..3 {
            append(&path, &RecordFrame::new(1000 + i, Snapshot::default())).unwrap();
        }
        let frames = load(&path).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].at, 1000);
        assert_eq!(frames[2].at, 1002);

        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 3, "one JSON object per line");
        let first: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert!(first.get("at").is_some() && first.get("snapshot").is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "a recording can name clients: owner only");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_appends_and_prunes_by_age() {
        let dir = tmpdir("prune");
        let path = dir.join("session.rwr");
        append(&path, &RecordFrame::new(100, Snapshot::default())).unwrap();
        append(&path, &RecordFrame::new(500, Snapshot::default())).unwrap();
        append(&path, &RecordFrame::new(900, Snapshot::default())).unwrap();

        // keep the last 500s of a 900s timeline
        let kept = prune(&path, 500, 900).unwrap();
        assert_eq!(kept, 2);
        let frames = load(&path).unwrap();
        assert_eq!(
            frames.iter().map(|f| f.at).collect::<Vec<_>>(),
            vec![500, 900]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_tail_does_not_break_replay() {
        let dir = tmpdir("truncated");
        let path = dir.join("session.rwr");
        append(&path, &RecordFrame::new(1, Snapshot::default())).unwrap();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"at\":2,\"snapshot\":{{\"sign").unwrap();
        let frames = load(&path).unwrap();
        assert_eq!(frames.len(), 1, "the partial line is skipped, not fatal");
        let _ = fs::remove_dir_all(&dir);
    }
}
