//! Try band sets one after another, measure each, and rank what was measured.
//!
//! Every row says what happened to that set: applied and measured, refused,
//! applied but no service, applied and never measured because the run stopped,
//! or never attempted. A set that was not measured is not ranked at the bottom
//! as if it were slow: it carries no number.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Measured {
        down_bps: f64,
        up_bps: Option<f64>,
        ttfb_ms: f64,
    },
    NoService,
    /// The set reached the router and the run ended before a measurement.
    AppliedNoMeasure {
        why: String,
    },
    /// The set landed and the measurement itself failed. Not `Refused`: the
    /// router accepted the change, the network did not produce a number.
    MeasFailed {
        why: String,
    },
    Refused {
        why: String,
    },
    NotRun,
}

impl Outcome {
    /// The word the table prints in its `result` column.
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Measured { .. } => "measured",
            Outcome::NoService => "applied, no service",
            Outcome::AppliedNoMeasure { .. } => "applied, not measured",
            Outcome::MeasFailed { .. } => "applied, measurement failed",
            Outcome::Refused { .. } => "refused",
            Outcome::NotRun => "not run",
        }
    }

    /// The reason this outcome carries, if it carries one.
    pub fn why(&self) -> Option<&str> {
        match self {
            Outcome::AppliedNoMeasure { why }
            | Outcome::MeasFailed { why }
            | Outcome::Refused { why } => Some(why),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetResult {
    pub bands: Vec<u8>,
    pub outcome: Outcome,
    pub note: Option<String>,
}

/// Order by measured download, fastest first. Ties break on ttfb. Rows without a
/// measurement keep their input order and sit at the end.
pub fn rank(results: &[SetResult]) -> Vec<usize> {
    let mut with: Vec<(usize, f64, f64)> = Vec::new();
    let mut without: Vec<usize> = Vec::new();
    for (i, r) in results.iter().enumerate() {
        match &r.outcome {
            Outcome::Measured {
                down_bps, ttfb_ms, ..
            } => with.push((i, *down_bps, *ttfb_ms)),
            _ => without.push(i),
        }
    }
    with.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.0.cmp(&b.0))
    });
    let mut order: Vec<usize> = with.into_iter().map(|(i, _, _)| i).collect();
    order.extend(without);
    order
}

/// What the wait after an applied set came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// The link carries an address again.
    Ready,
    /// The cap closed with no address.
    NoService,
    /// The user pressed Ctrl-C while the loop waited.
    Interrupted,
}

/// Everything the loop touches, injected so the order and the failure paths are
/// testable without a router, a clock or a byte on the wire.
pub trait ScanIo {
    fn apply(&mut self, bands: &[u8]) -> Result<(), String>;
    fn settle(&mut self) -> Settle;
    fn measure(&mut self) -> Result<Outcome, String>;
    /// Whether any `apply` in this run reached the router and was accepted.
    /// The restore is skipped when this is false: writing the starting mask back
    /// onto a radio that never moved resets the link for nothing.
    fn applied(&self) -> bool;
    /// Put back the mask the router had before the run.
    fn restore(&mut self) -> Result<(), String>;
    fn interrupted(&self) -> bool;
}

