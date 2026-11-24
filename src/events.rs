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

/// The nine families of state change. `ReadFail` is a state change too: a tier
/// that stopped answering is why a panel froze. `Config` carries field names
/// only, never a value and never a fingerprint. `Tm` is the cell's transmission
/// mode moving while the cell itself stayed. `Cell` is the serving cell itself
/// changing, which is context for everything read after it, never a verdict on
/// the link. `Budget` is the local ledger
/// crossing 80% or 95% of the month's budget, a fact about this machine's
/// history and not about the link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Write,
    Renew,
    Wan,
    ReadFail,
    Signal,
    Config,
    Tm,
    Cell,
    Budget,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Write => "write",
            EventKind::Renew => "renew",
            EventKind::Wan => "wan",
            EventKind::ReadFail => "read",
            EventKind::Signal => "signal",
            EventKind::Config => "config",
            EventKind::Tm => "tm",
            EventKind::Cell => "cell",
            EventKind::Budget => "budget",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "write" => Some(EventKind::Write),
            "renew" => Some(EventKind::Renew),
            "wan" => Some(EventKind::Wan),
            "read" => Some(EventKind::ReadFail),
            "signal" => Some(EventKind::Signal),
            "config" => Some(EventKind::Config),
            "tm" => Some(EventKind::Tm),
            "cell" => Some(EventKind::Cell),
            "budget" => Some(EventKind::Budget),
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

/// The states the radio tier can change from one read to the next. Rows are
/// produced on the crossing only, so a link that stays poor overnight writes one
/// row, not one per read. A TM value is a crossing too, but a noisier one, so it
/// must be read twice in a row before it counts.
#[derive(Debug, Default)]
pub struct Watch {
    poor: Option<bool>,
    wan_up: Option<bool>,
    /// The cell the previous read named, and the TM confirmed for it.
    prev_cell: Option<crate::cells::CellKey>,
    confirmed_tm: Option<String>,
    /// A TM value seen once, with how many reads in a row have shown it.
    pending_tm: Option<(String, u32)>,
    /// A cell key seen once, with how many reads in a row have shown it.
    pending_cell: Option<(crate::cells::CellKey, u32)>,
}

impl Watch {
    /// A full collect sees every document, so it sets the baseline without
    /// writing: a first read has nothing to be a change from.
    pub fn prime(&mut self, snap: &crate::model::Snapshot) {
        self.poor = Some(is_poor(snap));
        self.wan_up = Some(is_up(snap));
        self.prev_cell = cell_key(snap);
        self.confirmed_tm = snap.signal.tm.clone();
        self.pending_tm = None;
        self.pending_cell = None;
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

        // A cell change is a change of context, not a verdict: the row says
        // which cell the read left and which one it arrived on. The rule is the
        // TM rule, because the firmware can name another cell for one sample:
        // two reads in a row make the change, and a return drops the candidate.
        if let Some(key) = cell_key(snap) {
            match self.prev_cell.clone() {
                // A first read is a baseline: there is nothing to be a change
                // from, and the catalog records it as a sighting instead.
                None => {
                    self.prev_cell = Some(key);
                    self.pending_cell = None;
                }
                Some(prev) if prev == key => {
                    // The confirmed cell answered, so a single odd read is gone.
                    self.pending_cell = None;
                }
                Some(prev) => {
                    let held = match self.pending_cell.as_mut() {
                        Some((candidate, held)) if *candidate == key => {
                            *held += 1;
                            *held
                        }
                        _ => {
                            self.pending_cell = Some((key.clone(), 1));
                            1
                        }
                    };
                    if held >= 2 {
                        self.prev_cell = Some(key.clone());
                        self.pending_cell = None;
                        // The TM arrived with the cell, so it is that cell's
                        // baseline. Writing a TM row here would report the same
                        // change twice.
                        self.confirmed_tm = snap.signal.tm.clone();
                        self.pending_tm = None;
                        rows.push(ObsEvent::new(
                            EventKind::Cell,
                            crate::worker::tier_label(crate::worker::Tier::Radio),
                            at_unix,
                            "changed",
                            Some(cell_change_detail(snap, &prev, &key, held)),
                        ));
                    }
                }
            }
        }

        // TM moves on its own while the cell stays, so it gets its own row.
        // One read that disagrees with the confirmed value is not a change yet:
        // some firmware alternates the field for a single sample.
        if let Some(key) = cell_key(snap) {
            if self.prev_cell.as_ref() == Some(&key) {
                if let Some(tm) = snap.signal.tm.clone() {
                    if self.confirmed_tm.is_none() {
                        self.confirmed_tm = Some(tm);
                        self.pending_tm = None;
                    } else if self.confirmed_tm.as_deref() == Some(tm.as_str()) {
                        self.pending_tm = None;
                    } else {
                        let held = match self.pending_tm.as_mut() {
                            Some((candidate, held)) if candidate == &tm => {
                                *held += 1;
                                *held
                            }
                            _ => {
                                self.pending_tm = Some((tm.clone(), 1));
                                1
                            }
                        };
                        if held >= 2 {
                            let before = self.confirmed_tm.clone();
                            self.confirmed_tm = Some(tm.clone());
                            self.pending_tm = None;
                            rows.push(ObsEvent::new(
                                EventKind::Tm,
                                crate::worker::tier_label(crate::worker::Tier::Radio),
                                at_unix,
                                "changed",
                                Some(tm_detail(&key, before.as_deref(), &tm, held)),
                            ));
                        }
                    }
                }
            }
        }
        rows
    }
}

