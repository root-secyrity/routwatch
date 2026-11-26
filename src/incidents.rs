//! Incidents: what the logs say happened, and what they cannot say.
//!
//! An incident is a window in which the logs kept reporting the same fault. It
//! is derived, never recorded: nothing here writes a file, reads a clock or
//! contacts the router, so the same logs always produce the same list.
//!
//! The one rule that shapes the whole module: this tool refuses causality. Two
//! events can only be ordered if the signal samples bracket both of them, and
//! only if the gap between those samples is narrower than the distance between
//! the events. The smear is therefore read off the log around the pair, never
//! taken from a constant: a cadence change moves it. When the samples do not
//! bracket the pair, or the gap is wider than the distance, the incident says so
//! and no incident ever names a cause. Confidence is the count of rows that
//! agree, not a probability.

use std::path::Path;

use crate::history;
use crate::signal_history::{self, Row};

/// Rows of the same fault further apart than this are two incidents, not one.
pub const WINDOW_SECS: u64 = 300;

/// The families an incident can belong to, named after the row kind they come
/// from so a reader never needs this enum to read the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Read,
    Wan,
    Signal,
    Write,
    Renew,
    /// A speed test that stopped early: evidence the link was degraded, never a
    /// cause.
    Speed,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Wan => "wan",
            Kind::Signal => "signal",
            Kind::Write => "write",
            Kind::Renew => "renew",
            Kind::Speed => "speed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Kind::Read),
            "wan" => Some(Kind::Wan),
            "signal" => Some(Kind::Signal),
            "write" => Some(Kind::Write),
            "renew" => Some(Kind::Renew),
            "speed" => Some(Kind::Speed),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Read => "read failed",
            Kind::Wan => "wan down",
            Kind::Signal => "signal poor",
            Kind::Write => "write failed",
            Kind::Renew => "renew failed",
            Kind::Speed => "speed test unfinished",
        }
    }

    /// The outcome word in the observation log that makes a row a fault. A row
    /// that reports a recovery is a row, not an incident: the fix is not a fault.
    fn is_fault(self, outcome: &str) -> bool {
        match self {
            Kind::Read | Kind::Write => outcome == "failed",
            Kind::Wan => outcome == "down",
            Kind::Signal => outcome == "poor",
            Kind::Renew => outcome == "down" || outcome == "no address",
            // A speed row carries no outcome word; it never arrives here.
            Kind::Speed => false,
        }
    }
}

/// Confidence is the number of rows that agree, nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// One row is a rumour, two agree, four or more is a pattern.
pub fn confidence(evidence: usize) -> Confidence {
    match evidence {
        0 | 1 => Confidence::Low,
        2 | 3 => Confidence::Medium,
        _ => Confidence::High,
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Incident {
    /// Stable across runs: the kind and the first moment, no counter, no clock.
    pub id: String,
    pub kind: Kind,
    pub start_unix: u64,
    pub end_unix: u64,
    pub evidence: usize,
    pub confidence: Confidence,
    /// The rows, oldest first, as the logs wrote them.
    pub lines: Vec<String>,
    /// Set when something else happened inside the smear. It states the limit,
    /// it never names a cause.
    pub caveat: Option<String>,
}

impl Incident {
    pub fn title(&self) -> String {
        format!(
            "{} ({} rows, {:?})",
            self.kind.title(),
            self.evidence,
            self.confidence
        )
    }

    pub fn window(&self) -> String {
        let secs = self.end_unix.saturating_sub(self.start_unix);
        format!("{} (+{secs}s)", self.start_unix)
    }
}

/// Everything `detect` is allowed to look at. Passing it explicitly is what
/// keeps the function pure: no path, no env, no clock inside.
pub struct Source {
    pub events: Vec<crate::events::ObsEvent>,
    pub signal: Vec<Row>,
    pub speed: Vec<history::Entry>,
}

impl Source {
    /// The three logs, read from the data dir. A missing file is an empty log,
    /// not an error: incidents are derived from whatever exists.
    pub fn load() -> Source {
        let events = crate::events::path()
            .map(|p| crate::events::load(&p).rows)
            .unwrap_or_default();
        let signal = signal_history::path()
            .map(|p| signal_history::load(&p).rows)
            .unwrap_or_default();
        let speed = history::path()
            .map(|p| history::load(&p).rows)
            .unwrap_or_default();
        Source {
            events,
            signal,
            speed,
        }
    }

    pub fn load_from(events: &Path, signal: &Path, speed: &Path) -> Source {
        Source {
            events: crate::events::load(events).rows,
            signal: signal_history::load(signal).rows,
            speed: history::load(speed).rows,
        }
    }
}

/// The 95th percentile of a set of gaps, in milliseconds. `None` when there is
/// nothing to rank. Sorts in place: the caller hands over a scratch vector.
pub fn gap_p95(gaps: &mut [u64]) -> Option<u64> {
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    // Nearest rank: ceil(95/100 * n), one-based. A floor index, `n * 95 / 100`,
    // lands on the last element whenever n is 20 or less, so a short window
    // reports its widest gap as the 95th percentile.
    let rank = (gaps.len() * 95).div_ceil(100).max(1);
    Some(gaps[rank - 1])
}

/// The smear that covers a window: the 95th percentile gap between the samples
/// inside it, in milliseconds, read off a sorted copy so a log written out of
/// order measures the same. `None` when the window holds fewer than two samples
/// or no positive gap, because zero would claim the order of a pair inside a
/// hole is settled. `samples` is unix seconds and may arrive in any order.
pub fn smear_in_window(samples: &[u64], since: u64, now: u64) -> Option<u64> {
    let mut at: Vec<u64> = samples
        .iter()
        .copied()
        .filter(|t| *t >= since && *t <= now)
        .collect();
    at.sort_unstable();
    let mut gaps: Vec<u64> = at
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]) * 1000)
        .filter(|g| *g > 0)
        .collect();
    gap_p95(&mut gaps)
}

