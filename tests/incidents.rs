//! Incidents are derived from files on disk, so the tests write the files.
//!
//! Every value here is synthetic: a made-up tier, a made-up moment. Nothing in
//! this file talks to a router.

use std::path::{Path, PathBuf};

use routwatch::incidents::{detect, smear_between, smear_ms, Confidence, Kind, WINDOW_SECS};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let base = std::env::temp_dir().join(format!(
            "rw-incidents-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("temp dir");
        TempDir(base)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn event_line(at: u64, kind: &str, outcome: &str) -> String {
    format!(r#"{{"v":1,"at_unix":{at},"tier":"signal","kind":"{kind}","outcome":"{outcome}"}}"#)
}

/// Every field is named: `Row` has no `Default`, so a partial row is a damaged
/// line and would read as a missing sample rather than a gap.
fn signal_line(at: u64) -> String {
    format!(
        r#"{{"v":1,"at_unix":{at},"net_type":null,"band":null,"nr_band":null,"rsrp_dbm":-95.0,"rsrq_db":null,"sinr_db":null,"pci":null,"endc":null}}"#
    )
}

fn write(path: &Path, lines: &[String]) {
    std::fs::write(path, lines.join("\n") + "\n").expect("write fixture");
}

#[test]
fn the_logs_on_disk_are_the_only_input() {
    let dir = TempDir::new();
    let events = dir.path("events.jsonl");
    write(
        &events,
        &[
            event_line(1000, "read", "failed"),
            event_line(1060, "read", "failed"),
            event_line(1200, "wan", "up"),
        ],
    );
    let src =
        routwatch::incidents::Source::load_from(&events, &dir.path("none"), &dir.path("none"));
    let list = detect(&src);
    assert_eq!(
        list.len(),
        1,
        "one fault window, and a recovery is not a fault"
    );
    assert_eq!(list[0].kind, Kind::Read);
    assert_eq!(list[0].evidence, 2);
    assert_eq!(list[0].confidence, Confidence::Medium);
}

#[test]
fn a_missing_log_is_an_empty_log_not_an_error() {
    let dir = TempDir::new();
    let src = routwatch::incidents::Source::load_from(
        &dir.path("no-events"),
        &dir.path("no-signal"),
        &dir.path("no-speed"),
    );
    assert!(detect(&src).is_empty());
}

#[test]
fn a_gap_past_the_window_splits_the_incident() {
    let dir = TempDir::new();
    let events = dir.path("events.jsonl");
    write(
        &events,
        &[
            event_line(1000, "read", "failed"),
            event_line(1000 + WINDOW_SECS + 1, "read", "failed"),
        ],
    );
    let src =
        routwatch::incidents::Source::load_from(&events, &dir.path("none"), &dir.path("none"));
    assert_eq!(detect(&src).len(), 2);
}

#[test]
fn two_faults_inside_the_smear_stay_unordered() {
    let dir = TempDir::new();
    let events = dir.path("events.jsonl");
    write(
        &events,
        &[
            event_line(1000, "read", "failed"),
            event_line(1001, "wan", "down"),
        ],
    );
    let src =
        routwatch::incidents::Source::load_from(&events, &dir.path("none"), &dir.path("none"));
    let list = detect(&src);
    assert_eq!(list.len(), 2);
    for inc in &list {
        let caveat = inc.caveat.as_deref().expect("the smear must be stated");
        assert!(caveat.contains("no cause"), "{caveat}");
    }
}

#[test]
fn a_wider_gap_in_the_signal_log_widens_the_refusal() {
    let dir = TempDir::new();
    let events = dir.path("events.jsonl");
    let signal = dir.path("signal.jsonl");
    write(&events, &[event_line(1000, "wan", "down")]);
    // A five second gap between two rows: the smear cannot be narrower than what
    // the log itself shows.
    write(&signal, &[signal_line(500), signal_line(505)]);
    let src = routwatch::incidents::Source::load_from(&events, &signal, &dir.path("none"));
    assert_eq!(smear_ms(&src), 5000);
    // The smear that matters is the one bracketing the pair, and here the log does
    // not reach the event at 1000 s at all.
    assert_eq!(
        smear_between(&[500_000, 505_000], 500_000, 505_000),
        Some(5000)
    );
    assert_eq!(smear_between(&[500_000, 505_000], 500_000, 1_000_000), None);
}

#[test]
fn the_same_logs_produce_the_same_list() {
    let dir = TempDir::new();
    let events = dir.path("events.jsonl");
    write(
        &events,
        &[
            event_line(10, "read", "failed"),
            event_line(20, "signal", "poor"),
            event_line(30, "read", "failed"),
        ],
    );
    let src =
        routwatch::incidents::Source::load_from(&events, &dir.path("none"), &dir.path("none"));
    assert_eq!(
        detect(&src),
        detect(&src),
        "detect reads no clock and no state"
    );
}
