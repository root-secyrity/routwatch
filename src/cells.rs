//! The cells catalog: every distinct serving cell this machine has read, kept as
//! a row of plain facts.
//!
//! Store only. The file says which cell was seen and when; it never says one is
//! better than another, because no capture has a second cell with a five minute
//! window to compare against. Ranking needs that window before it can be honest.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Snapshot;
use crate::signal_history::{self, Row};

/// Distinct cells kept on file. A modem roams between a handful; 64 leaves room
/// for a year of travel and still bounds a document the reader parses whole.
pub const CELL_CAP: usize = 64;

/// One serving cell, as read. Optional fields fill on first availability and are
/// never cleared afterwards: a sample without `tac` is silence from the firmware,
/// not evidence that the cell changed its tracking area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellRecord {
    pub band: String,
    pub pci: u16,
    pub earfcn_dl: Option<u32>,
    pub earfcn_ul: Option<u32>,
    pub tac: Option<u16>,
    pub first_seen: u64,
    pub last_seen: u64,
    pub rows: u64,
    pub best_sinr: Option<i32>,
    pub best_sinr_at: Option<u64>,
    pub earfcn_unknown: bool,
}

/// What makes two reads the same cell.
///
/// A PCI is only unique inside one carrier: the same 481 exists on every ARFCN
/// around it. So a keyed cell is `(earfcn_dl, pci)`, and a cell whose firmware
/// gave no ARFCN keeps a band key in its own variant, which never merges with a
/// keyed entry nor with another unkeyed one from a different band.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CellKey {
    Carrier { earfcn_dl: u32, pci: u16 },
    Unkeyed { band: String, pci: u16 },
}

/// The key a stored row carries: `(earfcn_dl, pci)` when the firmware gave a
/// downlink ARFCN, otherwise the band-and-PCI variant. `None` when the row names
/// no cell at all, which is the case a PCI alone must never fill.
///
/// One function, so the catalog that stores a cell and the watcher that reports a
/// TM change cannot disagree about which cell they are looking at.
pub fn key_from_row(row: &Row) -> Option<CellKey> {
    let pci = row.pci.and_then(|p| u16::try_from(p).ok())?;
    match row.earfcn_dl {
        Some(earfcn_dl) => Some(CellKey::Carrier { earfcn_dl, pci }),
        None => row.band.clone().map(|band| CellKey::Unkeyed { band, pci }),
    }
}

/// The catalog in memory. Loaded once from disk, then kept current by the radio
/// read that produced each sample.
#[derive(Debug, Default)]
pub struct CellCatalog {
    entries: BTreeMap<CellKey, CellRecord>,
    loaded: bool,
    last_path: Option<PathBuf>,
    last_minute: Option<u64>,
    writes: u64,
}

impl CellCatalog {
    /// Files written since this catalog was built. The catalog is the only
    /// counter of its own writes, so "ten samples, one write" is measured rather
    /// than inferred from a timestamp.
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// Rows newest first. The order is chronological on purpose: it says when a
    /// cell was last heard, not how good it is.
    pub fn sorted(&self) -> Vec<CellRecord> {
        newest_first(self.entries.values().cloned().collect())
    }

    /// Fold one radio read in, then save when the write policy says so.
    ///
    /// `path` is passed in rather than remembered from the first call: tests swap
    /// `XDG_DATA_HOME` between cases, and a remembered path would write into the
    /// directory of a previous test.
    pub fn observe(&mut self, s: &Snapshot, at: u64, path: &Path) {
        self.ensure_loaded(path);
        // The row gate is the signal log's: a sample with no figure at all is not
        // a cell sighting, and `Row` already decides that once.
        let Some(row) = Row::from_snapshot(s, at) else {
            return;
        };
        let Some(key) = key_from_row(&row) else {
            return;
        };
        let pci = match &key {
            CellKey::Carrier { pci, .. } | CellKey::Unkeyed { pci, .. } => *pci,
        };
        let tac = s.signal.tac.and_then(|t| u16::try_from(t).ok());
        let sinr = row.sinr_db.map(|v| v.round() as i32);
        let fresh = !self.entries.contains_key(&key);
        let rec = self.entries.entry(key).or_insert_with(|| CellRecord {
            band: row.band.clone().unwrap_or_else(|| "unknown".into()),
            pci,
            earfcn_dl: row.earfcn_dl,
            earfcn_ul: row.earfcn_ul,
            tac,
            first_seen: at,
            last_seen: at,
            rows: 1,
            best_sinr: sinr,
            best_sinr_at: sinr.map(|_| at),
            earfcn_unknown: row.earfcn_dl.is_none(),
        });
        if !fresh {
            rec.last_seen = at;
            rec.rows += 1;
            if rec.tac.is_none() {
                rec.tac = tac;
            }
            if rec.earfcn_ul.is_none() {
                rec.earfcn_ul = row.earfcn_ul;
            }
            if let Some(v) = sinr {
                if rec.best_sinr.is_none_or(|best| v > best) {
                    rec.best_sinr = Some(v);
                    rec.best_sinr_at = Some(at);
                }
            }
        }
        self.evict();
        let minute = at / 60;
        let due =
            fresh || self.last_minute != Some(minute) || self.last_path.as_deref() != Some(path);
        if due && self.save(path).is_ok() {
            self.writes += 1;
            self.last_minute = Some(minute);
            self.last_path = Some(path.to_path_buf());
        }
    }

