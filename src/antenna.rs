//! Whether another antenna would change what this cell gives, read from the
//! signal log. The judgement is about the radio link, not about shopping: the
//! numbers that move with an antenna are RSRP (how much arrives) and SINR (how
//! clean it arrives). RSRQ is cell load, so no aim and no antenna changes it.
//!
//! Reference for the class vocabulary: the gauges' own quality bands
//! (`quality_label` in `src/ui/tabs.rs`), so a bar that says "poor" and a verdict
//! that says "limited" never disagree on the same number.
//!
//! Throughput is reported, never used to classify. Measured 2026-11-22 on an
//! H122-373: three speed runs at one position and constant RSRP -96.0 dB moved
//! x1.69, so one speed row cannot place a lever.

use crate::advisor::{median, median_f64, percentile, Confidence, MIN_ROWS};
use crate::history::Entry;
use crate::signal_history::Row;

/// The default window: one day, the same span the advisor judges.
pub const WINDOW_SECS: u64 = 86_400;

/// What the link, not the antenna, decides. Each class names the one lever it
/// implies, and never a purchase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignalClass {
    /// Too few rows, or a metric the log does not carry, to say anything.
    #[default]
    Insufficient,
    /// RSRP alone is poor: more gain, or a higher mount, is the lever.
    SignalLimited,
    /// Energy arrives (RSRP fair or good) and quality does not (SINR poor):
    /// rejection is the lever, which is what a directional pattern gives.
    InterferenceLimited,
    /// RSRP and SINR are both fair or better: the radio link is not the limit,
    /// so an antenna change buys nothing measurable here.
    NotSignalLimited,
}

impl SignalClass {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalClass::Insufficient => "insufficient",
            SignalClass::SignalLimited => "signal-limited",
            SignalClass::InterferenceLimited => "interference-limited",
            SignalClass::NotSignalLimited => "not-signal-limited",
        }
    }

    /// The lever the class points at, in plain words, with no buying advice.
    pub fn lever(self) -> &'static str {
        match self {
            SignalClass::Insufficient => "no reading yet",
            SignalClass::SignalLimited => "more gain or a higher mount",
            SignalClass::InterferenceLimited => "a pattern that rejects, not a bigger one",
            SignalClass::NotSignalLimited => "nothing on the radio side",
        }
    }
}

/// One cell's reading, over the judged window.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CellSignal {
    /// The cell key as the log carries it (v2 rows only).
    pub earfcn_dl: Option<u32>,
    pub pci: Option<u32>,
    pub band: Option<String>,
    pub rows: usize,
    pub rsrp_median: Option<f32>,
    pub rsrq_median: Option<f32>,
    pub sinr_median: Option<f32>,
    pub sinr_p10: Option<f32>,
    pub class: SignalClass,
    pub confidence: Confidence,
}

/// The answer to "is an antenna worth it here", one class per cell plus what the
/// judgement could not read.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct WorthReport {
    pub window_secs: u64,
    pub rows: usize,
    pub cells: Vec<CellSignal>,
    /// Speed rows inside the window. Reported, never used to classify.
    pub speed_rows: usize,
    pub download_median_mbps: Option<f64>,
    pub notes: Vec<String>,
    pub missing: Vec<String>,
}

impl WorthReport {
    /// The class of the cell with most rows, which is the one serving the link.
    pub fn leading(&self) -> Option<&CellSignal> {
        self.cells.iter().max_by_key(|c| c.rows)
    }
}

/// Classify one cell from its medians. The order matters: a weak signal makes
/// SINR poor for a reason an antenna's pattern cannot fix, so RSRP is judged
/// first.
fn classify(rows: usize, rsrp: Option<f32>, sinr: Option<f32>) -> SignalClass {
    if rows < MIN_ROWS {
        return SignalClass::Insufficient;
    }
    let (Some(rsrp), Some(sinr)) = (rsrp, sinr) else {
        return SignalClass::Insufficient;
    };
    // The gauge's own cut, read from the one constant it is labelled with.
    if rsrp < crate::advisor::RSRP_FAIR_DBM {
        return SignalClass::SignalLimited;
    }
    // RSRQ stays out of the verdict: cell load moves it, so an antenna cannot.
    if sinr < crate::advisor::SINR_FAIR_DB {
        return SignalClass::InterferenceLimited;
    }
    SignalClass::NotSignalLimited
}

