//! Band advice from evidence, and only from evidence.
//!
//! Three rules shape everything here.
//!
//! 1. The signal log records the band that was *served*, never the set that was
//!    configured. So the log can rank served bands and cannot rank band sets;
//!    set advice comes from `scan` results alone, and the two are never mixed
//!    into one ranking.
//! 2. A reading taken within the measured smear of a band change is not
//!    attributed to either band. This firmware answers signal reads in about
//!    450 ms or about 1500 ms depending on which tail it is in (measured
//!    2026-11-20, max 2860 ms), so two readings closer than that describe one
//!    moment, not two.
//! 3. A figure beside a band is an association, never a cause. Nothing here
//!    switches a band, writes a file, reads the clock or touches the network:
//!    the function takes rows and returns an opinion about them.
//!
//! Confidence is derived from how much evidence exists, not from how strongly
//! the ranking points: the thresholds are named below and a level is only ever
//! reached by meeting one.

use serde::Serialize;

/// Default window: one day of the signal log.
pub const WINDOW_SECS: u64 = 86_400;

/// The fewest rows in the window that can say anything at all.
pub const MIN_ROWS: usize = 30;

/// Rows per band for `high`. Two hours of one-minute rows is 120; a band that
/// held the link that long has been seen in more than one condition.
const HIGH_ROWS: usize = 120;
const MEDIUM_ROWS: usize = 40;

/// Measurements per scanned set for `high` and `medium`.
const HIGH_MEASURE: usize = 5;
const MEDIUM_MEASURE: usize = 3;

/// How much the evidence supports the advice. `Insufficient` is not a failure:
/// it says what is missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    #[default]
    Insufficient,
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Insufficient => "insufficient",
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        }
    }

    pub(crate) fn of_rows(rows: usize, min_rows: usize) -> Confidence {
        if rows >= HIGH_ROWS {
            Confidence::High
        } else if rows >= MEDIUM_ROWS {
            Confidence::Medium
        } else if rows >= min_rows {
            Confidence::Low
        } else {
            Confidence::Insufficient
        }
    }

    fn of_measurements(measured: usize) -> Confidence {
        if measured >= HIGH_MEASURE {
            Confidence::High
        } else if measured >= MEDIUM_MEASURE {
            Confidence::Medium
        } else if measured >= 2 {
            Confidence::Low
        } else {
            Confidence::Insufficient
        }
    }

    pub(crate) fn min(self, other: Confidence) -> Confidence {
        if self <= other {
            self
        } else {
            other
        }
    }
}

/// One served band as the log recorded it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Served {
    /// Bare band number as the log spells it (`3`, not `B3`).
    pub band: String,
    pub rows: usize,
    /// Share of the window's rows, so a reader sees how much of the time it held.
    pub share: f64,
    pub rsrp_median: Option<f32>,
    /// Tenth percentile: the weak end, which is what a bad moment feels like.
    pub rsrp_p10: Option<f32>,
    pub sinr_median: Option<f32>,
    /// Median download of the speed entries measured while this band was served.
    /// An association: the speed test ran beside that band, not because of it.
    pub download_median_mbps: Option<f64>,
    pub measurements: usize,
}

/// One scanned set, from `scan` results only.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Set {
    pub bands: Vec<u8>,
    pub measurements: usize,
    pub download_median_mbps: Option<f64>,
    /// False when the set names a band this firmware does not advertise.
    pub advertised: bool,
}

/// What the evidence supports, and what it does not.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Advice {
    pub window_secs: u64,
    pub rows_used: usize,
    /// The measured smear, or `None` when the window holds fewer than two reads
    /// and no gap can be ranked. `None` is not zero: zero would claim an order.
    pub smear_ms: Option<u64>,
    /// The weaker of the two sections' levels: an advice is only as good as the
    /// part of it that is least supported.
    pub confidence: Confidence,
    pub served_confidence: Confidence,
    pub set_confidence: Confidence,
    /// Rows dropped because they sat inside the smear of a band change.
    pub dropped_in_smear: usize,
    /// Order: most rows first.
    pub served: Vec<Served>,
    /// Order: measured download, fastest first. Unmeasured sets are not listed.
    pub sets: Vec<Set>,
    pub notes: Vec<String>,
    /// What would raise the confidence, phrased as something to run.
    pub missing: Vec<String>,
}