/// Runs the sets in order and restores the starting mask when a set landed.
/// The restore error comes back separately: the table is still worth printing
/// when the restore failed, and the caller has to say so out loud.
pub fn run(sets: &[Vec<u8>], io: &mut dyn ScanIo) -> (Vec<SetResult>, Option<String>) {
    let mut out = Vec::new();
    let mut stopped = false;
    for bands in sets {
        if stopped || io.interrupted() {
            stopped = true;
            out.push(SetResult {
                bands: bands.clone(),
                outcome: Outcome::NotRun,
                note: Some("stopped by Ctrl-C".into()),
            });
            continue;
        }
        match io.apply(bands) {
            Err(why) => out.push(SetResult {
                bands: bands.clone(),
                outcome: Outcome::Refused { why },
                note: None,
            }),
            Ok(()) => {
                // Ctrl-C can land inside `apply` (up to a 20s verify window):
                // measure before spending a settle window on a link nobody asked
                // to keep.
                if io.interrupted() {
                    stopped = true;
                    out.push(SetResult {
                        bands: bands.clone(),
                        outcome: Outcome::AppliedNoMeasure {
                            why: "stopped after applying".into(),
                        },
                        note: None,
                    });
                    continue;
                }
                match io.settle() {
                    Settle::Interrupted => {
                        stopped = true;
                        out.push(SetResult {
                            bands: bands.clone(),
                            outcome: Outcome::AppliedNoMeasure {
                                why: "stopped after applying".into(),
                            },
                            note: None,
                        });
                    }
                    Settle::NoService => out.push(SetResult {
                        bands: bands.clone(),
                        outcome: Outcome::NoService,
                        note: Some("no WAN address within the cap".into()),
                    }),
                    Settle::Ready => {
                        // A measurement is the longest step of the run (up to
                        // `worst_case_secs`, 103s by default). It is not
                        // interruptible, but it must not start after Ctrl-C.
                        if io.interrupted() {
                            stopped = true;
                            out.push(SetResult {
                                bands: bands.clone(),
                                outcome: Outcome::AppliedNoMeasure {
                                    why: "stopped before the measurement".into(),
                                },
                                note: None,
                            });
                            continue;
                        }
                        match io.measure() {
                            Ok(o) => out.push(SetResult {
                                bands: bands.clone(),
                                outcome: o,
                                note: None,
                            }),
                            // A failed measurement is not a refusal by the
                            // router. Naming it `Refused` put a network fault in
                            // the band's record; `MeasFailed` keeps the two
                            // apart in the table and in `--json`.
                            Err(why) => out.push(SetResult {
                                bands: bands.clone(),
                                outcome: Outcome::MeasFailed { why },
                                note: Some("applied, measurement failed".into()),
                            }),
                        }
                    }
                }
            }
        }
    }
    let restore_err = if io.applied() {
        io.restore().err()
    } else {
        None
    };
    (out, restore_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(down: f64, ttfb: f64) -> Outcome {
        Outcome::Measured {
            down_bps: down,
            up_bps: Some(down / 4.0),
            ttfb_ms: ttfb,
        }
    }

    fn set(bands: &[u8], outcome: Outcome) -> SetResult {
        SetResult {
            bands: bands.to_vec(),
            outcome,
            note: None,
        }
    }

    #[test]
    fn the_fastest_set_comes_first() {
        let rows = vec![
            set(&[1], measured(10e6, 30.0)),
            set(&[3], measured(90e6, 40.0)),
            set(&[5], measured(50e6, 20.0)),
        ];
        assert_eq!(rank(&rows), vec![1, 2, 0]);
    }

    #[test]
    fn a_tie_on_download_breaks_on_ttfb() {
        let rows = vec![
            set(&[1], measured(50e6, 40.0)),
            set(&[3], measured(50e6, 12.0)),
        ];
        assert_eq!(rank(&rows), vec![1, 0]);
    }

    /// A set nobody measured has no speed, so it cannot be placed on a speed
    /// scale: it keeps the order the user typed and sits after every number.
    #[test]
    fn rows_without_a_measurement_keep_their_order_at_the_end() {
        let rows = vec![
            set(&[1], Outcome::NoService),
            set(&[3], measured(20e6, 10.0)),
            set(
                &[5],
                Outcome::Refused {
                    why: "100006".into(),
                },
            ),
            set(
                &[7],
                Outcome::MeasFailed {
                    why: "no route".into(),
                },
            ),
            set(
                &[8],
                Outcome::AppliedNoMeasure {
                    why: "stopped".into(),
                },
            ),
            set(&[20], Outcome::NotRun),
        ];
        assert_eq!(rank(&rows), vec![1, 0, 2, 3, 4, 5]);
    }

    #[test]
    fn only_a_measurement_carries_a_number() {
        let bare = [
            Outcome::NoService,
            Outcome::AppliedNoMeasure { why: "x".into() },
            Outcome::MeasFailed { why: "x".into() },
            Outcome::Refused { why: "x".into() },
            Outcome::NotRun,
        ];
        for o in bare {
            assert!(
                !matches!(o, Outcome::Measured { .. }),
                "{o:?} must not carry a speed"
            );
        }
        assert!(matches!(measured(1.0, 1.0), Outcome::Measured { .. }));
    }

    /// A scripted `ScanIo`: every call answers from a queue and is recorded, so
    /// a test can assert what the loop asked for and in which order.
    struct Fake {
        apply: Vec<Result<(), String>>,
        settle: Vec<Settle>,
        measure: Vec<Result<Outcome, String>>,
        restored: usize,
        restore_err: Option<String>,
        interrupt_after: Option<usize>,
        applies: usize,
        measures: usize,
        settles: usize,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                apply: Vec::new(),
                settle: Vec::new(),
                measure: Vec::new(),
                restored: 0,
                restore_err: None,
                interrupt_after: None,
                applies: 0,
                measures: 0,
                settles: 0,
            }
        }

        fn ok(mut self) -> Fake {
            self.apply.push(Ok(()));
            self
        }

        fn refuse(mut self, why: &str) -> Fake {
            self.apply.push(Err(why.to_string()));
            self
        }

        fn settles(mut self, s: Settle) -> Fake {
            self.settle.push(s);
            self
        }

        fn measures(mut self, o: Outcome) -> Fake {
            self.measure.push(Ok(o));
            self
        }

        fn measure_fails(mut self, why: &str) -> Fake {
            self.measure.push(Err(why.to_string()));
            self
        }
    }

    impl ScanIo for Fake {
        fn apply(&mut self, _bands: &[u8]) -> Result<(), String> {
            let r = self
                .apply
                .get(self.applies)
                .cloned()
                .unwrap_or(Err("unscripted apply".into()));
            self.applies += 1;
            r
        }

        fn settle(&mut self) -> Settle {
            let s = self
                .settle
                .get(self.settles)
                .copied()
                .unwrap_or(Settle::NoService);
            self.settles += 1;
            s
        }

        fn measure(&mut self) -> Result<Outcome, String> {
            let r = self
                .measure
                .get(self.measures)
                .cloned()
                .unwrap_or(Err("unscripted measure".into()));
            self.measures += 1;
            r
        }

        fn applied(&self) -> bool {
            self.apply.iter().take(self.applies).any(|r| r.is_ok())
        }

        fn restore(&mut self) -> Result<(), String> {
            self.restored += 1;
            match &self.restore_err {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        fn interrupted(&self) -> bool {
            self.interrupt_after
                .map(|n| self.applies >= n)
                .unwrap_or(false)
        }
    }

    #[test]
    fn a_refused_set_does_not_stop_the_rest() {
        let mut io = Fake::new()
            .refuse("100006")
            .ok()
            .settles(Settle::Ready)
            .measures(measured(42e6, 20.0));
        let (rows, restore_err) = run(&[vec![1], vec![3]], &mut io);
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0].outcome, Outcome::Refused { .. }));
        assert_eq!(rows[1].outcome, measured(42e6, 20.0));
        assert_eq!(restore_err, None);
        assert_eq!(io.restored, 1, "the one set that landed is restored");
    }

    #[test]
    fn a_failed_measurement_is_named_meas_failed_not_refused() {
        let mut io = Fake::new()
            .ok()
            .settles(Settle::Ready)
            .measure_fails("speedtest: no route to host");
        let (rows, _) = run(&[vec![1]], &mut io);
        match &rows[0].outcome {
            Outcome::MeasFailed { why } => assert!(why.contains("no route")),
            other => panic!("a network fault must not read as a refusal: {other:?}"),
        }
    }

    /// Rewriting the starting mask onto a radio that never moved resets the
    /// link for nothing, so the restore is gated on something having landed.
    #[test]
    fn the_restore_is_never_asked_when_every_set_was_refused() {
        let mut io = Fake::new()
            .refuse("100006")
            .refuse("100005")
            .refuse("100006")
            .refuse("100005")
            .refuse("100006");
        let (rows, restore_err) = run(&[vec![1], vec![3], vec![5], vec![7], vec![8]], &mut io);
        assert_eq!(rows.len(), 5);
        assert!(rows
            .iter()
            .all(|r| matches!(r.outcome, Outcome::Refused { .. })));
        assert_eq!(io.restored, 0);
        assert_eq!(restore_err, None);
    }

    #[test]
    fn the_restore_error_is_returned_not_hidden() {
        let mut io = Fake::new()
            .refuse("100006")
            .refuse("100005")
            .refuse("100006")
            .refuse("100005")
            .refuse("100006");
        io.restore_err = Some("device still reports 3,8".into());
        let (rows, restore_err) = run(&[vec![1], vec![3], vec![5], vec![7], vec![8]], &mut io);
        assert_eq!(rows.len(), 5);
        assert_eq!(io.restored, 0, "nothing landed, so nothing is restored");
        assert_eq!(
            restore_err, None,
            "an unasked restore has no error to report"
        );

        let mut io = Fake::new().ok().settles(Settle::Interrupted);
        io.restore_err = Some("device still reports 3,8".into());
        let (_, restore_err) = run(&[vec![1], vec![3]], &mut io);
        assert_eq!(io.restored, 1);
        assert_eq!(restore_err.as_deref(), Some("device still reports 3,8"));
    }

    #[test]
    fn a_set_that_landed_is_restored_once_even_after_ctrl_c() {
        let mut io = Fake::new()
            .ok()
            .settles(Settle::Interrupted)
            .ok()
            .settles(Settle::Ready)
            .measures(measured(1e6, 5.0));
        let (rows, _) = run(&[vec![1], vec![3]], &mut io);
        assert_eq!(io.restored, 1);
        assert_eq!(rows.len(), 2, "every set gets a row, even the unrun one");
    }

    #[test]
    fn ctrl_c_stops_the_rest_and_never_starts_a_measurement() {
        let mut io = Fake::new()
            .ok()
            .settles(Settle::Ready)
            .measures(measured(10e6, 10.0));
        io.interrupt_after = Some(1);
        let (rows, _) = run(&[vec![1], vec![3], vec![5]], &mut io);
        assert_eq!(io.measures, 0, "no measurement may start after Ctrl-C");
        assert_eq!(io.applies, 1);
        for row in &rows[1..] {
            assert_eq!(row.outcome, Outcome::NotRun);
            assert_eq!(row.note.as_deref(), Some("stopped by Ctrl-C"));
        }
        assert_eq!(io.restored, 1);
    }

    #[test]
    fn every_row_prints_a_word_and_its_reason() {
        assert_eq!(measured(1.0, 1.0).label(), "measured");
        assert_eq!(Outcome::NoService.label(), "applied, no service");
        assert_eq!(
            Outcome::MeasFailed { why: "x".into() }.label(),
            "applied, measurement failed"
        );
        assert_eq!(Outcome::NotRun.label(), "not run");
        assert_eq!(Outcome::NotRun.why(), None);
        assert_eq!(
            Outcome::Refused {
                why: "100006".into()
            }
            .why(),
            Some("100006")
        );
    }
}