/// Judge the window: which cell serves, how it reads, and what that means.
pub fn antenna_worth(
    signal: &[Row],
    speed: &[Entry],
    now_unix: u64,
    window_secs: u64,
) -> WorthReport {
    let since = now_unix.saturating_sub(window_secs.max(60));
    let in_window: Vec<&Row> = signal
        .iter()
        .filter(|r| r.at_unix >= since && r.at_unix <= now_unix)
        .collect();

    let mut report = WorthReport {
        window_secs: window_secs.max(60),
        rows: in_window.len(),
        ..WorthReport::default()
    };

    // Group by the cell key. A row without an earfcn cannot be placed on a cell,
    // so it is counted and excluded rather than pooled into a fake one.
    let mut keyless = 0usize;
    let mut groups: std::collections::BTreeMap<(u32, u32), Vec<&Row>> = Default::default();
    for r in &in_window {
        match (r.earfcn_dl, r.pci) {
            (Some(earfcn), Some(pci)) => groups.entry((earfcn, pci)).or_default().push(r),
            _ => keyless += 1,
        }
    }
    if keyless > 0 {
        report.notes.push(format!(
            "{keyless} row(s) carry no earfcn or no pci and are not placed on a cell"
        ));
    }

    for ((earfcn, pci), rows) in &groups {
        let rsrp = median(rows.iter().filter_map(|r| r.rsrp_dbm).collect());
        let rsrq = median(rows.iter().filter_map(|r| r.rsrq_db).collect());
        let sinr = median(rows.iter().filter_map(|r| r.sinr_db).collect());
        let sinr_p10 = percentile(
            &rows.iter().filter_map(|r| r.sinr_db).collect::<Vec<f32>>(),
            0.10,
        );
        let class = classify(rows.len(), rsrp, sinr);
        // A reading is as good as its rows: 30 rows over a day is an hour.
        let confidence = match rows.len() {
            n if n >= 240 => Confidence::High,
            n if n >= 60 => Confidence::Medium,
            n if n >= MIN_ROWS => Confidence::Low,
            _ => Confidence::Insufficient,
        };
        report.cells.push(CellSignal {
            earfcn_dl: Some(*earfcn),
            pci: Some(*pci),
            band: rows.iter().find_map(|r| r.band.clone()),
            rows: rows.len(),
            rsrp_median: rsrp,
            rsrq_median: rsrq,
            sinr_median: sinr,
            sinr_p10,
            class,
            confidence,
        });
    }
    report.cells.sort_by_key(|c| std::cmp::Reverse(c.rows));

    if report.cells.is_empty() {
        report.missing.push(
            "no row with an earfcn and a pci in the window: only v2 rows can be placed on a cell"
                .to_string(),
        );
    }

    let speed_in: Vec<&Entry> = speed
        .iter()
        .filter(|e| e.ended_unix >= since && e.ended_unix <= now_unix)
        .collect();
    report.speed_rows = speed_in.len();
    let downs: Vec<f64> = speed_in
        .iter()
        .map(|e| e.download_bps / 1_000_000.0)
        .collect();
    report.download_median_mbps = median_f64(&downs);
    if report.speed_rows == 0 {
        report.missing.push(
            "no saved speed run in the window: the class is read from the radio only".to_string(),
        );
    } else {
        report.notes.push(format!(
            "{} speed run(s) in the window: shown beside the class, not used to classify",
            report.speed_rows
        ));
    }
    report
}

/// The human table: one row per cell, then what the reading means.
pub fn table(report: &WorthReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "antenna worth · last {} h · {} signal row(s) · {} speed row(s)\n",
        report.window_secs / 3600,
        report.rows,
        report.speed_rows
    ));
    if report.cells.is_empty() {
        out.push_str("  no cell to judge\n");
    } else {
        out.push_str(&format!(
            "  {:<8} {:<6} {:<4} {:>6} {:>9} {:>7} {:>7} {:>6}  {:<20}\n",
            "earfcn", "pci", "band", "rows", "rsrp dBm", "rsrq dB", "sinr dB", "p10", "class"
        ));
        for c in &report.cells {
            out.push_str(&format!(
                "  {:<8} {:<6} {:<4} {:>6} {:>9} {:>7} {:>7} {:>6}  {:<20}\n",
                c.earfcn_dl
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                c.pci.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                c.band.clone().unwrap_or_else(|| "-".into()),
                c.rows,
                fmt1(c.rsrp_median),
                fmt1(c.rsrq_median),
                fmt1(c.sinr_median),
                fmt1(c.sinr_p10),
                c.class.as_str(),
            ));
        }
        if let Some(c) = report.leading() {
            out.push_str(&format!("\n  lever: {}\n", c.class.lever()));
            out.push_str(&format!("  confidence: {}\n", c.confidence.as_str()));
        }
    }
    if let Some(d) = report.download_median_mbps {
        out.push_str(&format!("  median download in the window: {d:.1} Mbps\n"));
    }
    for n in &report.notes {
        out.push_str(&format!("  note: {n}\n"));
    }
    for m in &report.missing {
        out.push_str(&format!("  missing: {m}\n"));
    }
    out
}

