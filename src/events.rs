//! The observation log: an append-only JSONL file of what changed on the link.
//!
//! A row is a state change, never an attempt. A write that was started and failed
//! before reaching the radio is not here; a verdict is. A read that answered is
//! not here; the transition into a poor signal is.
//!
//! The `tier` field names the group whose state the row describes, and `at_unix`
//! is the moment that group's state was observed: for a read, the moment the read
//! came back; for a write, the verdict, because that is when the radio changed.
//! A row is never stamped with the wall clock of the moment it was written.
//!
//! The file contract is the one the two history logs already use: 0600 on
//! creation, one sidecar lock, a torn last line repaired before the next row,
//! and compaction that drops whole old rows and never rewrites a kept one.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Bumped only when a reader of an older version would misread a new row.
pub const VERSION: u32 = 1;

/// Rows kept by compaction. The log is a record of transitions, so it grows with
/// how often the link changes, not with a cadence.
pub const KEEP: usize = 2000;

/// Compact once the file passes this size, checked after an append.
const MAX_BYTES: u64 = 512 * 1024;

/// The five families of state change. `ReadFail` is a state change too: a tier
/// that stopped answering is why a panel froze.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Write,
    Renew,
    Wan,
    ReadFail,
    Signal,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Write => "write",
            EventKind::Renew => "renew",
            EventKind::Wan => "wan",
            EventKind::ReadFail => "read",
            EventKind::Signal => "signal",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "write" => Some(EventKind::Write),
            "renew" => Some(EventKind::Renew),
            "wan" => Some(EventKind::Wan),
            "read" => Some(EventKind::ReadFail),
            "signal" => Some(EventKind::Signal),
            _ => None,
        }
    }
}

/// One row. `tier` is a label rather than a number so a reader of the file does
/// not need this enum to know what it is looking at.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObsEvent {
    pub v: u32,
    pub at_unix: u64,
    pub tier: String,
    pub kind: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ObsEvent {
    pub fn new(
        kind: EventKind,
        tier: &str,
        at_unix: u64,
        outcome: &str,
        detail: Option<String>,
    ) -> Self {
        ObsEvent {
            v: VERSION,
            at_unix,
            tier: tier.to_string(),
            kind: kind.as_str().to_string(),
            outcome: outcome.to_string(),
            detail,
        }
    }

    pub fn event_kind(&self) -> Option<EventKind> {
        EventKind::parse(&self.kind)
    }
}

/// `None` when the platform has no data dir: the log is off rather than written
/// into the current directory.
pub fn path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("events.jsonl"))
}

/// What a read of the log gives back. `rows` is in file order, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    pub rows: Vec<ObsEvent>,
    pub damaged: usize,
}

/// Append one row under the sidecar lock, after repairing a torn tail.
///
/// A holder of the lock answers `WouldBlock` instead of waiting: the caller is a
/// poller, and an event that waits on a lock is worse than an event that is late.
pub fn append(path: &Path, e: &ObsEvent) -> std::io::Result<()> {
    crate::signal_history::ensure_dir(path)?;
    {
        let _lock = crate::history::lock_exclusive(path)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "another routwatch holds the event log",
            )
        })?;
        let mut f = crate::signal_history::open_log(path)?;
        crate::signal_history::repair_tail(&mut f)?;
        let mut line = serde_json::to_string(e)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        line.push('\n');
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
    }
    maybe_compact(path);
    Ok(())
}

/// Trim only when the file has grown past `MAX_BYTES`, and never as part of the
/// append lock: a failed trim leaves the rows on disk, which is the safe side.
fn maybe_compact(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= MAX_BYTES {
        return;
    }
    let _ = compact_if_needed(path, KEEP);
}

/// Read every row, counting the ones that will not parse. A damaged row is
/// reported, not repaired: the file is append-only, and a rewrite is compaction's
/// job, under the lock.
pub fn load(path: &Path) -> Loaded {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Loaded::default();
    };
    let mut out = Loaded::default();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ObsEvent>(line) {
            Ok(e) if e.v <= VERSION => out.rows.push(e),
            // A newer row is not damaged, but this build cannot read it, so it is
            // counted rather than silently rendered as an unknown event.
            _ => out.damaged += 1,
        }
    }
    out
}

/// Keep the newest `keep` rows. File order is chronological, so the tail is kept
/// and whole rows are dropped from the head.
pub fn compact_if_needed(path: &Path, keep: usize) -> std::io::Result<usize> {
    let _lock = crate::history::lock_exclusive(path)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another routwatch holds the event log",
        )
    })?;
    let raw = std::fs::read_to_string(path)?;
    let (mut rows, _) = {
        let loaded = load(path);
        (loaded.rows, loaded.damaged)
    };
    if rows.len() <= keep {
        return Ok(rows.len());
    }
    let drop = rows.len() - keep;
    rows.drain(..drop);
    let mut body = String::new();
    for r in &rows {
        body.push_str(
            &serde_json::to_string(r)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        );
        body.push('\n');
    }
    let _ = raw;
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