/// Which two things are being ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairSide {
    /// Two log events. Each carries its own stamp.
    TwoEvents,
    /// An event and a signal reading. The reading has no stamp of its own; the
    /// signal samples around it bound how closely the log can place it.
    EventAndReading,
}

/// What the log can say about the order of one pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairOrder {
    /// The stamps settle it.
    Known,
    /// Both sides carry the same stamp: the log records whole seconds, so either
    /// could have come first inside that second.
    SameStamp,
    /// An event and a reading no further apart than the measured smear: the
    /// reading sits between two samples, so its side of the event cannot be read.
    InsideSmear,
    /// An event and a reading with no rankable gap in the window: there is no
    /// number to compare against, and zero would claim the order it cannot know.
    NoReads,
}

/// Order one pair. This is the only definition of "which came first": two
/// events are ordered by their own stamps and the signal smear never enters that
/// pair; an event against a reading is ordered only outside the smear of the
/// window, and a window without two reads settles nothing. Every distance is
/// whole milliseconds: dividing by 1000 would truncate a 2860 ms smear to 2 s
/// and flip the verdict at the boundary.
pub fn classify_pair(side: PairSide, a_ms: u64, b_ms: u64, smear_ms: Option<u64>) -> PairOrder {
    if a_ms == b_ms {
        return PairOrder::SameStamp;
    }
    match side {
        PairSide::TwoEvents => PairOrder::Known,
        PairSide::EventAndReading => match smear_ms {
            None => PairOrder::NoReads,
            // The boundary counts as inside: a pair exactly as far apart as the
            // smear is not ordered by a log that cannot resolve better.
            Some(smear) if a_ms.abs_diff(b_ms) <= smear => PairOrder::InsideSmear,
            Some(_) => PairOrder::Known,
        },
    }
}

/// The clause an incident adds about a partner it cannot order. `None` when the
/// pair is ordered. It names no cause in every branch, and the InsideSmear
/// branch prints the measured smear instead of a constant.
pub fn order_note(order: PairOrder, smear_ms: Option<u64>) -> Option<String> {
    match order {
        PairOrder::Known => None,
        PairOrder::SameStamp => Some(
            "carries the same stamp; the log records whole seconds, so the order is not \
             resolvable and this incident names no cause"
                .to_string(),
        ),
        PairOrder::InsideSmear => Some(match smear_ms {
            Some(ms) => format!(
                "falls inside the measured smear of {ms} ms, so the order is not resolvable and \
                 this incident names no cause"
            ),
            // InsideSmear is only returned when the smear is known; the wording
            // never prints a zero it did not measure.
            None => "sits beside a change whose smear is not measurable, so the order is not \
                     resolvable and this incident names no cause"
                .to_string(),
        }),
        PairOrder::NoReads => Some(
            "sits in a window with fewer than two reads, so no gap can rank the pair, and this \
             incident names no cause"
                .to_string(),
        ),
    }
}