/// The cell this read names, through the same key the catalog stores.
fn cell_key(snap: &crate::model::Snapshot) -> Option<crate::cells::CellKey> {
    crate::signal_history::Row::from_snapshot(snap, 0)
        .as_ref()
        .and_then(crate::cells::key_from_row)
}

/// One cell key as a line a person reads. ARFCN and PCI are broadcast
/// parameters, so they can be named; a key without an ARFCN says so instead of
/// printing a dash that would read like a missing value.
fn cell_label(key: &crate::cells::CellKey) -> String {
    match key {
        crate::cells::CellKey::Carrier { earfcn_dl, pci } => {
            format!("earfcn_dl {earfcn_dl} pci {pci}")
        }
        crate::cells::CellKey::Unkeyed { band, pci } => {
            format!("band {band} pci {pci} earfcn unknown")
        }
    }
}

/// The cell and the TM move, in one line. Nothing here is device identity.
fn tm_detail(key: &crate::cells::CellKey, before: Option<&str>, after: &str, held: u32) -> String {
    let cell = cell_label(key);
    format!(
        "{cell} tm {} -> {after} held {held}",
        before.unwrap_or("unknown")
    )
}

/// The cell a read left and the cell it arrived on, with the band and the TAC
/// the read reported. No signal figure: the row says the context moved, and a
/// figure here would read as a verdict on the link.
fn cell_change_detail(
    snap: &crate::model::Snapshot,
    before: &crate::cells::CellKey,
    after: &crate::cells::CellKey,
    held: u32,
) -> String {
    let mut parts = vec![format!("{} -> {}", cell_label(before), cell_label(after))];
    if let Some(band) = snap.signal.band.as_deref() {
        parts.push(format!("band {band}"));
    }
    match snap.signal.tac.and_then(|t| u16::try_from(t).ok()) {
        Some(tac) => parts.push(format!("tac {tac}")),
        None => parts.push("tac unknown".to_string()),
    }
    parts.push(format!("held {held}"));
    parts.join(" ")
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
        let kinds = [
            EventKind::Write,
            EventKind::Renew,
            EventKind::Wan,
            EventKind::ReadFail,
            EventKind::Signal,
            EventKind::Config,
            EventKind::Tm,
            EventKind::Cell,
            EventKind::Budget,
        ];
        // The module doc calls them the nine families: this list is the count.
        assert_eq!(kinds.len(), 9);
        for k in kinds {
            assert_eq!(EventKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(EventKind::parse("attempt"), None);
    }

    /// One radio read, one cell, one TM value. Every other field is fixed so a
    /// change in the row can only come from the TM or the cell.
    fn read(pci: Option<u32>, earfcn: Option<&str>, tm: Option<&str>) -> crate::model::Snapshot {
        let mut s = crate::model::Snapshot::default();
        s.signal.rsrp_dbm = Some(-95.0);
        s.signal.sinr_db = Some(9.0);
        s.signal.band = Some("B1".into());
        s.signal.pci = pci;
        s.signal.earfcn = earfcn.map(str::to_string);
        s.signal.tm = tm.map(str::to_string);
        s
    }

    const EARFCN: Option<&str> = Some("DL:100 UL:18100");

    #[test]
    fn a_tm_change_holds_two_reads_before_it_writes() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        assert!(
            w.radio(&read(Some(481), EARFCN, Some("TM[4]")), 100)
                .is_empty(),
            "one read that disagrees is not a change yet"
        );
        let rows = w.radio(&read(Some(481), EARFCN, Some("TM[4]")), 160);
        assert_eq!(rows.len(), 1, "two reads in a row earn one row: {rows:?}");
        let e = &rows[0];
        assert_eq!(e.kind, "tm");
        assert_eq!(e.tier, "signal");
        assert_eq!(e.outcome, "changed");
        let detail = e.detail.as_deref().expect("the row names the cell");
        assert!(detail.contains("earfcn_dl 100"), "{detail}");
        assert!(detail.contains("pci 481"), "{detail}");
        assert!(detail.contains("TM[3] -> TM[4]"), "{detail}");
        assert!(detail.contains("held 2"), "{detail}");

        assert!(
            w.radio(&read(Some(481), EARFCN, Some("TM[4]")), 220)
                .is_empty(),
            "the value is confirmed now, so it is not a change again"
        );
    }

    #[test]
    fn a_single_odd_read_writes_no_tm_event() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        assert!(w
            .radio(&read(Some(481), EARFCN, Some("TM[4]")), 100)
            .is_empty());
        assert!(
            w.radio(&read(Some(481), EARFCN, Some("TM[3]")), 160)
                .is_empty(),
            "the confirmed value came back, so the odd read is dropped"
        );
        assert!(
            w.radio(&read(Some(481), EARFCN, Some("TM[4]")), 220)
                .is_empty(),
            "and the next single read starts over instead of counting to two"
        );
    }

    #[test]
    fn a_cell_change_is_not_a_tm_change() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        assert!(
            w.radio(
                &read(Some(395), Some("DL:300 UL:18300"), Some("TM[4]")),
                100
            )
            .is_empty(),
            "another cell is another baseline, not a TM move"
        );
        let rows = w.radio(
            &read(Some(395), Some("DL:300 UL:18300"), Some("TM[4]")),
            160,
        );
        assert_eq!(rows.len(), 1, "the second read confirms the cell: {rows:?}");
        assert_eq!(rows[0].kind, "cell");
        assert!(
            rows.iter().all(|e| e.kind != "tm"),
            "the TM arrived with the cell, so it is not a TM change: {rows:?}"
        );
    }

    #[test]
    fn a_reading_without_a_key_writes_no_tm_event() {
        let mut w = Watch::default();
        w.prime(&read(None, EARFCN, Some("TM[3]")));

        assert!(
            w.radio(&read(None, EARFCN, Some("TM[4]")), 100).is_empty(),
            "a PCI alone names no cell, so it cannot carry a cell's TM"
        );
        assert!(w.radio(&read(None, EARFCN, Some("TM[4]")), 160).is_empty());
    }

    const OTHER: Option<&str> = Some("DL:300 UL:18300");

    #[test]
    fn a_cell_change_held_two_reads_writes_one_row() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        assert!(
            w.radio(&read(Some(395), OTHER, Some("TM[3]")), 100)
                .is_empty(),
            "one read that names another cell is not a change yet"
        );
        let mut arrived = read(Some(395), OTHER, Some("TM[3]"));
        arrived.signal.band = Some("B3".into());
        arrived.signal.tac = Some(516);
        let rows = w.radio(&arrived, 160);
        assert_eq!(rows.len(), 1, "two reads in a row earn one row: {rows:?}");
        let e = &rows[0];
        assert_eq!(e.kind, "cell");
        assert_eq!(e.tier, "signal");
        assert_eq!(e.outcome, "changed");
        assert_eq!(e.at_unix, 160);
        let detail = e.detail.as_deref().expect("the row names both cells");
        assert!(detail.contains("earfcn_dl 100 pci 481"), "{detail}");
        assert!(detail.contains("earfcn_dl 300 pci 395"), "{detail}");
        assert!(detail.contains("band B3"), "{detail}");
        assert!(detail.contains("tac 516"), "{detail}");
        assert!(detail.contains("held 2"), "{detail}");
        assert!(
            !detail.contains("rsrp") && !detail.contains("sinr"),
            "the row is context, not a verdict: {detail}"
        );

        assert!(
            w.radio(&read(Some(395), OTHER, Some("TM[3]")), 220)
                .is_empty(),
            "staying on the new cell is not another change"
        );
    }

    #[test]
    fn a_single_stray_cell_read_writes_nothing() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        assert!(
            w.radio(&read(Some(395), OTHER, Some("TM[4]")), 100)
                .is_empty(),
            "one read that names another cell is not a change yet"
        );
        assert!(
            w.radio(&read(Some(481), EARFCN, Some("TM[3]")), 160)
                .is_empty(),
            "the confirmed cell came back, so the odd read is dropped"
        );
        assert!(
            w.radio(&read(Some(481), EARFCN, Some("TM[3]")), 220)
                .is_empty(),
            "and the stray read left no TM baseline behind it"
        );
    }

    #[test]
    fn a_return_to_the_previous_cell_is_its_own_change() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));
        assert!(w
            .radio(&read(Some(395), OTHER, Some("TM[3]")), 100)
            .is_empty());
        assert_eq!(
            w.radio(&read(Some(395), OTHER, Some("TM[3]")), 160).len(),
            1
        );

        assert!(w
            .radio(&read(Some(481), EARFCN, Some("TM[3]")), 220)
            .is_empty());
        let rows = w.radio(&read(Some(481), EARFCN, Some("TM[3]")), 280);
        assert_eq!(
            rows.len(),
            1,
            "the way back is a change of its own: {rows:?}"
        );
        assert_eq!(rows[0].kind, "cell");
        let detail = rows[0].detail.as_deref().expect("both cells");
        assert!(
            detail.contains("earfcn_dl 300 pci 395 -> earfcn_dl 100 pci 481"),
            "{detail}"
        );
    }

    #[test]
    fn an_unkeyed_cell_never_merges_with_a_keyed_one() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        // Same band and PCI, but the firmware reported no ARFCN: the key is the
        // band variant, so it is another cell and it says why.
        assert!(w
            .radio(&read(Some(481), None, Some("TM[3]")), 100)
            .is_empty());
        let rows = w.radio(&read(Some(481), None, Some("TM[3]")), 160);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let detail = rows[0].detail.as_deref().expect("both keys");
        assert!(
            detail.contains("earfcn_dl 100 pci 481 -> band B1 pci 481 earfcn unknown"),
            "{detail}"
        );
    }

    #[test]
    fn a_cell_change_and_a_tm_change_write_one_row() {
        let mut w = Watch::default();
        w.prime(&read(Some(481), EARFCN, Some("TM[3]")));

        assert!(w
            .radio(&read(Some(395), OTHER, Some("TM[4]")), 100)
            .is_empty());
        let rows = w.radio(&read(Some(395), OTHER, Some("TM[4]")), 160);
        assert_eq!(rows.len(), 1, "one change, one row: {rows:?}");
        assert_eq!(rows[0].kind, "cell");
        assert!(
            rows.iter().all(|e| e.kind != "tm"),
            "the TM came with the cell, so it is that cell's baseline: {rows:?}"
        );
        assert!(
            w.radio(&read(Some(395), OTHER, Some("TM[4]")), 220)
                .is_empty(),
            "and the new baseline is not a change either"
        );
    }
}