    fn ensure_loaded(&mut self, path: &Path) {
        if self.loaded {
            return;
        }
        self.entries = read_records(path)
            .0
            .into_iter()
            .map(|r| (key_of(&r), r))
            .collect();
        self.loaded = true;
    }

    /// Drop the cell heard longest ago until the cap holds. Ties break on the key
    /// so the drop is deterministic.
    fn evict(&mut self) {
        while self.entries.len() > CELL_CAP {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by(|(ka, ra), (kb, rb)| ra.last_seen.cmp(&rb.last_seen).then(ka.cmp(kb)))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Whole file through a temp name, then a rename: a reader never sees half a
    /// catalog, and a failed write leaves no stray file next to it.
    fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;

        signal_history::ensure_dir(path)?;
        let body =
            serde_json::to_string_pretty(&newest_first(self.entries.values().cloned().collect()))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_file_name(format!(".cells-{}.json.tmp", std::process::id()));
        let written = (|| {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(body.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
        }
        Ok(())
    }
}

fn key_of(r: &CellRecord) -> CellKey {
    match r.earfcn_dl {
        Some(earfcn_dl) => CellKey::Carrier {
            earfcn_dl,
            pci: r.pci,
        },
        None => CellKey::Unkeyed {
            band: r.band.clone(),
            pci: r.pci,
        },
    }
}

fn newest_first(mut v: Vec<CellRecord>) -> Vec<CellRecord> {
    v.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then(a.band.cmp(&b.band))
            .then(a.pci.cmp(&b.pci))
            .then(a.earfcn_dl.cmp(&b.earfcn_dl))
    });
    v
}

/// `None` when the platform has no data dir: the catalog is then off rather than
/// written into the current directory.
pub fn cells_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("cells.json"))
}

/// The catalog as read from `path`, newest first, plus the reason the read was
/// partial.
///
/// A missing or empty file is an empty catalog and not a warning: a fresh install
/// has written nothing yet. A damaged document or a record this build cannot name
/// is reported so the reader can say so instead of printing an empty table that
/// looks like "no cell was ever seen".
pub fn read_records(path: &Path) -> (Vec<CellRecord>, Option<String>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), None),
        Err(e) => return (Vec::new(), Some(format!("cells: {e}"))),
    };
    if raw.trim().is_empty() {
        return (Vec::new(), None);
    }
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(e) => return (Vec::new(), Some(format!("cells: unreadable catalog ({e})"))),
    };
    let Some(items) = value.as_array() else {
        return (Vec::new(), Some("cells: catalog is not a list".into()));
    };
    let mut records = Vec::new();
    let mut skipped = 0usize;
    for item in items {
        match serde_json::from_value::<CellRecord>(item.clone()) {
            Ok(r) => records.push(r),
            Err(_) => skipped += 1,
        }
    }
    let warn = (skipped > 0).then(|| format!("cells: {skipped} record(s) skipped"));
    (newest_first(records), warn)
}

/// The catalog every radio read writes to.
fn catalog() -> &'static std::sync::Mutex<CellCatalog> {
    static CATALOG: std::sync::OnceLock<std::sync::Mutex<CellCatalog>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| std::sync::Mutex::new(CellCatalog::default()))
}

/// Record the serving cell of one radio read. Never fails the caller: the
/// snapshot the user asked for is not lost because a catalog could not be written.
pub fn record_cell(s: &Snapshot, at: u64) {
    let Some(path) = cells_path() else {
        return;
    };
    let mut c = catalog()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    c.observe(s, at, &path);
}

