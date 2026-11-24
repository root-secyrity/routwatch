//! Comparing one cell's link around a logged event.
//!
//! The comparison lives inside a single cell. Two cells measured in different
//! hours are not a difference: the window, the load and the weather moved too.
//! So the window is split into stretches by cell key, and each stretch is
//! compared with itself, before the event against after it.
//!
//! Everything here is a pure function of numbers handed in: no clock, no file,
//! no network. The caller reads the three logs and passes the window and the
//! measured smear width.

use serde::Serialize;

use crate::advisor::{median, median_f64, percentile, Confidence};
use crate::cells::{key_from_row, CellKey};
use crate::events::ObsEvent;
use crate::history::Entry;
use crate::signal_history::{Row, SIGNAL_SAMPLE_SECS};

/// Seconds on each side of the event unless the caller says otherwise: half an
/// hour, which is the smallest window this machine has measured twice.
pub const WINDOW_SECS: u64 = 1800;

/// Rows a side needs before a difference is printed: ten minutes of the log's
/// one-a-minute cadence.
pub const MIN_ROWS: usize = 10;

/// The share of the window a side must have held a readable cell for. Half the
/// window is a comparison; less is a memory of a few minutes dressed as thirty.
pub const MIN_COVERAGE: f64 = 0.5;

/// How far a SINR median must move to be a change and not the weather: three
/// times the measured spread of one cell's SINR (1.202 dB), declared once.
pub const SINR_NOISE_DB: f32 = 3.6;

/// The same for RSRP: three times its measured spread (0.223 dB).
pub const RSRP_NOISE_DB: f32 = 0.7;

/// What a side-by-side comparison supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Both medians moved past their noise threshold, one of them upward.
    Improved,
    /// Both medians moved past their noise threshold, one of them downward.
    Degraded,
    /// Every move stayed inside the threshold: this is the weather.
    WithinNoise,
    /// One figure improved while the other fell, so no single verdict holds.
    Mixed,
    /// A side has too few rows or too little coverage to say anything.
    Insufficient,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Improved => "improved",
            Class::Degraded => "degraded",
            Class::WithinNoise => "within noise",
            Class::Mixed => "mixed",
            Class::Insufficient => "insufficient",
        }
    }

    /// A class word for a sentence: the table and the prose agree on it.
    pub fn sentence(self) -> &'static str {
        match self {
            Class::Improved => "improved beyond the noise threshold",
            Class::Degraded => "fell beyond the noise threshold",
            Class::WithinNoise => "stayed inside the noise threshold",
            Class::Mixed => "moved one way on SINR and the other on RSRP",
            Class::Insufficient => "not comparable: a side is too thin",
        }
    }
}

/// One side of the event: the minutes before it, or the minutes after.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Side {
    pub rows: usize,
    /// Seconds the log actually covered on this side: one sample per row, at the
    /// measured minute cadence, never more than the window.
    pub watched_secs: u64,
    /// `watched_secs / window_secs`, so 0.5 means half the window was read.
    pub coverage: f64,
    pub sinr_median: Option<f32>,
    /// Tenth percentile: the weak end, which is the moment that gets noticed.
    pub sinr_p10: Option<f32>,
    pub rsrp_median: Option<f32>,
    pub download_median_mbps: Option<f64>,
    pub speed_rows: usize,
}

/// One stretch of the window served by one cell key, compared with itself.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Segment {
    /// The cell this stretch was served by, as the key names it.
    pub cell: String,
    pub before: Side,
    pub after: Side,
    /// After minus before, in dB. `None` when a side is too thin to subtract.
    pub sinr_delta_db: Option<f32>,
    pub rsrp_delta_db: Option<f32>,
    /// Median download after minus before, in Mbps, when each side has two reads.
    pub download_delta_mbps: Option<f64>,
    pub class: Class,
    pub confidence: Confidence,
}

/// The whole answer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Comparison {
    /// `{kind}-{at_unix}`, the id the log's own time gives it.
    pub event_id: String,
    pub event_kind: String,
    pub event_at_unix: u64,
    pub event_detail: Option<String>,
    pub window_secs: u64,
    pub smear_ms: u64,
    pub segments: Vec<Segment>,
    /// The weakest segment's confidence: a summary is only as good as its worst part.
    pub confidence: Confidence,
    pub notes: Vec<String>,
    /// What is absent by name, so an empty figure is never read as zero.
    pub missing: Vec<String>,
}