/// The two states the radio tier can change from one read to the next. Rows are
/// produced on the crossing only, so a link that stays poor overnight writes one
/// row, not one per read.
#[derive(Debug, Default)]
pub struct Watch {
    poor: Option<bool>,
    wan_up: Option<bool>,
}

impl Watch {
    /// A full collect sees every document, so it sets the baseline without
    /// writing: a first read has nothing to be a change from.
    pub fn prime(&mut self, snap: &crate::model::Snapshot) {
        self.poor = Some(is_poor(snap));
        self.wan_up = Some(is_up(snap));
    }

    /// One radio-tier read. Returns the rows this read earns, oldest first.
    pub fn radio(&mut self, snap: &crate::model::Snapshot, at_unix: u64) -> Vec<ObsEvent> {
        let mut rows = Vec::new();
        let up = is_up(snap);
        if let Some(before) = self.wan_up {
            if before != up {
                rows.push(ObsEvent::new(
                    EventKind::Wan,
                    crate::worker::tier_label(crate::worker::Tier::Radio),
                    at_unix,
                    if up { "up" } else { "down" },
                    None,
                ));
            }
        }
        self.wan_up = Some(up);

        let poor = is_poor(snap);
        if let Some(before) = self.poor {
            if before != poor {
                rows.push(ObsEvent::new(
                    EventKind::Signal,
                    crate::worker::tier_label(crate::worker::Tier::Radio),
                    at_unix,
                    if poor { "entered" } else { "left" },
                    signal_detail(snap),
                ));
            }
        }
        self.poor = Some(poor);
        rows
    }
}

/// Up means the firmware says it is connected. An address can be held while the
/// link is down, so the status is the witness, not the address.
fn is_up(snap: &crate::model::Snapshot) -> bool {
    snap.connection.status.eq_ignore_ascii_case("connected")
}

/// Poor is the same step the panels paint, so a panel cannot show a poor reading
/// while the log says the link is fine.
fn is_poor(snap: &crate::model::Snapshot) -> bool {
    let s = &snap.signal;
    crate::theme::worst_step(s.rsrp_dbm, s.rsrq_db, s.sinr_db) == Some(0)
}

/// The readings behind the crossing, in one line. Signal values are not identity,
/// so they can be kept; a cell id or an address would not be.
fn signal_detail(snap: &crate::model::Snapshot) -> Option<String> {
    let s = &snap.signal;
    let mut parts = Vec::new();
    if let Some(v) = s.rsrp_dbm {
        parts.push(format!("rsrp {v}"));
    }
    if let Some(v) = s.rsrq_db {
        parts.push(format!("rsrq {v}"));
    }
    if let Some(v) = s.sinr_db {
        parts.push(format!("sinr {v}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// A failed tier read is a row because a panel that froze has a cause. The class
/// is the stable word (`busy`, `denied`), so a reader can count them.
pub fn read_failed(
    tier: crate::worker::Tier,
    at_unix: u64,
    e: &crate::error::ApiError,
) -> ObsEvent {
    ObsEvent::new(
        EventKind::ReadFail,
        crate::worker::tier_label(tier),
        at_unix,
        e.class(),
        Some(e.to_string()),
    )
}

/// A write verdict. `detail` names what was written, never what the router
/// answered: the answer can echo a document and the log is not a dump.
pub fn write_settled(at_unix: u64, outcome: &str, detail: Option<String>) -> ObsEvent {
    ObsEvent::new(
        EventKind::Write,
        crate::worker::tier_label(crate::worker::Tier::Radio),
        at_unix,
        outcome,
        detail,
    )
}

/// A write verdict from its `WriteState`, so the TUI worker and the CLI cannot
/// word the same outcome differently. `what` says which document was written.
pub fn write_verdict(at_unix: u64, settled: &crate::write::WriteState, what: &str) -> ObsEvent {
    match settled {
        crate::write::WriteState::Success { intent, .. } => {
            write_settled(at_unix, "ok", Some(format!("{what} · {}", intent.label())))
        }
        crate::write::WriteState::Failed { intent, err } => write_settled(
            at_unix,
            "failed",
            Some(format!("{what} · {} · {err}", intent.label())),
        ),
        _ => write_settled(at_unix, "unsettled", Some(what.to_string())),
    }
}

/// A renew verdict, stamped by the radio tier because that is the group whose
/// address changed. The address itself is never a field of this row.
pub fn renew_settled(at_unix: u64, outcome: &str, detail: Option<String>) -> ObsEvent {
    ObsEvent::new(
        EventKind::Renew,
        crate::worker::tier_label(crate::worker::Tier::Radio),
        at_unix,
        outcome,
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_round_trips_through_its_word() {
        for k in [
            EventKind::Write,
            EventKind::Renew,
            EventKind::Wan,
            EventKind::ReadFail,
            EventKind::Signal,
        ] {
            assert_eq!(EventKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(EventKind::parse("attempt"), None);
    }
}