/// Everything the advice is computed from. Borrowed, so the caller keeps
/// ownership of the logs and nothing here can write to them.
pub struct Input<'a> {
    pub signal: &'a [crate::signal_history::Row],
    pub speed: &'a [crate::history::Entry],
    pub scans: &'a [crate::scan::SetResult],
    pub advertised: &'a [u8],
    /// Preset name and bands, so a band no preset offers can be marked.
    pub presets: &'a [(String, Vec<u8>)],
    pub now_unix: u64,
    pub window_secs: u64,
    pub min_rows: usize,
    /// The smear measured over the same window the advice reads, or `None`.
    pub smear_ms: Option<u64>,
}

/// Bare band number. The log says `3`; a caller that hands over `B3` gets `3`.
fn band_number(band: &str) -> Option<u8> {
    let t = band.trim();
    let t = t
        .strip_prefix('B')
        .or_else(|| t.strip_prefix('b'))
        .unwrap_or(t);
    t.parse().ok()
}

pub(crate) fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

pub(crate) fn percentile(values: &[f32], p: f64) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    Some(sorted[idx])
}

pub(crate) fn median_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

/// The advice. Pure: no clock, no file, no network, no write.
pub fn advise(input: &Input) -> Advice {
    let since = input.now_unix.saturating_sub(input.window_secs);
    let in_window: Vec<&crate::signal_history::Row> = input
        .signal
        .iter()
        .filter(|r| r.at_unix >= since && r.at_unix <= input.now_unix)
        .collect();

    // A reading within the smear of a change sits between two bands; attributing
    // it to either one would be inventing a fact the firmware did not report.
    let mut kept: Vec<&crate::signal_history::Row> = Vec::new();
    let mut dropped_in_smear = 0usize;
    let mut prev: Option<&crate::signal_history::Row> = None;
    for row in &in_window {
        if let Some(p) = prev {
            // One definition of order: a reading against a change is open inside
            // the measured smear, and a window without two reads ranks nothing.
            let order = crate::incidents::classify_pair(
                crate::incidents::PairSide::EventAndReading,
                p.at_unix.saturating_mul(1000),
                row.at_unix.saturating_mul(1000),
                input.smear_ms,
            );
            if order == crate::incidents::PairOrder::InsideSmear && row.band != p.band {
                dropped_in_smear += 1;
                prev = Some(row);
                continue;
            }
        }
        kept.push(row);
        prev = Some(row);
    }

    let mut by_band: std::collections::BTreeMap<String, Vec<&crate::signal_history::Row>> =
        std::collections::BTreeMap::new();
    for row in &kept {
        if let Some(band) = row.band.as_deref().and_then(band_number) {
            by_band.entry(band.to_string()).or_default().push(row);
        }
    }

    let mut served: Vec<Served> = Vec::new();
    for (band, rows) in &by_band {
        let rsrp: Vec<f32> = rows.iter().filter_map(|r| r.rsrp_dbm).collect();
        let sinr: Vec<f32> = rows.iter().filter_map(|r| r.sinr_db).collect();
        let numbers: Vec<u8> = vec![band.parse().unwrap_or(0)];
        let (downs, measurements) = speed_for(input.speed, &numbers, since, input.now_unix);
        served.push(Served {
            band: band.clone(),
            rows: rows.len(),
            share: if kept.is_empty() {
                0.0
            } else {
                rows.len() as f64 / kept.len() as f64
            },
            rsrp_median: median(rsrp.clone()),
            rsrp_p10: percentile(&rsrp, 0.10),
            sinr_median: median(sinr),
            download_median_mbps: median_f64(&downs),
            measurements,
        });
    }
    served.sort_by(|a, b| b.rows.cmp(&a.rows).then(a.band.cmp(&b.band)));

    // Set advice is the scan's, and only the scan's. Two scans of the same set
    // are two measurements of it, so the figure is a real median and not a
    // running average dressed as one.
    let mut samples: std::collections::BTreeMap<Vec<u8>, Vec<f64>> =
        std::collections::BTreeMap::new();
    for result in input.scans {
        let crate::scan::Outcome::Measured { down_bps, .. } = &result.outcome else {
            continue;
        };
        samples
            .entry(result.bands.clone())
            .or_default()
            .push(*down_bps / 1_000_000.0);
    }
    let mut sets: Vec<Set> = samples
        .into_iter()
        .map(|(bands, downs)| Set {
            advertised: bands
                .iter()
                .all(|b| input.advertised.is_empty() || input.advertised.contains(b)),
            measurements: downs.len(),
            download_median_mbps: median_f64(&downs),
            bands,
        })
        .collect();
    sets.sort_by(|a, b| {
        b.download_median_mbps
            .partial_cmp(&a.download_median_mbps)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let served_confidence = Confidence::of_rows(kept.len(), input.min_rows);
    let set_confidence = if sets.is_empty() {
        Confidence::Insufficient
    } else {
        sets.iter()
            .map(|s| Confidence::of_measurements(s.measurements))
            .max()
            .unwrap_or(Confidence::Insufficient)
    };
    let mut confidence = served_confidence.min(set_confidence);
    if served_confidence == Confidence::Insufficient && set_confidence == Confidence::Insufficient {
        confidence = Confidence::Insufficient;
    }

    let mut notes: Vec<String> = Vec::new();
    notes.push(
        "the log records the band that was served, not the set that was configured".to_string(),
    );
    match input.smear_ms {
        Some(ms) if ms > 0 => notes.push(format!(
            "a reading within {ms} ms of a band change is not attributed to either band"
        )),
        Some(_) => {}
        None => notes.push(
            "the window holds fewer than two reads, so a reading near a band change cannot be \
             placed inside a measured smear"
                .to_string(),
        ),
    }
    notes.push("a figure beside a band is associated with it, never caused by it".to_string());
    if dropped_in_smear > 0 {
        notes.push(format!(
            "{dropped_in_smear} reading(s) dropped inside the smear"
        ));
    }
    if !input.presets.is_empty() {
        let offered: Vec<String> = served
            .iter()
            .filter(|s| !preset_offers(input.presets, &s.band))
            .map(|s| format!("B{}", s.band))
            .collect();
        if !offered.is_empty() {
            notes.push(format!(
                "no preset offers {}: a switch would need a new preset",
                offered.join(", ")
            ));
        }
    }

    let mut missing: Vec<String> = Vec::new();
    if kept.len() < input.min_rows {
        missing.push(format!(
            "{} rows in the window, {min_rows} needed: let the log run",
            kept.len(),
            min_rows = input.min_rows
        ));
    }
    if sets.is_empty() {
        missing.push(
            "no measured set: set advice needs scan results, for example `routwatch scan --set 1,3 --set 20,28`"
                .to_string(),
        );
    }
    if !input.advertised.is_empty() {
        let unadvertised: Vec<String> = served
            .iter()
            .filter(|s| {
                s.band
                    .parse::<u8>()
                    .map(|b| !input.advertised.contains(&b))
                    .unwrap_or(false)
            })
            .map(|s| format!("B{}", s.band))
            .collect();
        if !unadvertised.is_empty() {
            missing.push(format!(
                "{} was served but is not in the advertised list: the firmware cannot be asked for it",
                unadvertised.join(", ")
            ));
        }
    }
    if confidence < Confidence::High {
        missing.push(format!(
            "more rows per band raises confidence: {} now, {HIGH_ROWS} for high",
            kept.len()
        ));
    }

    Advice {
        window_secs: input.window_secs,
        rows_used: kept.len(),
        smear_ms: input.smear_ms,
        confidence,
        served_confidence,
        set_confidence,
        dropped_in_smear,
        served,
        sets,
        notes,
        missing,
    }
}

// ---------------------------------------------------------------------------
// Drift guard: is the aim still where it was?
//
// A directional antenna does not turn by itself, but its mount can: wind on a
// loose clamp, a slow creep over days. The guard compares today's best SINR on
// one cell with the best SINR that same cell reached on previous complete local
// days, and refuses to say anything before two such days exist. SINR, not RSRP:
// an aim that slipped keeps receiving the tower, and only loses the ratio.
//
// Like `advise`, it takes rows and a clock value and touches nothing: no file,
// no network, no time source of its own.
// ---------------------------------------------------------------------------

/// Where a gauge stops saying "fair": the cut the signal bars are labelled
/// with, declared once so a bar that reads "poor" and a class that reads
/// "limited" cannot disagree on the same number.
pub const RSRP_FAIR_DBM: f32 = -100.0;
pub const SINR_FAIR_DB: f32 = 10.0;

/// How far today's best SINR must sit below the baseline to be called drift.
/// The measured daily spread of one cell's best SINR over 80.55 h was 3 dB, so
/// a 3 dB difference is the weather and not the mount.
pub const GUARD_ALERT_DB: f32 = 4.0;

/// Complete previous days a baseline needs.
pub const GUARD_BASELINE_DAYS: usize = 2;

/// Days behind a medium and a high baseline.
const GUARD_MEDIUM_DAYS: usize = 3;
const GUARD_HIGH_DAYS: usize = 5;

/// Rows a previous day needs to count: one hour at the measured minute cadence.
/// A day that began at 23:40 is a memory of twenty minutes, not of a day.
const GUARD_DAY_ROWS: usize = 60;

/// Today must cover this much before its best is compared with anything.
pub const GUARD_TODAY_SECS: u64 = 6 * 3600;

/// The cell being judged, and the numbers behind its verdict.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CellGuard {
    pub earfcn_dl: Option<u32>,
    pub pci: Option<u32>,
    /// Best SINR on this cell today, so far.
    pub today_best_db: Option<f32>,
    /// Median of the best SINR of each complete previous day.
    pub baseline_db: Option<f32>,
    /// How far today's best sits below the baseline. Positive is worse.
    pub drift_db: Option<f32>,
    /// Complete previous days the baseline was built from.
    pub days: usize,
    pub alert: bool,
    pub confidence: Confidence,
}

/// What the guard can say, and what it is still waiting for.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GuardAdvice {
    pub cell: CellGuard,
    /// PCI changes seen inside the judged earfcn: another cell, not a drift.
    pub pci_changes: usize,
    /// Rows carrying no earfcn or no pci, written before v2. Counted, never
    /// judged: a cell key guessed from a legacy row would be an invention.
    pub legacy_rows: usize,
    /// True while the evidence is still being collected: no verdict yet.
    pub collecting: bool,
    pub notes: Vec<String>,
    pub missing: Vec<String>,
}

