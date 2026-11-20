//! The privacy baseline and the two-agreement rule.
//!
//! The rule is a claim about behaviour that the module cannot prove on its own:
//! one read is a rumour, a refusal is not a change, and a field that is absent
//! from this read says nothing about the setting. Each test pins one of those
//! sentences, so a later refactor that "simplifies" the comparison fails here.

mod common;

use common::{isolate_data_home, TempDir};
use routwatch::privacy::{Audit, Check, State};
use routwatch::privacy_drift::{Agree, Baseline, Drift};

fn audit(pairs: &[(&'static str, State)]) -> Audit {
    Audit {
        checks: pairs
            .iter()
            .map(|(name, state)| Check {
                name,
                state: *state,
                detail: String::new(),
            })
            .collect(),
    }
}

fn saved(pairs: &[(&'static str, State)]) -> Baseline {
    Baseline::from_audit(&audit(pairs), 1_700_000_000)
}

fn names(drifts: &[Drift]) -> Vec<&str> {
    drifts.iter().map(|d| d.name.as_str()).collect()
}

#[test]
fn a_baseline_is_saved_private_and_reloads() {
    let dir = TempDir::new("save");
    let file = dir.path().join("privacy-baseline.json");
    let base = saved(&[("tr069", State::NotExposed), ("firewall", State::Off)]);
    routwatch::privacy_drift::save(&file, &base).expect("save baseline");

    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&file)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode, 0o600, "the baseline must not be readable by others");

    let back = routwatch::privacy_drift::load(&file)
        .expect("load")
        .expect("a baseline");
    assert_eq!(back, base);
    assert_eq!(back.saved_at, 1_700_000_000);
}

#[test]
fn a_missing_baseline_is_not_an_error() {
    let dir = TempDir::new("missing");
    let file = dir.path().join("privacy-baseline.json");
    assert_eq!(routwatch::privacy_drift::load(&file), Ok(None));
}

#[test]
fn a_refusal_is_never_a_change() {
    let base = saved(&[("tr069", State::On), ("admin", State::On)]);
    let mut agree = Agree::default();
    // Two reads of a refused document: the firmware declined, it did not switch
    // anything off, and "unknown is not off" is the whole point of the arm.
    for _ in 0..2 {
        let now = audit(&[("tr069", State::Unknown), ("admin", State::NotExposed)]);
        assert!(agree.observe(&base, &now).is_empty());
    }
}

#[test]
fn one_read_is_a_rumour_and_two_reads_settle_it() {
    let base = saved(&[("firewall", State::On)]);
    let mut agree = Agree::default();
    let off = audit(&[("firewall", State::Off)]);

    assert!(
        agree.observe(&base, &off).is_empty(),
        "one read must not report a drift"
    );
    let settled = agree.observe(&base, &off);
    assert_eq!(names(&settled), vec!["firewall"]);
    assert_eq!(settled[0].from, "on");
    assert_eq!(settled[0].to, "off");
    assert!(settled[0].line().contains("two reads agreed"));

    // The same value on a third read is the same drift, already reported.
    assert!(agree.observe(&base, &off).is_empty());
}

#[test]
fn a_refusal_between_two_reads_does_not_break_their_agreement() {
    let base = saved(&[("firewall", State::On)]);
    let mut agree = Agree::default();
    // Off, then a refused document, then Off again: the two answering reads agree
    // and the refusal is no information, so the drift settles.
    assert!(agree
        .observe(&base, &audit(&[("firewall", State::Off)]))
        .is_empty());
    assert!(agree
        .observe(&base, &audit(&[("firewall", State::Unknown)]))
        .is_empty());
    assert_eq!(
        names(&agree.observe(&base, &audit(&[("firewall", State::Off)]))),
        vec!["firewall"]
    );
}

#[test]
fn a_value_that_alternates_with_the_baseline_never_settles() {
    let base = saved(&[("firewall", State::On)]);
    let mut agree = Agree::default();
    for _ in 0..3 {
        assert!(agree
            .observe(&base, &audit(&[("firewall", State::Off)]))
            .is_empty());
        assert!(agree
            .observe(&base, &audit(&[("firewall", State::On)]))
            .is_empty());
    }
}

#[test]
fn a_value_that_returns_to_the_baseline_is_no_drift() {
    let base = saved(&[("firewall", State::On)]);
    let mut agree = Agree::default();
    assert!(agree
        .observe(&base, &audit(&[("firewall", State::Off)]))
        .is_empty());
    // Back to the accepted answer: the candidate is dropped, not reported.
    assert!(agree
        .observe(&base, &audit(&[("firewall", State::On)]))
        .is_empty());
    assert!(agree
        .observe(&base, &audit(&[("firewall", State::Off)]))
        .is_empty());
    // A settled drift stays settled until the value returns to the baseline; a
    // later move to yet another value is news again.
    assert_eq!(
        agree
            .observe(&base, &audit(&[("firewall", State::Off)]))
            .len(),
        1
    );
}

#[test]
fn a_check_missing_from_the_read_is_not_a_change() {
    let base = saved(&[("tr069", State::On), ("dns", State::Off)]);
    let mut agree = Agree::default();
    // A short read is no information about the absent check, so it cannot drift.
    for _ in 0..3 {
        assert!(agree
            .observe(&base, &audit(&[("dns", State::Off)]))
            .is_empty());
    }
}

#[test]
fn a_check_absent_from_the_baseline_cannot_drift() {
    let base = saved(&[("tr069", State::On)]);
    let mut agree = Agree::default();
    for _ in 0..2 {
        assert!(agree
            .observe(
                &base,
                &audit(&[("tr069", State::On), ("admin", State::Off)])
            )
            .is_empty());
    }
}

#[test]
fn the_baseline_file_name_follows_the_data_home() {
    // `dirs::data_dir()` honours XDG_DATA_HOME on Linux; the CLI and this
    // assertion must agree on one path or `--diff` would look in the wrong place.
    let home = isolate_data_home("privacy-xdg");
    let path = routwatch::privacy_drift::path().expect("a data dir");
    assert_eq!(path, home.data_dir().join("privacy-baseline.json"));
}
