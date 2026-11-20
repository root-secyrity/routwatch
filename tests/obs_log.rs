//! The observation log carries the file contract the two history logs already
//! promise: private on creation, one row per line, a torn tail repaired instead of
//! swallowed, damaged rows counted, compaction keeping whole newest rows.
//!
//! The `Watch` tests are the point of the log: a row is earned by a crossing, so
//! a link that stays poor for an hour writes one row, not one per read.

mod common;

use common::{isolate_data_home, TempDir};
use routwatch::events::{self, EventKind, ObsEvent, Watch};
use routwatch::model::Snapshot;

fn row(at_unix: u64, outcome: &str) -> ObsEvent {
    ObsEvent::new(EventKind::Wan, "signal", at_unix, outcome, None)
}

/// The same line the struct serialises to, written by hand: an integration test
/// cannot read a binary's dependencies, and the row shape is the contract under
/// test, so spelling it out is the stronger assertion.
fn line(at_unix: u64, outcome: &str) -> String {
    format!(
        "{{\"v\":1,\"at_unix\":{at_unix},\"tier\":\"signal\",\"kind\":\"wan\",\"outcome\":\"{outcome}\"}}\n"
    )
}

fn snap(status: &str, rsrp: f32) -> Snapshot {
    let mut s = Snapshot::default();
    s.connection.status = status.to_string();
    s.signal.rsrp_dbm = Some(rsrp);
    s
}

#[test]
fn a_row_lands_private_with_its_tier_and_moment() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("private");
    let log = dir.path().join("events.jsonl");

    events::append(&log, &row(1_700_000_000, "down")).expect("append");

    let mode = std::fs::metadata(&log)
        .expect("the log exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the log is private, got {mode:o}");

    let loaded = events::load(&log);
    assert_eq!(loaded.damaged, 0);
    assert_eq!(loaded.rows.len(), 1);
    assert_eq!(loaded.rows[0].tier, "signal");
    assert_eq!(loaded.rows[0].kind, "wan");
    assert_eq!(loaded.rows[0].at_unix, 1_700_000_000);
    assert_eq!(loaded.rows[0].event_kind(), Some(EventKind::Wan));
}

#[test]
fn a_torn_tail_does_not_swallow_the_next_row() {
    let dir = TempDir::new("torn");
    let log = dir.path().join("events.jsonl");
    // A process that died mid-write left half a row and no newline.
    std::fs::write(&log, "{\"v\":1,\"at_unix\":17").expect("seed");

    events::append(&log, &row(1_700_000_060, "up")).expect("append after a torn tail");

    let loaded = events::load(&log);
    assert_eq!(loaded.rows.len(), 1, "the new row is whole and alone");
    assert_eq!(loaded.damaged, 1, "the torn row is counted, not repaired");
    assert_eq!(loaded.rows[0].outcome, "up");
}

#[test]
fn a_damaged_row_is_counted_not_dropped_silently() {
    let dir = TempDir::new("damaged");
    let log = dir.path().join("events.jsonl");
    let mut body = String::new();
    body.push_str(&line(10, "down"));
    body.push_str("not a row\n");
    body.push_str(&line(20, "up"));
    std::fs::write(&log, body).expect("seed");

    let loaded = events::load(&log);
    assert_eq!(loaded.rows.len(), 2);
    assert_eq!(loaded.damaged, 1);
    assert_eq!(loaded.rows[1].at_unix, 20, "file order is chronological");
}

#[test]
fn compaction_keeps_the_newest_rows_whole() {
    let dir = TempDir::new("compact");
    let log = dir.path().join("events.jsonl");
    for i in 0..6 {
        events::append(&log, &row(100 + i, "down")).expect("append");
    }

    let kept = events::compact_if_needed(&log, 2).expect("compact");
    assert_eq!(kept, 2);

    let loaded = events::load(&log);
    assert_eq!(loaded.rows.len(), 2);
    assert_eq!(loaded.rows[0].at_unix, 104);
    assert_eq!(loaded.rows[1].at_unix, 105);
}

#[test]
fn the_watch_writes_one_row_per_crossing() {
    let mut watch = Watch::default();
    let good = snap("connected", -70.0);
    let poor = snap("connected", -105.0);
    let down = snap("disconnected", -70.0);

    watch.prime(&good);
    assert!(
        watch.radio(&good, 100).is_empty(),
        "a read that changes nothing writes nothing"
    );
    assert!(watch.radio(&good, 160).is_empty());

    let rows = watch.radio(&down, 220);
    assert_eq!(rows.len(), 1, "losing the link is one row, got {rows:?}");
    assert_eq!(rows[0].kind, "wan");
    assert_eq!(rows[0].outcome, "down");
    assert_eq!(rows[0].at_unix, 220, "stamped by the read, not the clock");

    assert!(
        watch.radio(&down, 280).is_empty(),
        "still down is the same state, not a second row"
    );

    let rows = watch.radio(&poor, 340);
    assert_eq!(rows.len(), 2, "up and poor arrived on one read");
    assert_eq!(rows[0].kind, "wan");
    assert_eq!(rows[0].outcome, "up");
    assert_eq!(rows[1].kind, "signal");
    assert_eq!(rows[1].outcome, "entered");
    assert!(
        rows[1].detail.as_deref().unwrap_or("").contains("rsrp"),
        "the crossing carries the readings: {:?}",
        rows[1].detail
    );

    assert!(watch.radio(&poor, 400).is_empty(), "poor is not new poor");

    let rows = watch.radio(&good, 460);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome, "left");
}

#[test]
fn a_write_row_names_what_was_written_not_what_answered() {
    let e = events::write_settled(1_700_000_000, "ok", Some("bands · apply 2 bands".into()));
    assert_eq!(e.kind, "write");
    assert_eq!(e.tier, "signal");
    assert_eq!(e.outcome, "ok");
    assert!(e.detail.unwrap().starts_with("bands"));
}

#[test]
fn the_log_path_follows_the_data_home() {
    let home = isolate_data_home("events-xdg");
    let resolved = events::path().expect("a data dir resolves");
    assert!(
        resolved.starts_with(home.path()),
        "the log follows XDG_DATA_HOME, got {resolved:?}"
    );
    assert!(resolved.ends_with("routwatch/events.jsonl"));
}