/// Everything the comparison is computed from. Borrowed, so the caller keeps the
/// logs and nothing here can write to them.
pub struct CompareInput<'a> {
    pub event: &'a ObsEvent,
    pub signal: &'a [Row],
    pub speed: &'a [Entry],
    /// Times of the other events in the log, so a neighbour inside the smear can
    /// be named rather than guessed at.
    pub neighbours: &'a [u64],
    pub window_secs: u64,
    /// How wide the measured smear is: two events closer than this may have
    /// happened in either order.
    pub smear_ms: u64,
}

fn cell_label(k: &CellKey) -> String {
    match k {
        CellKey::Carrier { earfcn_dl, pci } => format!("earfcn_dl {earfcn_dl} pci {pci}"),
        CellKey::Unkeyed { band, pci } => format!("band {band} pci {pci} (earfcn unknown)"),
    }
}

/// One side built from its rows and the speed reads that ran inside it.
fn side(rows: &[&Row], speed: &[&Entry], window_secs: u64) -> Side {
    let watched_secs = (rows.len() as u64)
        .saturating_mul(SIGNAL_SAMPLE_SECS)
        .min(window_secs);
    let coverage = if window_secs == 0 {
        0.0
    } else {
        watched_secs as f64 / window_secs as f64
    };
    let sinrs: Vec<f32> = rows.iter().filter_map(|r| r.sinr_db).collect();
    let rsrps: Vec<f32> = rows.iter().filter_map(|r| r.rsrp_dbm).collect();
    let downloads: Vec<f64> = speed.iter().map(|e| e.download_bps / 1_000_000.0).collect();
    Side {
        rows: rows.len(),
        watched_secs,
        coverage,
        sinr_median: median(sinrs.clone()),
        sinr_p10: percentile(&sinrs, 0.10),
        rsrp_median: median(rsrps),
        // One read is a moment, not a rate: a difference needs two on each side.
        download_median_mbps: if downloads.len() >= 2 {
            median_f64(&downloads)
        } else {
            None
        },
        speed_rows: downloads.len(),
    }
}

fn thin(s: &Side) -> bool {
    s.rows < MIN_ROWS || s.coverage < MIN_COVERAGE
}

/// The verdict from two medians against their declared thresholds.
fn class(before: &Side, after: &Side) -> Class {
    let (Some(b), Some(a)) = (before.sinr_median, after.sinr_median) else {
        return Class::Insufficient;
    };
    let ds = a - b;
    let dr = match (before.rsrp_median, after.rsrp_median) {
        (Some(b), Some(a)) => Some(a - b),
        _ => None,
    };
    let sinr_moved = ds.abs() > SINR_NOISE_DB;
    let rsrp_moved = dr.is_some_and(|d| d.abs() > RSRP_NOISE_DB);
    match (sinr_moved, rsrp_moved) {
        (false, false) => Class::WithinNoise,
        _ => {
            let up = (sinr_moved && ds > 0.0) || (rsrp_moved && dr.is_some_and(|d| d > 0.0));
            let down = (sinr_moved && ds < 0.0) || (rsrp_moved && dr.is_some_and(|d| d < 0.0));
            match (up, down) {
                (true, true) => Class::Mixed,
                (true, false) => Class::Improved,
                (false, true) => Class::Degraded,
                (false, false) => Class::WithinNoise,
            }
        }
    }
}

fn confidence(before: &Side, after: &Side) -> Confidence {
    if thin(before) || thin(after) {
        return Confidence::Insufficient;
    }
    // The weaker side and the thinner coverage decide, never the size of the move.
    Confidence::of_rows(before.rows.min(after.rows), MIN_ROWS)
}