/// The sentence an incident carries when a second kind cannot be ordered against
/// it. Every partner here is an event or a speed row, so each carries its own
/// stamp and only the same second leaves the order open.
fn caveat_for(kind: Kind, at_ms: u64, others: &[(Kind, u64)]) -> Option<String> {
    for (other, at) in others.iter().filter(|(k, _)| *k != kind) {
        let order = classify_pair(PairSide::TwoEvents, at_ms, *at, None);
        if let Some(note) = order_note(order, None) {
            return Some(format!("a {} event {note}", other.title()));
        }
    }
    None
}

/// Group the logs into incidents. Pure: same input, same output.
pub fn detect(src: &Source) -> Vec<Incident> {
    let mut rows: Vec<(Kind, u64, String)> = Vec::new();
    for e in &src.events {
        let Some(kind) = Kind::parse(&e.kind) else {
            continue;
        };
        if !kind.is_fault(&e.outcome) {
            continue;
        }
        let detail = match &e.detail {
            Some(d) => format!("{} {}: {d}", e.tier, e.kind,),
            None => format!("{} {}: {}", e.tier, e.kind, e.outcome),
        };
        rows.push((kind, e.at_unix, detail));
    }
    for e in &src.speed {
        if matches!(e.status, history::Status::Partial) {
            rows.push((
                Kind::Speed,
                e.started_unix,
                format!("speed test did not finish: {}", e.status.label()),
            ));
        }
    }
    rows.sort_by_key(|(_, at, _)| *at);

    let mut out: Vec<Incident> = Vec::new();
    for (kind, at, line) in rows {
        let join = match out.last_mut() {
            Some(last) if last.kind == kind && at.saturating_sub(last.end_unix) <= WINDOW_SECS => {
                Some(last)
            }
            _ => None,
        };
        match join {
            Some(last) => {
                last.end_unix = last.end_unix.max(at);
                last.evidence += 1;
                last.confidence = confidence(last.evidence);
                last.lines.push(line);
            }
            None => out.push(Incident {
                id: format!("{}-{at}", kind.as_str()),
                kind,
                start_unix: at,
                end_unix: at,
                evidence: 1,
                confidence: confidence(1),
                lines: vec![line],
                caveat: None,
            }),
        }
    }

    let others: Vec<(Kind, u64)> = out
        .iter()
        .map(|i| (i.kind, i.start_unix * 1000))
        .chain(
            src.speed
                .iter()
                .map(|e| (Kind::Speed, e.started_unix * 1000)),
        )
        .collect();
    for inc in out.iter_mut() {
        inc.caveat = caveat_for(inc.kind, inc.start_unix * 1000, &others);
    }
    out
}

pub fn find<'a>(incidents: &'a [Incident], id: &str) -> Option<&'a Incident> {
    incidents.iter().find(|i| i.id == id)
}

