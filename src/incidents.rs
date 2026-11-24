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

/// The widest gap the signal log itself shows, in milliseconds. Zero means the
/// log holds fewer than two samples, so it brackets nothing on its own.
pub fn smear_ms(src: &Source) -> u64 {
    let mut worst = 0;
    let mut prev: Option<u64> = None;
    for row in &src.signal {
        if let Some(p) = prev {
            worst = worst.max(row.at_unix.saturating_sub(p) * 1000);
        }
        prev = Some(row.at_unix);
    }
    worst
}

/// The smear that covers two moments: the widest gap between the signal samples
/// that bracket them. `None` means the samples do not reach both moments, so
/// their order is unknown. Every argument is milliseconds, and `samples` must be
/// sorted oldest first.
pub fn smear_between(samples: &[u64], a: u64, b: u64) -> Option<u64> {
    let lo = a.min(b);
    let hi = a.max(b);
    let last_before = samples.iter().rposition(|s| *s <= lo)?;
    let first_after = samples.iter().position(|s| *s >= hi)?;
    if first_after < last_before {
        return None;
    }
    let mut worst = 0;
    for pair in samples[last_before..=first_after].windows(2) {
        worst = worst.max(pair[1] - pair[0]);
    }
    Some(worst)
}

/// The sentence an incident carries when a second kind sits inside the smear of
/// the pair. It always names the samples it was read from.
fn caveat_for(kind: Kind, at_ms: u64, others: &[(Kind, u64)], samples: &[u64]) -> Option<String> {
    for (other, at) in others.iter().filter(|(k, _)| *k != kind) {
        let title = other.title();
        match smear_between(samples, at_ms, *at) {
            Some(smear) if at.abs_diff(at_ms) <= smear => {
                return Some(format!(
                    "a {title} happened within {smear} ms of this window; that is the widest \
                     gap between the signal samples around both moments, so their order is not \
                     resolvable and this incident names no cause"
                ));
            }
            Some(_) => continue,
            None => {
                return Some(format!(
                    "a {title} happened near this window and the signal log does not bracket \
                     both moments, so their order is unknown and this incident names no cause"
                ));
            }
        }
    }
    None
}

/// Group the logs into incidents. Pure: same input, same output.
pub fn detect(src: &Source) -> Vec<Incident> {
    let mut samples: Vec<u64> = src.signal.iter().map(|r| r.at_unix * 1000).collect();
    samples.sort_unstable();
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
        inc.caveat = caveat_for(inc.kind, inc.start_unix * 1000, &others, &samples);
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
    fn a_pair_inside_the_smear_is_never_ordered() {
        let out = detect(&src(vec![
            ev(EventKind::ReadFail, "failed", 1000),
            ev(EventKind::Wan, "down", 1001),
        ]));
        assert_eq!(out.len(), 2, "two kinds are two incidents");
        for inc in &out {
            let caveat = inc.caveat.as_deref().expect("the smear must be stated");
            assert!(caveat.contains("no cause"), "{caveat}");
            assert!(!caveat.contains("caused"));
        }
    }

    #[test]
    fn the_same_gap_with_500ms_samples_is_ordered() {
        let smear = smear_between(&[100_000, 100_500, 101_000], 100_000, 101_000);
        assert_eq!(smear, Some(500), "the widest bracketing gap, in ms");
        let caveat = caveat_for(
            Kind::Read,
            100_000,
            &[(Kind::Wan, 101_000)],
            &[100_000, 100_500, 101_000],
        );
        assert!(
            caveat.is_none(),
            "a thousand ms apart is wider than a 500 ms gap, so the order holds"
        );
    }

    #[test]
    fn the_same_gap_with_3000ms_samples_is_unknown() {
        let caveat = caveat_for(
            Kind::Read,
            100_000,
            &[(Kind::Wan, 101_000)],
            &[100_000, 103_000],
        )
        .expect("a 1000 ms pair inside a 3000 ms gap cannot be ordered");
        assert!(caveat.contains("within 3000 ms"), "{caveat}");
        assert!(caveat.contains("no cause"), "{caveat}");
    }

    #[test]
    fn missing_samples_mean_no_cause() {
        let caveat = caveat_for(Kind::Read, 100_000, &[(Kind::Wan, 101_000)], &[])
            .expect("an unbracketed pair still states the limit");
        assert!(caveat.contains("does not bracket"), "{caveat}");
        assert!(caveat.contains("no cause"), "{caveat}");
        assert_eq!(smear_between(&[], 1, 2), None, "no sample reaches the pair");
        assert_eq!(
            smear_between(&[100_000], 100_000, 101_000),
            None,
            "one sample brackets one moment only"
        );
    }

    #[test]
    fn the_printed_smear_is_the_measured_one() {
        let caveat = caveat_for(
            Kind::Read,
            100_000,
            &[(Kind::Wan, 101_000)],
            &[100_000, 102_740],
        )
        .expect("the pair is inside the gap");
        assert!(caveat.contains("within 2740 ms"), "{caveat}");
        assert!(
            !caveat.contains("2860"),
            "no constant may stand in for the samples: {caveat}"
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
        entry.started_unix = 1002;
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
        assert!(speed.caveat.is_some(), "two kinds inside the smear");
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