/// Compare one cell against itself, before and after the event.
pub fn compare(input: &CompareInput<'_>) -> Comparison {
    let t0 = input.event.at_unix;
    let w = input.window_secs;
    let lo = t0.saturating_sub(w);
    let hi = t0.saturating_add(w);

    let mut rows: Vec<&Row> = input
        .signal
        .iter()
        .filter(|r| r.at_unix >= lo && r.at_unix <= hi && r.at_unix != t0)
        .collect();
    rows.sort_by_key(|r| r.at_unix);

    // Stretches of one cell key, cut wherever the key changes.
    let mut groups: Vec<(Option<CellKey>, Vec<&Row>)> = Vec::new();
    for r in rows {
        let key = key_from_row(r);
        match groups.last_mut() {
            Some((k, v)) if *k == key => v.push(r),
            _ => groups.push((key, vec![r])),
        }
    }

    let mut segments = Vec::new();
    let mut notes = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for (key, group) in groups {
        let label = match &key {
            Some(k) => cell_label(k),
            None => "no cell named".to_string(),
        };
        let before: Vec<&Row> = group.iter().copied().filter(|r| r.at_unix < t0).collect();
        let after: Vec<&Row> = group.iter().copied().filter(|r| r.at_unix > t0).collect();
        // A speed read belongs to the stretch it started in: the stretch names the
        // cell that was serving then, and nothing else links the two logs.
        let before_speed: Vec<&Entry> = match (before.first(), before.last()) {
            (Some(f), Some(l)) => input
                .speed
                .iter()
                .filter(|e| e.started_unix >= f.at_unix && e.started_unix <= l.at_unix)
                .collect(),
            _ => Vec::new(),
        };
        let after_speed: Vec<&Entry> = match (after.first(), after.last()) {
            (Some(f), Some(l)) => input
                .speed
                .iter()
                .filter(|e| e.started_unix >= f.at_unix && e.started_unix <= l.at_unix)
                .collect(),
            _ => Vec::new(),
        };
        let b = side(&before, &before_speed, w);
        let a = side(&after, &after_speed, w);
        let thin_side = thin(&b) || thin(&a);
        let cls = if thin_side {
            Class::Insufficient
        } else {
            class(&b, &a)
        };
        let (sd, rd) = if thin_side {
            (None, None)
        } else {
            (
                match (b.sinr_median, a.sinr_median) {
                    (Some(x), Some(y)) => Some(y - x),
                    _ => None,
                },
                match (b.rsrp_median, a.rsrp_median) {
                    (Some(x), Some(y)) => Some(y - x),
                    _ => None,
                },
            )
        };
        if b.sinr_median.is_none() || a.sinr_median.is_none() {
            let m = "sinr: a side of the window carries no SINR figure".to_string();
            if !missing.contains(&m) {
                missing.push(m);
            }
        }
        segments.push(Segment {
            cell: label,
            confidence: confidence(&b, &a),
            sinr_delta_db: sd,
            rsrp_delta_db: rd,
            download_delta_mbps: match (b.download_median_mbps, a.download_median_mbps) {
                (Some(x), Some(y)) => Some(y - x),
                _ => None,
            },
            class: cls,
            before: b,
            after: a,
        });
    }

    if segments.len() > 1 {
        notes.push(format!(
            "the window is {} stretches by cell key: each is compared with itself, never across cells",
            segments.len()
        ));
    }
    let neighbour = input.neighbours.iter().any(|&t| {
        let d = t.abs_diff(t0) * 1000;
        d > 0 && d <= input.smear_ms
    });
    if neighbour {
        notes.push(format!(
            "another event falls inside the measured smear of {} ms, so which came first is not resolved",
            input.smear_ms
        ));
    }

    let speed_note = "speed: a side has fewer than two reads, so there is no difference";
    for s in &segments {
        if s.before.speed_rows < 2 || s.after.speed_rows < 2 {
            let m = speed_note.to_string();
            if !missing.contains(&m) {
                missing.push(m);
            }
            break;
        }
    }

    let confidence = segments
        .iter()
        .map(|s| s.confidence)
        .min()
        .unwrap_or(Confidence::Insufficient);

    Comparison {
        event_id: format!("{}-{t0}", input.event.kind),
        event_kind: input.event.kind.clone(),
        event_at_unix: t0,
        event_detail: input.event.detail.clone(),
        window_secs: w,
        smear_ms: input.smear_ms,
        segments,
        confidence,
        notes,
        missing,
    }
}