/// The incident as a document that can be pasted into an issue. `redact` drops
/// every detail line: an error string can carry a host or an address, and a
/// report is not the place to find out.
pub fn markdown(inc: &Incident, redact: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("# routwatch incident\n\n");
    let _ = writeln!(out, "- id: {}", inc.id);
    let _ = writeln!(out, "- what: {}", inc.kind.title());
    let _ = writeln!(out, "- window: {}", inc.window());
    let _ = writeln!(out, "- evidence: {} rows", inc.evidence);
    let _ = writeln!(out, "- confidence: {:?}", inc.confidence);
    out.push_str("\n## Rows\n\n");
    for line in &inc.lines {
        if redact {
            let _ = writeln!(out, "- (hidden)");
        } else {
            let _ = writeln!(out, "- {line}");
        }
    }
    out.push_str("\n## What this does not say\n\n");
    match &inc.caveat {
        Some(c) => {
            let _ = writeln!(out, "{c}.");
        }
        None => {
            let _ = writeln!(
                out,
                "No cause is named. The logs record what answered, not why it stopped."
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EventKind, ObsEvent};

    fn ev(kind: EventKind, outcome: &str, at: u64) -> ObsEvent {
        ObsEvent::new(kind, "signal", at, outcome, None)
    }

    fn src(events: Vec<ObsEvent>) -> Source {
        Source {
            events,
            signal: Vec::new(),
            speed: Vec::new(),
        }
    }

    #[test]
    fn a_quiet_log_yields_no_incidents() {
        assert!(detect(&src(vec![ev(EventKind::Wan, "up", 100)])).is_empty());
    }

    /// The log is written by one thread but read by a command that may sort it
    /// first: the smear is a measurement of the window, so order must not move
    /// it.
    #[test]
    fn a_mixed_order_log_measures_the_same_as_a_sorted_one() {
        let sorted = [0, 10, 20, 30];
        let mixed = [20, 0, 30, 10];
        assert_eq!(
            smear_in_window(&mixed, 0, 30),
            smear_in_window(&sorted, 0, 30)
        );
        assert_eq!(smear_in_window(&sorted, 0, 30), Some(10_000));
    }

    /// The overnight hole in the real log is 16 hours wide: an event read inside
    /// the day must not inherit it.
    #[test]
    fn a_gap_outside_the_window_does_not_enter() {
        let samples = [0, 57_600, 57_660];
        assert_eq!(
            smear_in_window(&samples, 57_600, 57_660),
            Some(60_000),
            "the 16 hour hole sits before the window"
        );
    }

    /// The max is what made every event look unresolved: a 58 000 s overnight gap
    /// outranked every real cadence gap. One outlier must not win the rank.
    #[test]
    fn the_rank_is_the_95th_percentile_not_the_widest_gap() {
        let mut samples: Vec<u64> = (0..100).map(|i| i * 10).collect();
        samples.push(1_990);
        let smear = smear_in_window(&samples, 0, 1_990).expect("a hundred gaps");
        assert_eq!(smear, 10_000, "99 short gaps outrank the one 1000 s gap");
    }

    /// Nearest rank, not a floor index: a window of 20 gaps ranks its second
    /// largest, so the widest gap cannot win by being the last element.
    #[test]
    fn the_percentile_is_nearest_rank_for_a_small_window() {
        let mut twenty: Vec<u64> = (1..=20).collect();
        assert_eq!(gap_p95(&mut twenty), Some(19), "ceil(0.95 * 20) is 19");
        assert_eq!(gap_p95(&mut []), None);
        assert_eq!(gap_p95(&mut [7]), Some(7));
        assert_eq!(
            gap_p95(&mut [5, 9]),
            Some(9),
            "ceil(1.9) is the second value"
        );
    }

    #[test]
    fn one_sample_or_none_is_unmeasurable() {
        assert_eq!(smear_in_window(&[], 0, 100), None);
        assert_eq!(smear_in_window(&[50], 0, 100), None);
        assert_eq!(
            smear_in_window(&[50, 50], 0, 100),
            None,
            "two rows in the same second bound nothing"
        );
        assert_eq!(smear_in_window(&[200, 300], 0, 100), None, "both outside");
    }

    /// A cell change is context for everything read after it, never a fault: the
    /// log can carry one and the incident list still comes back empty.
    #[test]
    fn a_cell_change_alone_opens_no_incident() {
        assert_eq!(Kind::parse("cell"), None);
        let loaded = src(vec![ObsEvent::new(
            EventKind::Cell,
            "signal",
            100,
            "changed",
            Some("earfcn_dl 100 pci 481 -> earfcn_dl 300 pci 395 band B3 tac 516 held 2".into()),
        )]);
        assert!(detect(&loaded).is_empty());
    }

    #[test]
    fn the_same_fault_twice_in_the_window_is_one_incident() {
        let out = detect(&src(vec![
            ev(EventKind::ReadFail, "failed", 100),
            ev(EventKind::ReadFail, "failed", 160),
        ]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].evidence, 2);
        assert_eq!(out[0].confidence, Confidence::Medium);
        assert_eq!(out[0].end_unix, 160);
    }

    #[test]
    fn a_gap_wider_than_the_window_is_two_incidents() {
        let out = detect(&src(vec![
            ev(EventKind::ReadFail, "failed", 100),
            ev(EventKind::ReadFail, "failed", 100 + WINDOW_SECS + 1),
        ]));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn confidence_follows_the_evidence_count() {
        assert_eq!(confidence(1), Confidence::Low);
        assert_eq!(confidence(2), Confidence::Medium);
        assert_eq!(confidence(3), Confidence::Medium);
        assert_eq!(confidence(4), Confidence::High);
    }

    #[test]
    fn two_kinds_in_the_same_second_are_not_ordered() {
        let out = detect(&src(vec![
            ev(EventKind::ReadFail, "failed", 1000),
            ev(EventKind::Wan, "down", 1000),
        ]));
        assert_eq!(out.len(), 2, "two kinds are two incidents");
        for inc in &out {
            let caveat = inc
                .caveat
                .as_deref()
                .expect("the same stamp must be stated");
            assert!(caveat.contains("same stamp"), "{caveat}");
            assert!(caveat.contains("no cause"), "{caveat}");
            assert!(!caveat.contains("caused"));
        }
    }

    /// Two events carry their own stamps, so one second apart is ordered. The
    /// signal log is empty here on purpose: a log with no reads used to turn this
    /// pair into "the samples do not bracket both moments".
    #[test]
    fn two_events_a_second_apart_are_ordered() {
        let out = detect(&src(vec![
            ev(EventKind::ReadFail, "failed", 1000),
            ev(EventKind::Wan, "down", 1001),
        ]));
        assert_eq!(out.len(), 2);
        for inc in &out {
            assert!(
                inc.caveat.is_none(),
                "stamps a second apart settle the order: {:?}",
                inc.caveat
            );
        }
    }

    #[test]
    fn the_classifier_orders_each_side_by_its_own_rule() {
        // Two events: their stamps settle it, whatever the smear says.
        assert_eq!(
            classify_pair(PairSide::TwoEvents, 100_000, 130_000, Some(61_000)),
            PairOrder::Known
        );
        assert_eq!(
            classify_pair(PairSide::TwoEvents, 100_000, 100_000, Some(61_000)),
            PairOrder::SameStamp
        );
        // An event against a reading: inside the smear is open, outside is not.
        assert_eq!(
            classify_pair(PairSide::EventAndReading, 100_000, 130_000, Some(61_000)),
            PairOrder::InsideSmear
        );
        assert_eq!(
            classify_pair(PairSide::EventAndReading, 100_000, 220_000, Some(61_000)),
            PairOrder::Known
        );
    }

    #[test]
    fn the_smear_boundary_counts_as_inside() {
        assert_eq!(
            classify_pair(PairSide::EventAndReading, 0, 61_000, Some(61_000)),
            PairOrder::InsideSmear,
            "a pair exactly as far apart as the smear is not ordered"
        );
        assert_eq!(
            classify_pair(PairSide::EventAndReading, 0, 61_001, Some(61_000)),
            PairOrder::Known
        );
    }

    #[test]
    fn a_window_without_two_reads_settles_nothing() {
        assert_eq!(
            classify_pair(PairSide::EventAndReading, 0, 61_000, None),
            PairOrder::NoReads
        );
        let note = order_note(PairOrder::NoReads, None).expect("a limit must be stated");
        assert!(note.contains("fewer than two reads"), "{note}");
        assert!(
            !note.chars().any(|c| c.is_ascii_digit()),
            "no invented number: {note}"
        );
    }

    #[test]
    fn the_same_second_note_carries_no_smear_number() {
        let caveat = caveat_for(Kind::Read, 100_000, &[(Kind::Wan, 100_000)])
            .expect("the same stamp leaves the order open");
        assert!(caveat.contains("whole seconds"), "{caveat}");
        assert!(caveat.contains("no cause"), "{caveat}");
        assert!(
            !caveat.contains("2860") && !caveat.contains(" ms"),
            "two events name no smear: {caveat}"
        );
    }

    #[test]
    fn an_id_names_the_kind_and_the_moment() {
        let out = detect(&src(vec![ev(EventKind::Wan, "down", 4242)]));
        assert_eq!(out[0].id, "wan-4242");
        assert!(find(&out, "wan-4242").is_some());
        assert!(find(&out, "wan-1").is_none());
    }

    #[test]
    fn a_recovery_row_is_not_an_incident() {
        let out = detect(&src(vec![
            ev(EventKind::Wan, "down", 10),
            ev(EventKind::Wan, "up", 11),
            ev(EventKind::Signal, "good", 12),
        ]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, Kind::Wan);
    }

    #[test]
    fn a_speed_row_inside_a_window_is_evidence_not_a_cause() {
        let mut entry = history::Entry::blank();
        entry.started_unix = 1000;
        entry.status = history::Status::Partial;
        let src = Source {
            events: vec![ev(EventKind::Wan, "down", 1000)],
            signal: Vec::new(),
            speed: vec![entry],
        };
        let out = detect(&src);
        assert_eq!(out.len(), 2, "a speed row is its own kind");
        let speed = out
            .iter()
            .find(|i| i.kind == Kind::Speed)
            .expect("the unfinished test must appear");
        assert_eq!(speed.evidence, 1);
        assert!(
            speed.caveat.as_deref().unwrap_or("").contains("same stamp"),
            "two rows in one second cannot be ordered"
        );
        assert!(!speed.caveat.as_deref().unwrap().contains("because"));
    }

    #[test]
    fn a_redacted_report_keeps_the_shape_and_loses_the_detail() {
        let out = detect(&src(vec![ev(EventKind::Wan, "down", 7)]));
        let text = markdown(&out[0], true);
        assert!(text.contains("wan-7"));
        assert!(text.contains("(hidden)"));
        assert!(!text.contains("signal wan: down"));
    }
}
