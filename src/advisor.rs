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

    fn of_rows(rows: usize, min_rows: usize) -> Confidence {
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

    fn min(self, other: Confidence) -> Confidence {
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
    pub smear_ms: u64,
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
    pub smear_ms: u64,
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

fn median(mut values: Vec<f32>) -> Option<f32> {
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

fn percentile(values: &[f32], p: f64) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    Some(sorted[idx])
}

fn median_f64(values: &[f64]) -> Option<f64> {
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
    let smear_secs = input.smear_ms as f64 / 1000.0;
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
            let gap = row.at_unix.saturating_sub(p.at_unix) as f64;
            if gap > 0.0 && gap < smear_secs && row.band != p.band {
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
    notes.push(format!(
        "a reading within {} ms of a band change is not attributed to either band",
        input.smear_ms
    ));
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
            smear_ms,
        }
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
}