/// The local calendar day a row belongs to, as `YYYY-MM-DD`, through the same
/// libc path `signal_history::local_stamp` uses.
fn local_day(unix: u64) -> String {
    let stamp = crate::signal_history::local_stamp(unix);
    stamp.get(0..10).unwrap_or("").to_string()
}

fn best_sinr(rows: &[&crate::signal_history::Row]) -> Option<f32> {
    rows.iter().filter_map(|r| r.sinr_db).fold(None, |acc, v| {
        Some(match acc {
            Some(a) => a.max(v),
            None => v,
        })
    })
}

/// The seconds a day was actually read: each distinct sample slot counted once.
/// A span counts an outage as covered, which is how a day with two reads at the
/// two ends of seven hours looked like a full day.
pub fn sampled_secs(rows: &[&crate::signal_history::Row]) -> u64 {
    let width = crate::signal_history::SIGNAL_SAMPLE_SECS;
    let mut slots: Vec<u64> = rows.iter().map(|r| r.at_unix / width).collect();
    slots.sort_unstable();
    slots.dedup();
    slots.len() as u64 * width
}

/// Judge one cell against its own past. Pure: rows in, opinion out.
pub fn drift_guard(rows: &[crate::signal_history::Row], now_unix: u64) -> GuardAdvice {
    let mut out = GuardAdvice::default();
    let in_past: Vec<&crate::signal_history::Row> =
        rows.iter().filter(|r| r.at_unix <= now_unix).collect();
    let keyed: Vec<&crate::signal_history::Row> = in_past
        .iter()
        .copied()
        .filter(|r| r.earfcn_dl.is_some() && r.pci.is_some())
        .collect();
    out.legacy_rows = in_past.len() - keyed.len();
    if out.legacy_rows > 0 {
        out.notes.push(format!(
            "{} row(s) carry no earfcn or pci: written before v2, not judged",
            out.legacy_rows
        ));
    }
    let Some(newest) = keyed.last() else {
        out.missing.push(
            "no row with an earfcn and a pci: the guard needs v2 rows, and none exist yet"
                .to_string(),
        );
        return out;
    };
    let earfcn = newest.earfcn_dl;
    let pci = newest.pci;
    out.cell.earfcn_dl = earfcn;
    out.cell.pci = pci;

    let same_earfcn: Vec<&crate::signal_history::Row> = keyed
        .iter()
        .copied()
        .filter(|r| r.earfcn_dl == earfcn)
        .collect();
    let mut changes = 0usize;
    let mut last: Option<u32> = None;
    let mut pcis: Vec<u32> = Vec::new();
    for r in &same_earfcn {
        let Some(p) = r.pci else { continue };
        match last {
            Some(prev) if prev != p => {
                changes += 1;
                pcis.push(p);
            }
            None => pcis.push(p),
            _ => {}
        }
        last = Some(p);
    }
    out.pci_changes = changes;
    if changes > 0 {
        out.notes.push(format!(
            "pci changed inside earfcn {}: {}: another cell, and its rows are not part of this cell's drift",
            earfcn.map(|e| e.to_string()).unwrap_or_default(),
            pcis.iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(" then ")
        ));
    }

    // Everything below is the judged cell only: same earfcn, same pci.
    let cell: Vec<&crate::signal_history::Row> = same_earfcn
        .iter()
        .copied()
        .filter(|r| r.pci == pci)
        .collect();
    let today = local_day(now_unix);
    let today_rows: Vec<&crate::signal_history::Row> = cell
        .iter()
        .copied()
        .filter(|r| local_day(r.at_unix) == today)
        .collect();
    if today_rows.is_empty() {
        out.missing.push(
            "no row for this cell today: the guard judges today against previous days".to_string(),
        );
        out.collecting = true;
        return out;
    }
    out.cell.today_best_db = best_sinr(&today_rows);
    let first = today_rows
        .iter()
        .map(|r| r.at_unix)
        .min()
        .unwrap_or(now_unix);
    let last_at = today_rows
        .iter()
        .map(|r| r.at_unix)
        .max()
        .unwrap_or(now_unix);
    let span = last_at.saturating_sub(first) + crate::signal_history::SIGNAL_SAMPLE_SECS;
    let covered = sampled_secs(&today_rows);
    out.notes.push(format!(
        "today: {:.1} h spanned, {:.1} h sampled on this cell",
        span as f64 / 3600.0,
        covered as f64 / 3600.0
    ));
    if covered < GUARD_TODAY_SECS {
        out.collecting = true;
        out.missing.push(format!(
            "today spans {:.1} h, sampled {:.1} h, {} h needed before a verdict",
            span as f64 / 3600.0,
            covered as f64 / 3600.0,
            GUARD_TODAY_SECS / 3600
        ));
        return out;
    }
    let Some(today_best) = out.cell.today_best_db else {
        out.collecting = true;
        out.missing.push(
            "no SINR figure in today's rows for this cell: the guard compares SINR, not RSRP"
                .to_string(),
        );
        return out;
    };

    let mut by_day: std::collections::BTreeMap<String, Vec<&crate::signal_history::Row>> =
        std::collections::BTreeMap::new();
    for r in &cell {
        by_day.entry(local_day(r.at_unix)).or_default().push(r);
    }
    let mut bests: Vec<f32> = Vec::new();
    let mut skipped = 0usize;
    for (day, day_rows) in &by_day {
        if *day == today {
            continue;
        }
        if day_rows.len() < GUARD_DAY_ROWS {
            skipped += 1;
            continue;
        }
        if let Some(best) = best_sinr(day_rows) {
            bests.push(best);
        }
    }
    if skipped > 0 {
        out.notes.push(format!(
            "{} previous day(s) had fewer than {GUARD_DAY_ROWS} rows and did not enter the baseline",
            skipped
        ));
    }
    out.cell.days = bests.len();
    if bests.len() < GUARD_BASELINE_DAYS {
        out.collecting = true;
        out.missing.push(format!(
            "baseline needs {GUARD_BASELINE_DAYS} complete previous days on this cell, {} so far",
            bests.len()
        ));
        return out;
    }
    let Some(baseline) = median(bests) else {
        out.collecting = true;
        return out;
    };
    out.cell.baseline_db = Some(baseline);
    out.cell.drift_db = Some(baseline - today_best);
    out.cell.alert = out.cell.drift_db.unwrap_or(0.0) >= GUARD_ALERT_DB;
    out.cell.confidence = if out.cell.days >= GUARD_HIGH_DAYS {
        Confidence::High
    } else if out.cell.days >= GUARD_MEDIUM_DAYS {
        Confidence::Medium
    } else {
        Confidence::Low
    };
    if out.cell.alert {
        out.notes.push(format!(
            "cell {}/{}: today's best {today_best:.1} dB is {:.1} dB below the {baseline:.1} dB baseline of {} day(s)",
            earfcn.map(|e| e.to_string()).unwrap_or_default(),
            pci.map(|p| p.to_string()).unwrap_or_default(),
            out.cell.drift_db.unwrap_or(0.0),
            out.cell.days
        ));
    }
    out
}