fn fmt1(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1}"))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const EARFCN: u32 = 1650;
    const PCI: u32 = 481;

    fn row(at: u64, rsrp: f32, rsrq: f32, sinr: f32) -> Row {
        Row {
            v: crate::signal_history::VERSION,
            at_unix: at,
            net_type: Some("LTE".into()),
            band: Some("B1".into()),
            nr_band: None,
            rsrp_dbm: Some(rsrp),
            rsrq_db: Some(rsrq),
            sinr_db: Some(sinr),
            pci: Some(PCI),
            endc: Some(true),
            earfcn_dl: Some(EARFCN),
            earfcn_ul: Some(18100),
            tm: Some("TM[4]".into()),
        }
    }

    fn rows_of(n: usize, rsrp: f32, rsrq: f32, sinr: f32) -> Vec<Row> {
        (0..n)
            .map(|i| row(NOW - 60 * (n as u64 - i as u64), rsrp, rsrq, sinr))
            .collect()
    }

    #[test]
    fn a_weak_signal_is_the_lever_not_the_pattern() {
        let r = antenna_worth(&rows_of(120, -108.0, -14.0, 2.0), &[], NOW, WINDOW_SECS);
        assert_eq!(r.leading().unwrap().class, SignalClass::SignalLimited);
        assert_eq!(
            r.leading().unwrap().class.lever(),
            "more gain or a higher mount"
        );
    }

    #[test]
    fn a_strong_signal_with_poor_quality_is_the_pattern() {
        let r = antenna_worth(&rows_of(120, -92.0, -15.0, 3.0), &[], NOW, WINDOW_SECS);
        assert_eq!(r.leading().unwrap().class, SignalClass::InterferenceLimited);
    }

    #[test]
    fn the_class_reads_the_same_cut_as_the_gauge_label() {
        // One dB either side of a cut: the bar's word and the class move together,
        // because both read the constant.
        assert_eq!(
            classify(100, Some(-100.5), Some(5.0)),
            SignalClass::SignalLimited
        );
        assert_eq!(crate::ui::tabs::quality_label("RSRP", Some(-100.5)), "poor");
        assert_eq!(crate::ui::tabs::quality_label("RSRP", Some(-99.5)), "fair");
        assert_eq!(
            classify(100, Some(-90.0), Some(9.5)),
            SignalClass::InterferenceLimited
        );
        assert_eq!(crate::ui::tabs::quality_label("SINR", Some(9.5)), "poor");
        assert_eq!(crate::ui::tabs::quality_label("SINR", Some(10.5)), "fair");
    }

    #[test]
    fn a_good_link_is_not_signal_limited_and_says_so() {
        let r = antenna_worth(&rows_of(120, -78.0, -9.0, 22.0), &[], NOW, WINDOW_SECS);
        let c = r.leading().unwrap();
        assert_eq!(c.class, SignalClass::NotSignalLimited);
        assert_eq!(c.class.lever(), "nothing on the radio side");
        assert_eq!(c.confidence, Confidence::Medium);
    }

    #[test]
    fn twenty_nine_rows_are_not_a_reading() {
        let r = antenna_worth(&rows_of(29, -78.0, -9.0, 22.0), &[], NOW, WINDOW_SECS);
        assert_eq!(r.leading().unwrap().class, SignalClass::Insufficient);
    }

    #[test]
    fn a_row_without_a_cell_key_is_counted_not_pooled() {
        let mut rows = rows_of(60, -92.0, -15.0, 3.0);
        let mut loose = row(NOW - 30, -92.0, -15.0, 3.0);
        loose.earfcn_dl = None;
        rows.push(loose);
        let r = antenna_worth(&rows, &[], NOW, WINDOW_SECS);
        assert_eq!(r.cells.len(), 1);
        assert!(
            r.notes.iter().any(|n| n.contains("carry no earfcn")),
            "{:?}",
            r.notes
        );
    }

    #[test]
    fn no_speed_row_is_said_out_loud_and_does_not_change_the_class() {
        let rows = rows_of(120, -92.0, -15.0, 3.0);
        let without = antenna_worth(&rows, &[], NOW, WINDOW_SECS);
        let mut e = Entry::blank();
        e.ended_unix = NOW - 600;
        e.started_unix = e.ended_unix - 20;
        e.download_bps = 40_000_000.0;
        let with = antenna_worth(&rows, &[e], NOW, WINDOW_SECS);
        assert_eq!(
            without.leading().unwrap().class,
            with.leading().unwrap().class
        );
        assert_eq!(without.speed_rows, 0);
        assert!(without
            .missing
            .iter()
            .any(|m| m.contains("no saved speed run")));
        assert_eq!(with.speed_rows, 1);
        assert!((with.download_median_mbps.unwrap() - 40.0).abs() < 0.01);
    }

    #[test]
    fn the_table_names_no_purchase_and_prints_no_pipe_rate() {
        let r = antenna_worth(&rows_of(120, -92.0, -15.0, 3.0), &[], NOW, WINDOW_SECS);
        let t = table(&r);
        assert!(t.contains("interference-limited"), "{t}");
        for word in ["buy", "purchase", "order", "recommended antenna"] {
            assert!(!t.to_lowercase().contains(word), "{word} in {t}");
        }
        assert!(!t.contains("Mbps"), "no rate without a speed row: {t}");
    }

    #[test]
    fn an_empty_log_is_insufficient_not_a_guess() {
        let r = antenna_worth(&[], &[], NOW, WINDOW_SECS);
        assert!(r.cells.is_empty());
        assert!(r.missing.iter().any(|m| m.contains("only v2 rows")));
        assert!(table(&r).contains("no cell to judge"));
    }
}