/// The text report: what the window held, what moved, and what stayed unknown.
/// A missing figure prints as a dash, never as a zero.
pub fn summary(c: &Comparison) -> String {
    let mut out = format!(
        "compare {} · {} at {}\n",
        c.event_id,
        c.event_kind,
        crate::signal_history::local_iso(c.event_at_unix)
    );
    out.push_str(&format!(
        "window {} s each side · smear {} ms\n",
        c.window_secs, c.smear_ms
    ));
    if let Some(d) = &c.event_detail {
        out.push_str(&format!("detail: {d}\n"));
    }
    if c.segments.is_empty() {
        out.push_str("no signal row inside the window: nothing to compare\n");
    }
    for (i, s) in c.segments.iter().enumerate() {
        out.push_str(&format!("\nsegment {} · {}\n", i + 1, s.cell));
        for (name, side) in [("before", &s.before), ("after", &s.after)] {
            out.push_str(&format!(
                "  {:<6} {} rows · covered {:.2} · sinr {} (p10 {}) · rsrp {} · download {}\n",
                name,
                side.rows,
                side.coverage,
                db(side.sinr_median),
                db(side.sinr_p10),
                dbm(side.rsrp_median),
                mbps(side.download_median_mbps),
            ));
        }
        out.push_str(&format!(
            "  sinr {} · rsrp {} · download {}\n",
            delta(s.sinr_delta_db, "dB"),
            delta(s.rsrp_delta_db, "dBm"),
            delta_f(s.download_delta_mbps, "Mbps"),
        ));
        out.push_str(&format!(
            "  class: {} · confidence: {}\n",
            s.class.sentence(),
            s.confidence.as_str()
        ));
    }
    if !c.notes.is_empty() {
        out.push_str(&format!("\nnote: {}\n", c.notes.join("\nnote: ")));
    }
    if !c.missing.is_empty() {
        out.push_str(&format!("\nmissing: {}\n", c.missing.join("\nmissing: ")));
    }
    out
}

fn db(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} dB"))
        .unwrap_or_else(|| "-".into())
}

fn dbm(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} dBm"))
        .unwrap_or_else(|| "-".into())
}

fn mbps(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.1} Mbps"))
        .unwrap_or_else(|| "-".into())
}

fn delta(v: Option<f32>, unit: &str) -> String {
    v.map(|x| format!("{x:+.1} {unit}"))
        .unwrap_or_else(|| "-".into())
}