fn preset_offers(presets: &[(String, Vec<u8>)], band: &str) -> bool {
    let Ok(n) = band.parse::<u8>() else {
        return false;
    };
    presets.iter().any(|(_, bands)| bands.contains(&n))
}

/// Download figures of the speed entries measured while one of `bands` was
/// served, inside the window. `band_at_start` matters: a test that began on one
/// band and ended on another measured neither cleanly, so only entries that
/// started and ended on the same served band count.
fn speed_for(
    entries: &[crate::history::Entry],
    bands: &[u8],
    since: u64,
    until: u64,
) -> (Vec<f64>, usize) {
    let mut downs = Vec::new();
    for e in entries {
        if e.started_unix < since || e.started_unix > until {
            continue;
        }
        if e.band_changed {
            continue;
        }
        let served = e.band.as_deref().and_then(band_number);
        if let Some(b) = served {
            if bands.contains(&b) {
                downs.push(e.download_bps / 1_000_000.0);
            }
        }
    }
    let n = downs.len();
    (downs, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(at: u64, band: &str, rsrp: f32) -> crate::signal_history::Row {
        crate::signal_history::Row {
            v: crate::signal_history::VERSION,
            at_unix: at,
            net_type: None,
            band: Some(band.to_string()),
            nr_band: None,
            rsrp_dbm: Some(rsrp),
            rsrq_db: None,
            sinr_db: Some(10.0),
            pci: None,
            endc: None,
            earfcn_dl: None,
            earfcn_ul: None,
            tm: None,
        }
    }

    fn input<'a>(rows: &'a [crate::signal_history::Row], smear_ms: u64) -> Input<'a> {
        Input {
            signal: rows,
            speed: &[],
            scans: &[],
            advertised: &[],
            presets: &[],
            now_unix: 10_000,
            window_secs: 3_600,
            min_rows: 3,
            smear_ms: Some(smear_ms),
        }
    }

    fn input_without_a_smear<'a>(rows: &'a [crate::signal_history::Row]) -> Input<'a> {
        let mut input = input(rows, 0);
        input.smear_ms = None;
        input
    }

    #[test]
    fn a_reading_inside_the_smear_of_a_change_is_dropped() {
        // 2860 ms of smear: the row one second after the switch is not attributed.
        let rows = vec![
            row(9_900, "3", -100.0),
            row(9_901, "1", -90.0),
            row(9_950, "1", -92.0),
        ];
        let advice = advise(&input(&rows, 2_860));
        assert_eq!(advice.dropped_in_smear, 1);
        assert_eq!(advice.rows_used, 2);
        assert!(
            advice.served.iter().all(|s| s.band != "1" || s.rows == 1),
            "the dropped reading must not count toward B1"
        );
    }

    /// A window with fewer than two reads has no smear: the advice must not
    /// print "within 0 ms", which is a claim about a measurement that was never
    /// made.
    #[test]
    fn a_smear_of_zero_prints_no_window_note() {
        let rows = vec![
            row(9_900, "3", -100.0),
            row(9_950, "3", -99.0),
            row(9_990, "3", -98.0),
        ];
        let advice = advise(&input(&rows, 0));
        assert!(
            !advice
                .notes
                .iter()
                .any(|n| n.contains(" ms of a band change")),
            "{:?}",
            advice.notes
        );
    }

    /// `None` is not zero: with no rankable gap the advice says so instead of
    /// printing a number, and it drops nothing.
    #[test]
    fn a_window_without_two_reads_names_no_smear() {
        let rows = vec![row(9_900, "3", -100.0), row(9_901, "1", -90.0)];
        let advice = advise(&input_without_a_smear(&rows));
        assert_eq!(advice.smear_ms, None);
        assert_eq!(advice.dropped_in_smear, 0);
        assert!(
            advice
                .notes
                .iter()
                .any(|n| n.contains("fewer than two reads")),
            "{:?}",
            advice.notes
        );
        assert!(
            !advice.notes.iter().any(|n| n.contains("0 ms")),
            "no invented zero: {:?}",
            advice.notes
        );
    }

    #[test]
    fn a_band_spelled_with_a_prefix_reads_as_a_number() {
        assert_eq!(band_number("B3"), Some(3));
        assert_eq!(band_number("3"), Some(3));
        assert_eq!(band_number("n78"), None);
    }

    #[test]
    fn confidence_follows_the_evidence_count() {
        assert_eq!(Confidence::of_rows(10, 30), Confidence::Insufficient);
        assert_eq!(Confidence::of_rows(30, 30), Confidence::Low);
        assert_eq!(Confidence::of_rows(40, 30), Confidence::Medium);
        assert_eq!(Confidence::of_rows(120, 30), Confidence::High);
        assert_eq!(Confidence::of_measurements(1), Confidence::Insufficient);
        assert_eq!(Confidence::of_measurements(2), Confidence::Low);
        assert_eq!(Confidence::of_measurements(3), Confidence::Medium);
        assert_eq!(Confidence::of_measurements(5), Confidence::High);
    }

    #[test]
    fn nothing_is_recommended_that_the_scan_did_not_measure() {
        let rows = vec![
            row(100, "3", -100.0),
            row(200, "3", -99.0),
            row(300, "3", -98.0),
        ];
        let advice = advise(&input(&rows, 2_860));
        assert!(advice.sets.is_empty(), "the log alone ranks no set");
        assert_eq!(advice.set_confidence, Confidence::Insufficient);
        assert!(
            advice.missing.iter().any(|m| m.contains("routwatch scan")),
            "the missing evidence is named with the command that would supply it"
        );
    }

    // ------------------------------------------------------------ drift guard

    /// The first second of the local day `unix` sits in, minute precise. The
    /// tests build their days from this, so they hold in any time zone.
    fn start_of_day(unix: u64) -> u64 {
        let day = local_day(unix);
        let mut t = unix;
        while t >= 3_600 && local_day(t - 3_600) == day {
            t -= 3_600;
        }
        while t >= 60 && local_day(t - 60) == day {
            t -= 60;
        }
        t
    }

    fn keyed(at: u64, earfcn: u32, pci: u32, sinr: f32) -> crate::signal_history::Row {
        let mut r = row(at, "3", -95.0);
        r.sinr_db = Some(sinr);
        r.pci = Some(pci);
        r.earfcn_dl = Some(earfcn);
        r.earfcn_ul = Some(earfcn + 18_000);
        r
    }

    /// Rows of one cell on one local day, starting a minute after its midnight.
    fn day(
        start: u64,
        count: usize,
        sinr: f32,
        earfcn: u32,
        pci: u32,
    ) -> Vec<crate::signal_history::Row> {
        (0..count)
            .map(|i| keyed(start + 60 + i as u64 * 60, earfcn, pci, sinr))
            .collect()
    }

    const EARFCN: u32 = 1_650;
    const PCI: u32 = 481;

    /// Two complete previous days at 9 dB, plus today starting at `today_best`.
    fn three_days(today_best: f32, today_rows: usize) -> (Vec<crate::signal_history::Row>, u64) {
        let anchor = 1_700_000_000;
        let today_start = start_of_day(anchor);
        let yesterday = start_of_day(today_start - 60);
        let before = start_of_day(yesterday - 60);
        let mut rows = day(before, 360, 9.0, EARFCN, PCI);
        rows.extend(day(yesterday, 360, 9.0, EARFCN, PCI));
        rows.extend(day(today_start, today_rows, today_best, EARFCN, PCI));
        let now = today_start + 6 * 3_600 + 120;
        (rows, now)
    }

    #[test]
    fn four_db_below_the_baseline_alerts_and_three_does_not() {
        let (rows, now) = three_days(5.0, 361);
        let g = drift_guard(&rows, now);
        assert!(!g.collecting, "{:?}", g);
        assert_eq!(g.cell.baseline_db, Some(9.0));
        assert_eq!(g.cell.today_best_db, Some(5.0));
        assert!(g.cell.alert, "4 dB is drift: {:?}", g.cell);

        let (rows, now) = three_days(6.0, 361);
        let g = drift_guard(&rows, now);
        assert!(
            !g.cell.alert,
            "3 dB is the daily spread, not the mount: {:?}",
            g.cell
        );
        assert_eq!(g.cell.drift_db, Some(3.0));
    }

    /// Two reads at the two ends of seven hours are two minutes of samples, not
    /// a day of coverage.
    #[test]
    fn two_reads_at_the_ends_of_a_span_do_not_cover_it() {
        let anchor = 1_700_000_000;
        let today_start = start_of_day(anchor);
        let yesterday = start_of_day(today_start - 60);
        let before = start_of_day(yesterday - 60);
        let mut rows = day(before, 360, 9.0, EARFCN, PCI);
        rows.extend(day(yesterday, 360, 9.0, EARFCN, PCI));
        rows.push(keyed(today_start + 60, EARFCN, PCI, 8.0));
        rows.push(keyed(today_start + 60 + 7 * 3_600, EARFCN, PCI, 8.0));
        let now = today_start + 7 * 3_600 + 120;
        let g = drift_guard(&rows, now);
        assert!(g.collecting, "two reads cannot judge a day: {:?}", g);
        assert!(
            g.missing.iter().any(|m| m.contains("sampled 0.0 h")),
            "the coverage must be the sampled minutes: {:?}",
            g.missing
        );
        assert!(
            g.notes.iter().any(|n| n.contains("7.0 h spanned")),
            "the span is still named, beside the samples: {:?}",
            g.notes
        );
    }

    /// The same minute read twice is one minute of coverage.
    #[test]
    fn a_minute_read_twice_is_one_minute() {
        let anchor = 1_700_000_000;
        let today_start = start_of_day(anchor);
        let yesterday = start_of_day(today_start - 60);
        let before = start_of_day(yesterday - 60);
        let mut rows = day(before, 360, 9.0, EARFCN, PCI);
        rows.extend(day(yesterday, 360, 9.0, EARFCN, PCI));
        for m in 0..360 {
            let at = today_start + 60 + m * 60;
            rows.push(keyed(at, EARFCN, PCI, 6.0));
            rows.push(keyed(at + 1, EARFCN, PCI, 6.0));
        }
        let now = today_start + 6 * 3_600 + 200;
        let g = drift_guard(&rows, now);
        assert!(
            !g.collecting,
            "six hours of minutes, each read twice: {:?}",
            g
        );
        assert!(
            g.notes.iter().any(|n| n.contains("6.0 h sampled")),
            "{:?}",
            g.notes
        );
    }

    /// The span comes from the extreme stamps, never from the order of the file.
    #[test]
    fn an_out_of_order_day_still_has_a_span() {
        let anchor = 1_700_000_000;
        let today_start = start_of_day(anchor);
        let yesterday = start_of_day(today_start - 60);
        let before = start_of_day(yesterday - 60);
        let mut rows = day(before, 360, 9.0, EARFCN, PCI);
        rows.extend(day(yesterday, 360, 9.0, EARFCN, PCI));
        let mut today = day(today_start, 120, 6.0, EARFCN, PCI);
        today.reverse();
        rows.extend(today);
        let now = today_start + 6 * 3_600 + 120;
        let g = drift_guard(&rows, now);
        assert!(
            g.notes.iter().any(|n| n.contains("2.0 h spanned")),
            "reversed rows must not read as a zero span: {:?}",
            g.notes
        );
    }

    #[test]
    fn a_short_today_is_missing_not_an_alarm() {
        let (rows, now) = three_days(1.0, 120);
        let g = drift_guard(&rows, now);
        assert!(g.collecting);
        assert!(!g.cell.alert, "two hours cannot accuse the mount");
        assert!(
            g.missing.iter().any(|m| m.contains("6 h needed")),
            "{:?}",
            g.missing
        );
    }

    #[test]
    fn one_previous_day_is_not_a_baseline() {
        let anchor = 1_700_000_000;
        let today_start = start_of_day(anchor);
        let yesterday = start_of_day(today_start - 60);
        let mut rows = day(yesterday, 360, 9.0, EARFCN, PCI);
        rows.extend(day(today_start, 361, 9.0, EARFCN, PCI));
        let now = today_start + 6 * 3_600 + 120;
        let g = drift_guard(&rows, now);
        assert!(g.collecting);
        assert_eq!(g.cell.days, 1);
        assert!(!g.cell.alert);
        assert!(
            g.missing
                .iter()
                .any(|m| m.contains("2 complete previous days")),
            "{:?}",
            g.missing
        );
    }

    #[test]
    fn a_thin_previous_day_does_not_enter_the_baseline() {
        let anchor = 1_700_000_000;
        let today_start = start_of_day(anchor);
        let yesterday = start_of_day(today_start - 60);
        let before = start_of_day(yesterday - 60);
        let mut rows = day(before, 10, 20.0, EARFCN, PCI);
        rows.extend(day(yesterday, 360, 9.0, EARFCN, PCI));
        rows.extend(day(today_start, 361, 9.0, EARFCN, PCI));
        let now = today_start + 6 * 3_600 + 120;
        let g = drift_guard(&rows, now);
        assert_eq!(g.cell.days, 1, "twenty minutes is not a day");
        assert!(g.collecting);
        assert!(
            g.notes.iter().any(|n| n.contains("fewer than 60 rows")),
            "{:?}",
            g.notes
        );
    }

    #[test]
    fn a_pci_change_inside_one_earfcn_is_another_cell_not_a_drift() {
        let (mut rows, now) = three_days(5.0, 361);
        // The newest sample moved to another cell on the same earfcn. That is a
        // handover, and its 2 dB is compared with that cell's past, not this one.
        let today_start = start_of_day(1_700_000_000);
        rows.extend(day(today_start, 361, 2.0, EARFCN, 395));
        let g = drift_guard(&rows, now);
        assert_eq!(g.cell.pci, Some(395));
        assert_eq!(g.pci_changes, 1);
        assert!(
            g.notes.iter().any(|n| n.contains("pci changed")),
            "{:?}",
            g.notes
        );
        assert!(
            !g.cell.alert,
            "a new cell has no baseline yet: {:?}",
            g.cell
        );
        assert!(g.collecting);
    }

    #[test]
    fn legacy_rows_are_counted_and_never_judged() {
        let rows = vec![row(1_000, "3", -95.0), row(1_060, "3", -94.0)];
        let g = drift_guard(&rows, 2_000);
        assert_eq!(g.legacy_rows, 2);
        assert_eq!(g.cell.earfcn_dl, None);
        assert!(!g.cell.alert);
        assert!(
            g.notes.iter().any(|n| n.contains("before v2")),
            "{:?}",
            g.notes
        );
        assert!(
            g.missing.iter().any(|m| m.contains("v2 rows")),
            "{:?}",
            g.missing
        );
    }
}