/// The `cells list` table: a title, one header, one line per cell, and a tail line
/// when the cap held. Nothing here ranks a cell; the order is `last_seen` alone.
pub fn cells_table(records: &[CellRecord]) -> String {
    let mut out = format!("Cells · {} unique\n", records.len());
    if records.is_empty() {
        return out;
    }
    out.push_str(&format!(
        "{:<4}  {:<3}  {:<9}  {:<3}  {:<4}  {:<19}  {:<19}  {:<9}  {}\n",
        "band", "pci", "earfcn_dl", "tac", "rows", "first_seen", "last_seen", "best_sinr", "earfcn",
    ));
    for r in records {
        let sinr = r
            .best_sinr
            .map(|v| format!("{v} dB"))
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "{:<4}  {:<3}  {:<9}  {:<3}  {:<4}  {:<19}  {:<19}  {:<9}  {}\n",
            r.band,
            r.pci,
            r.earfcn_dl
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            r.tac.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            r.rows,
            signal_history::local_iso(r.first_seen),
            signal_history::local_iso(r.last_seen),
            sinr,
            if r.earfcn_unknown {
                "earfcn unknown"
            } else {
                "-"
            },
        ));
    }
    if records.len() >= CELL_CAP {
        out.push_str(&format!("cap {CELL_CAP} reached · oldest evicted\n"));
    }
    out
}

/// One row of the cell-change log, as `cells changes` prints it. The detail is
/// the log's own line carried through, so a reader sees what was written rather
/// than a second rendering of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangeRow {
    /// `{kind}-{at_unix}`, the same shape an incident id has, so a caller can
    /// name one row of the log without the log carrying an id column.
    pub id: String,
    pub at_unix: u64,
    /// The same moment in local time, for reading.
    pub at: String,
    pub tier: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The `cell` rows of an event log, newest first.
///
/// `window` keeps the last N seconds counted back from the newest row, not from
/// the clock: a log that stopped three days ago still prints the rows it holds
/// instead of an empty frame.
pub fn change_rows(loaded: &crate::events::Loaded, window: Option<u64>) -> Vec<ChangeRow> {
    let Some(newest) = loaded
        .rows
        .iter()
        .filter(|e| e.kind == "cell")
        .map(|e| e.at_unix)
        .max()
    else {
        return Vec::new();
    };
    let floor = window.map(|w| newest.saturating_sub(w));
    let mut rows: Vec<ChangeRow> = loaded
        .rows
        .iter()
        .filter(|e| e.kind == "cell")
        .filter(|e| floor.is_none_or(|f| e.at_unix >= f))
        .map(|e| ChangeRow {
            id: format!("{}-{}", e.kind, e.at_unix),
            at_unix: e.at_unix,
            at: signal_history::local_iso(e.at_unix),
            tier: e.tier.clone(),
            outcome: e.outcome.clone(),
            detail: e.detail.clone(),
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.at_unix));
    rows
}