fn delta_f(v: Option<f64>, unit: &str) -> String {
    v.map(|x| format!("{x:+.1} {unit}"))
        .unwrap_or_else(|| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventKind;

    const T0: u64 = 1_700_000_000;

    fn row(at: u64, pci: u32, earfcn: Option<u32>, sinr: f32, rsrp: f32) -> Row {
        Row {
            v: 1,
            at_unix: at,
            net_type: None,
            band: Some("1".into()),
            nr_band: None,
            rsrp_dbm: Some(rsrp),
            rsrq_db: None,
            sinr_db: Some(sinr),
            pci: Some(pci),
            endc: None,
            earfcn_dl: earfcn,
            earfcn_ul: None,
            tm: None,
        }
    }

    /// `n` reads, one a minute, ending `gap` seconds before the event.
    fn before_rows(n: u64, sinr: f32, rsrp: f32, pci: u32, earfcn: Option<u32>) -> Vec<Row> {
        (0..n)
            .map(|i| {
                let at = T0 - 60 - i * 60;
                row(at, pci, earfcn, sinr, rsrp)
            })
            .collect()
    }

    fn after_rows(n: u64, sinr: f32, rsrp: f32, pci: u32, earfcn: Option<u32>) -> Vec<Row> {
        (0..n)
            .map(|i| {
                let at = T0 + 60 + i * 60;
                row(at, pci, earfcn, sinr, rsrp)
            })
            .collect()
    }

    fn event() -> ObsEvent {
        ObsEvent::new(
            EventKind::Cell,
            "signal",
            T0,
            "changed",
            Some("earfcn_dl 100 pci 481 -> earfcn_dl 300 pci 395 held 2".into()),
        )
    }

    fn run(signal: Vec<Row>, window: u64, neighbours: &[u64]) -> Comparison {
        let e = event();
        let input = CompareInput {
            event: &e,
            signal: &signal,
            speed: &[],
            neighbours,
            window_secs: window,
            smear_ms: 2860,
        };
        compare(&input)
    }

    #[test]
    fn a_five_db_step_is_a_verdict() {
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(20, 15.0, -100.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments.len(), 1, "{c:?}");
        let seg = &c.segments[0];
        assert_eq!(seg.class, Class::Improved);
        assert_eq!(seg.sinr_delta_db, Some(5.0));
        assert_eq!(seg.rsrp_delta_db, Some(0.0));
        assert!(seg.confidence >= Confidence::Low, "{c:?}");
    }

    #[test]
    fn a_two_db_step_is_within_noise() {
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(20, 12.0, -100.5, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments[0].class, Class::WithinNoise);
    }

    #[test]
    fn an_rsrp_step_above_its_threshold_is_a_verdict() {
        let mut s = before_rows(20, 10.0, -102.0, 481, Some(100));
        s.extend(after_rows(20, 10.0, -100.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments[0].class, Class::Improved);
        assert_eq!(c.segments[0].rsrp_delta_db, Some(2.0));
    }

    #[test]
    fn a_short_side_is_insufficient() {
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(
            MIN_ROWS as u64 - 1,
            15.0,
            -100.0,
            481,
            Some(100),
        ));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments[0].class, Class::Insufficient);
        assert_eq!(
            c.segments[0].sinr_delta_db, None,
            "no number on a thin side"
        );
        assert_eq!(c.confidence, Confidence::Insufficient);
    }

    #[test]
    fn a_thin_coverage_is_insufficient() {
        // Twelve rows inside a thirty minute window is a fifth of it: enough rows
        // by count, not enough of the window.
        let mut s = before_rows(12, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(12, 15.0, -100.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[]);
        assert!(c.segments[0].before.coverage < MIN_COVERAGE);
        assert_eq!(c.segments[0].class, Class::Insufficient);
    }

    #[test]
    fn a_coverage_of_half_is_enough() {
        // Fifteen rows is 900 seconds, exactly half of the 1800 second window.
        let mut s = before_rows(15, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(15, 15.0, -100.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments[0].before.coverage, MIN_COVERAGE);
        assert_eq!(c.segments[0].class, Class::Improved);
        assert_eq!(c.segments[0].sinr_delta_db, Some(5.0));
    }

    #[test]
    fn a_key_change_splits_the_window() {
        // One cell, then another, then back: three stretches, and the middle one
        // is compared with itself alone, which it cannot be.
        let mut s = before_rows(15, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(15, 15.0, -100.0, 395, Some(300)));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments.len(), 2, "{c:?}");
        assert_eq!(c.segments[0].class, Class::Insufficient, "no before side");
        assert_eq!(c.segments[1].class, Class::Insufficient, "no after side");
        assert!(c.notes.iter().any(|n| n.contains("never across cells")));
        assert_eq!(c.confidence, Confidence::Insufficient);
    }

    #[test]
    fn an_unkeyed_side_is_compared_on_its_own_key() {
        // No downlink ARFCN: the key is band and PCI, and it compares with itself.
        let mut s = before_rows(20, 10.0, -100.0, 481, None);
        s.extend(after_rows(20, 15.0, -100.0, 481, None));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments.len(), 1, "{c:?}");
        assert!(c.segments[0].cell.contains("band 1 pci 481"), "{c:?}");
        assert_eq!(c.segments[0].class, Class::Improved);
    }

    #[test]
    fn a_mixed_step_has_no_single_verdict() {
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(20, 15.0, -104.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[]);
        assert_eq!(c.segments[0].class, Class::Mixed);
    }

    #[test]
    fn one_speed_row_names_missing() {
        let e = event();
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(20, 10.0, -100.0, 481, Some(100)));
        let mut entry = Entry::blank();
        entry.started_unix = T0 + 120;
        entry.ended_unix = T0 + 150;
        entry.elapsed_ms = 30_000;
        entry.download_bps = 50_000_000.0;
        let speed = vec![entry];
        let input = CompareInput {
            event: &e,
            signal: &s,
            speed: &speed,
            neighbours: &[],
            window_secs: WINDOW_SECS,
            smear_ms: 2860,
        };
        let c = compare(&input);
        assert!(c.missing.iter().any(|m| m.starts_with("speed:")), "{c:?}");
        assert_eq!(c.segments[0].download_delta_mbps, None);
    }

    #[test]
    fn a_neighbour_event_inside_the_smear_is_not_ordered() {
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(20, 10.0, -100.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[T0 + 2]);
        assert!(c.notes.iter().any(|n| n.contains("not resolved")), "{c:?}");
    }

    /// No sentence here may name a cause: the comparison says what moved.
    #[test]
    fn no_text_names_a_cause() {
        let mut s = before_rows(20, 10.0, -100.0, 481, Some(100));
        s.extend(after_rows(20, 15.0, -104.0, 481, Some(100)));
        let c = run(s, WINDOW_SECS, &[T0 + 2]);
        let mut all = String::new();
        all.push_str(&c.event_id);
        for n in &c.notes {
            all.push_str(n);
        }
        for m in &c.missing {
            all.push_str(m);
        }
        for seg in &c.segments {
            all.push_str(&seg.cell);
            all.push_str(seg.class.as_str());
            all.push_str(seg.class.sentence());
        }
        for word in ["because", "caused", "due to", "since "] {
            assert!(!all.contains(word), "{word} in {all}");
        }
    }
}
