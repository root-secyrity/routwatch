//! The schedule, tested without a clock and without a router.
//!
//! The collection loop runs one due tier and then recomputes; every deadline
//! rule below is a pure function so the rules can be stated here instead of
//! being inferred from a sleep.

use std::time::{Duration, Instant};

use routwatch::worker::{due_at, due_tier, tier_cadence, Tier};

/// A deadline is counted from the start of the read, not from its end: the
/// sleep must not add itself to the cadence, which is the 3088 ms the measured
/// cycle carried.
#[test]
fn the_next_due_is_counted_from_the_start_not_the_end() {
    let interval = Duration::from_millis(600);
    let started = Instant::now();
    let read_took = Duration::from_millis(542);
    let now = started + read_took;

    let next = due_at(started, interval, now);

    assert_eq!(
        next,
        started + interval,
        "the deadline comes from the start, so a 542 ms read leaves 58 ms of the interval"
    );
}

/// A read that overruns its deadline does not buy a catch-up run: one run at the
/// first opportunity, then a fresh deadline, so a slow router cannot start a
/// burst of back to back reads.
#[test]
fn a_slow_read_does_not_stack_missed_ticks() {
    let interval = Duration::from_millis(600);
    let started = Instant::now();
    let now = started + Duration::from_millis(5_000);

    assert_eq!(
        due_at(started, interval, now),
        now + interval,
        "an overrun yields exactly one late run, then a deadline one interval from now"
    );
}

/// A tier is not read before its deadline, and the first tier whose deadline has
/// passed is the one that runs.
#[test]
fn a_tier_is_not_read_before_its_due() {
    let t0 = Instant::now();
    let due = [
        t0 + Duration::from_millis(600),
        t0 + Duration::from_secs(30),
        t0 + Duration::from_secs(60),
    ];

    assert_eq!(due_tier(&due, t0 + Duration::from_millis(500)), None);
    assert_eq!(
        due_tier(&due, t0 + Duration::from_millis(600)),
        Some(Tier::Radio)
    );
}

/// The radio tier follows the refresh knob; the two slower tiers do not, so a
/// fast refresh cannot drag the slowest read in the tree (wlan/host-list, p95
/// 2297 ms) up to the radio cadence.
#[test]
fn the_radio_tier_cadence_is_independent_of_the_clients_tier() {
    let fast = tier_cadence(Tier::Radio, 600);
    let slow = tier_cadence(Tier::Radio, 2_000);

    assert_eq!(fast, Duration::from_millis(600));
    assert_eq!(slow, Duration::from_millis(2_000));
    assert_eq!(tier_cadence(Tier::Clients, 600), Duration::from_secs(30));
    assert_eq!(tier_cadence(Tier::Clients, 2_000), Duration::from_secs(30));
    assert_eq!(tier_cadence(Tier::Static, 600), Duration::from_secs(60));
}