/// The `cells changes` table. It lists what the log recorded; it draws no
/// conclusion from it, because a cell change is context and not a fault.
pub fn changes_table(rows: &[ChangeRow]) -> String {
    let mut out = format!("Cell changes · {}\n", rows.len());
    if rows.is_empty() {
        out.push_str(
            "no cell change recorded: the first read of a cell is a baseline, not a change\n",
        );
        return out;
    }
    out.push_str(&format!("{:<16}  {:<19}  {}\n", "id", "at", "change"));
    for r in rows {
        out.push_str(&format!(
            "{:<16}  {:<19}  {}\n",
            r.id,
            r.at,
            r.detail.as_deref().unwrap_or("-")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Signal;
    use std::path::PathBuf;

    /// An event log built by hand: two cell rows and two rows of other kinds, so
    /// the filter is tested rather than assumed.
    fn event_log() -> crate::events::Loaded {
        use crate::events::{EventKind, Loaded, ObsEvent};
        Loaded {
            rows: vec![
                ObsEvent::new(
                    EventKind::Cell,
                    "signal",
                    1_700_000_000,
                    "changed",
                    Some("earfcn_dl 100 pci 481 -> earfcn_dl 300 pci 395 held 2".into()),
                ),
                ObsEvent::new(
                    EventKind::Tm,
                    "signal",
                    1_700_000_100,
                    "changed",
                    Some("earfcn_dl 100 pci 481 tm TM3 -> TM4".into()),
                ),
                ObsEvent::new(EventKind::ReadFail, "signal", 1_700_000_200, "busy", None),
                ObsEvent::new(
                    EventKind::Cell,
                    "signal",
                    1_700_090_000,
                    "changed",
                    Some("earfcn_dl 300 pci 395 -> earfcn_dl 100 pci 481 held 2".into()),
                ),
            ],
            damaged: 0,
        }
    }

    #[test]
    fn changes_reads_only_cell_rows_newest_first() {
        let rows = change_rows(&event_log(), None);
        assert_eq!(rows.len(), 2, "only the cell rows: {rows:?}");
        assert_eq!(rows[0].at_unix, 1_700_090_000);
        assert_eq!(rows[1].at_unix, 1_700_000_000);
        assert_eq!(rows[0].id, "cell-1700090000");
        assert_eq!(rows[0].tier, "signal");
        assert_eq!(rows[0].outcome, "changed");
        assert!(rows[0].at.contains('T'), "{}", rows[0].at);
    }

    /// The window counts back from the newest row of the log, not from the clock:
    /// a log that stopped days ago still prints what it holds.
    #[test]
    fn the_window_counts_back_from_the_newest_row() {
        let loaded = event_log();
        let narrow = change_rows(&loaded, Some(3600));
        assert_eq!(narrow.len(), 1, "{narrow:?}");
        assert_eq!(narrow[0].at_unix, 1_700_090_000);
        let wide = change_rows(&loaded, Some(90_000));
        assert_eq!(wide.len(), 2);
    }

    #[test]
    fn a_log_with_no_cell_row_prints_the_empty_text() {
        let empty = crate::events::Loaded::default();
        assert!(change_rows(&empty, None).is_empty());
        let text = changes_table(&[]);
        assert_eq!(text, "Cell changes · 0\nno cell change recorded: the first read of a cell is a baseline, not a change\n");
    }

    /// A cell change is context: the row may name what changed and never why.
    #[test]
    fn the_changes_table_states_no_cause() {
        let text = changes_table(&change_rows(&event_log(), None));
        assert!(text.starts_with("Cell changes · 2\n"), "{text}");
        for word in ["because", "caused", "due to", "rsrp", "sinr"] {
            assert!(!text.contains(word), "{word} in:\n{text}");
        }
    }

    /// A scratch `cells.json`, removed when the test ends (panic or not).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for Scratch {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    fn scratch(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("rw-cells-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir.join("cells.json"))
    }

    fn sample(
        band: &str,
        pci: Option<u32>,
        earfcn: Option<&str>,
        sinr: Option<f32>,
        tac: Option<u32>,
    ) -> Snapshot {
        Snapshot {
            signal: Signal {
                band: Some(band.to_string()),
                pci,
                earfcn: earfcn.map(str::to_string),
                rsrp_dbm: Some(-95.0),
                sinr_db: sinr,
                tac,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn cell(pci: Option<u32>) -> Snapshot {
        sample("B1", pci, Some("DL:100 UL:18100"), Some(9.0), Some(516))
    }

    #[test]
    fn a_new_cell_starts_at_one_row_and_the_read_time() {
        let p = scratch("new");
        let mut c = CellCatalog::default();
        c.observe(&cell(Some(481)), 1_700_000_000, &p);
        let rows = c.sorted();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.band, "B1");
        assert_eq!(r.pci, 481);
        assert_eq!(r.earfcn_dl, Some(100));
        assert_eq!(r.earfcn_ul, Some(18100));
        assert_eq!(r.tac, Some(516));
        assert_eq!(r.rows, 1);
        assert_eq!(r.first_seen, 1_700_000_000);
        assert_eq!(r.last_seen, 1_700_000_000);
        assert_eq!(r.best_sinr, Some(9));
        assert_eq!(r.best_sinr_at, Some(1_700_000_000));
        assert!(!r.earfcn_unknown);
    }

    #[test]
    fn a_repeat_read_updates_the_same_record() {
        let p = scratch("repeat");
        let mut c = CellCatalog::default();
        c.observe(&cell(Some(481)), 1_700_000_000, &p);
        c.observe(&cell(Some(481)), 1_700_000_060, &p);
        let rows = c.sorted();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rows, 2);
        assert_eq!(rows[0].first_seen, 1_700_000_000);
        assert_eq!(rows[0].last_seen, 1_700_000_060);
    }

    #[test]
    fn a_better_sinr_moves_the_best_and_a_worse_one_does_not() {
        let p = scratch("sinr");
        let mut c = CellCatalog::default();
        c.observe(
            &sample("B1", Some(481), Some("DL:100"), Some(9.0), None),
            1_000,
            &p,
        );
        c.observe(
            &sample("B1", Some(481), Some("DL:100"), Some(12.0), None),
            1_060,
            &p,
        );
        c.observe(
            &sample("B1", Some(481), Some("DL:100"), Some(3.0), None),
            1_120,
            &p,
        );
        let r = &c.sorted()[0];
        assert_eq!(r.best_sinr, Some(12));
        assert_eq!(r.best_sinr_at, Some(1_060));
        assert_eq!(r.rows, 3);
    }

    #[test]
    fn a_sample_without_earfcn_takes_the_band_key_and_never_merges() {
        let p = scratch("unkeyed");
        let mut c = CellCatalog::default();
        c.observe(
            &sample("B1", Some(481), Some("DL:100"), Some(9.0), None),
            1_000,
            &p,
        );
        c.observe(&sample("B1", Some(481), None, Some(9.0), None), 1_060, &p);
        let rows = c.sorted();
        assert_eq!(rows.len(), 2, "an unknown ARFCN is its own entry");
        let unkeyed = rows
            .iter()
            .find(|r| r.earfcn_dl.is_none())
            .expect("unkeyed row");
        assert!(unkeyed.earfcn_unknown);
        assert_eq!(unkeyed.rows, 1);
        // Two keyed cells never fold into the unkeyed one either.
        c.observe(
            &sample("B1", Some(481), Some("DL:300"), Some(9.0), None),
            1_120,
            &p,
        );
        assert_eq!(c.sorted().len(), 3);
    }

    #[test]
    fn two_carriers_with_the_same_band_and_pci_stay_apart() {
        let p = scratch("carriers");
        let mut c = CellCatalog::default();
        c.observe(
            &sample("B3", Some(395), Some("DL:100 UL:18100"), Some(8.0), None),
            1_000,
            &p,
        );
        c.observe(
            &sample("B3", Some(395), Some("DL:300 UL:18300"), Some(8.0), None),
            1_060,
            &p,
        );
        let rows = c.sorted();
        assert_eq!(rows.len(), 2);
        let dls: Vec<_> = rows.iter().map(|r| r.earfcn_dl).collect();
        assert!(dls.contains(&Some(100)) && dls.contains(&Some(300)));
    }

    #[test]
    fn a_sample_without_tac_does_not_erase_a_known_tac() {
        let p = scratch("tac");
        let mut c = CellCatalog::default();
        c.observe(&cell(Some(481)), 1_000, &p);
        c.observe(
            &sample("B1", Some(481), Some("DL:100 UL:18100"), Some(9.0), None),
            1_060,
            &p,
        );
        assert_eq!(c.sorted()[0].tac, Some(516));
    }

    #[test]
    fn the_sixty_fifth_cell_evicts_the_oldest_last_seen() {
        let p = scratch("cap");
        let mut c = CellCatalog::default();
        for n in 0..65u32 {
            let dl = format!("DL:{}", 100 + n);
            let s = sample("B1", Some(481), Some(&dl), Some(9.0), None);
            c.observe(&s, 1_000 + u64::from(n) * 60, &p);
        }
        let rows = c.sorted();
        assert_eq!(rows.len(), CELL_CAP);
        assert!(
            rows.iter().all(|r| r.earfcn_dl != Some(100)),
            "the first cell heard is the one evicted"
        );
        assert!(rows.iter().any(|r| r.earfcn_dl == Some(100 + 64)));
    }

    #[test]
    fn the_file_is_0600_and_leaves_no_temp_behind() {
        use std::os::unix::fs::PermissionsExt;
        let p = scratch("perm");
        let mut c = CellCatalog::default();
        c.observe(&cell(Some(481)), 1_700_000_000, &p);
        let mode = std::fs::metadata(&p).expect("file").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<String> = std::fs::read_dir(p.parent().unwrap())
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![String::from("cells.json")]);
        assert_eq!(read_records(&p).0, c.sorted());
    }

    #[test]
    fn a_missing_or_empty_file_reads_as_an_empty_catalog() {
        let p = scratch("empty");
        assert_eq!(read_records(&p), (Vec::new(), None));
        std::fs::write(&p, "").expect("empty");
        assert_eq!(read_records(&p), (Vec::new(), None));
        std::fs::write(&p, "  \n\n").expect("blank");
        assert_eq!(read_records(&p), (Vec::new(), None));
        let mut c = CellCatalog::default();
        c.observe(&cell(Some(481)), 1_000, &p);
        assert_eq!(
            c.sorted().len(),
            1,
            "an empty file still accepts the first cell"
        );
    }

    #[test]
    fn a_damaged_file_warns_instead_of_panicking() {
        let p = scratch("damaged");
        std::fs::write(&p, "{not json").expect("write");
        let (rows, warn) = read_records(&p);
        assert!(rows.is_empty());
        assert!(warn.expect("a warning").contains("unreadable"));
    }

    #[test]
    fn ten_samples_in_one_minute_write_once() {
        let p = scratch("minute");
        let mut c = CellCatalog::default();
        for n in 0..10u64 {
            c.observe(&cell(Some(481)), 1_700_000_000 + n, &p);
        }
        assert_eq!(c.writes(), 1, "one minute pulse, one file write");
        assert_eq!(c.sorted()[0].rows, 10);
    }

    #[test]
    fn a_pci_alone_is_never_a_key() {
        let p = scratch("pci");
        let mut c = CellCatalog::default();
        // A PCI with neither ARFCN nor band names no cell.
        let mut bare = sample("B1", Some(481), None, Some(9.0), None);
        bare.signal.band = None;
        c.observe(&bare, 1_000, &p);
        // A sample with no PCI names no cell either.
        c.observe(
            &sample("B1", None, Some("DL:100"), Some(9.0), None),
            1_060,
            &p,
        );
        assert!(c.sorted().is_empty());
        assert_eq!(c.writes(), 0);
        assert!(!p.exists());
    }

    #[test]
    fn a_stored_record_keys_the_same_as_its_row() {
        let p = scratch("key-from-row");
        let s = cell(Some(481));
        let row = Row::from_snapshot(&s, 1_000).expect("a row with figures");
        let key = key_from_row(&row).expect("the row names a cell");
        assert_eq!(
            key,
            CellKey::Carrier {
                earfcn_dl: 100,
                pci: 481
            }
        );

        let mut c = CellCatalog::default();
        c.observe(&s, 1_000, &p);
        let (stored, rec) = c.entries.iter().next().expect("one entry");
        assert_eq!(
            stored, &key,
            "the catalog must file the cell under the key its row gives"
        );
        assert_eq!((rec.pci, rec.earfcn_dl), (481, Some(100)));

        // The same agreement on the other variant: no ARFCN, so band and PCI.
        let silent = sample("B3", Some(395), None, Some(8.0), Some(516));
        let row = Row::from_snapshot(&silent, 1_060).expect("a row with figures");
        assert_eq!(
            key_from_row(&row),
            Some(CellKey::Unkeyed {
                band: "B3".into(),
                pci: 395
            })
        );

        // And no key at all when neither an ARFCN nor a PCI is there.
        let nameless = sample("B1", None, None, Some(9.0), Some(516));
        let row = Row::from_snapshot(&nameless, 1_120).expect("a row with figures");
        assert_eq!(key_from_row(&row), None);
    }

    #[test]
    fn a_sample_with_no_signal_figure_is_not_recorded() {
        let p = scratch("nofigure");
        let mut c = CellCatalog::default();
        let mut s = cell(Some(481));
        s.signal.rsrp_dbm = None;
        s.signal.sinr_db = None;
        s.signal.rsrq_db = None;
        c.observe(&s, 1_000, &p);
        assert!(c.sorted().is_empty());
        assert_eq!(c.writes(), 0);
    }

    #[test]
    fn the_catalog_is_loaded_once_not_per_read() {
        let p = scratch("loadonce");
        let mut a = CellCatalog::default();
        a.observe(&cell(Some(481)), 1_000, &p);
        let mut b = CellCatalog::default();
        b.observe(&cell(Some(481)), 1_060, &p);
        assert_eq!(
            b.sorted()[0].rows,
            2,
            "the file was read on the first sample"
        );
        // A file changed behind the catalog's back is not merged in: the catalog
        // reads once, then trusts its own map.
        let foreign = serde_json::json!([{
            "band": "B1", "pci": 481, "earfcn_dl": 100, "earfcn_ul": 18100,
            "tac": 516, "first_seen": 0, "last_seen": 0, "rows": 99,
            "best_sinr": 1, "best_sinr_at": 0, "earfcn_unknown": false
        }]);
        std::fs::write(&p, serde_json::to_string(&foreign).unwrap()).expect("write");
        b.observe(&cell(Some(481)), 1_120, &p);
        assert_eq!(b.sorted()[0].rows, 3, "no second read of the file");
    }
}
