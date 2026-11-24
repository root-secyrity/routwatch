//! The alignment tab's numbers: one 15 s SINR average, the direction it moved
//! and the best average held since the serving cell last changed.
//!
//! Everything here is a pure function of the samples handed in: no clock, no
//! config, no render state. The caller keeps one bounded ring of samples and
//! asks for a view; the same samples always give the same view, so the tab can
//! be checked against a stationary capture instead of a live radio.

/// The averaging window. The radio tier is read about once a second (p50
/// 939 ms, p95 1748 ms, measured 2026-11-22), so 15 s holds ten to sixteen
/// reads: long enough that a single dip moves the number by a fraction of a dB.
pub const ALIGN_WINDOW_SECS: u64 = 15;

/// Movement smaller than this is not a direction. 3.6 dB is three times the
/// SINR spread measured on a stationary link (1.202 dB over 200 frames,
/// 2026-11-22), the same bar the comparison uses for a real change.
pub const SINR_DEADBAND_DB: f64 = 3.6;

const WINDOW_MS: u64 = ALIGN_WINDOW_SECS * 1000;

/// One radio read as the tab needs it: the value and when it was read. A ring
/// of values has no time axis, so its length is not seconds and a window
/// cannot be cut from it; the sample carries its own stamp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignSample {
    pub at_ms: u64,
    pub sinr_db: f64,
    /// The serving cell this read came from. A change clears the peak.
    pub pci: Option<u32>,
}

/// The best 15 s average held on the current cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakHold {
    pub sinr_db: f64,
    pub at_ms: u64,
    pub pci: Option<u32>,
}

/// Which way the average moved. Three words, because a 3.6 dB deadband cannot
/// support more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Rising,
    Falling,
    Steady,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Rising => "rising",
            Direction::Falling => "falling",
            Direction::Steady => "steady",
        }
    }

    pub fn arrow(self) -> &'static str {
        match self {
            Direction::Rising => "▲",
            Direction::Falling => "▼",
            Direction::Steady => "·",
        }
    }
}

/// One frame's worth of the tab, owned: the renderer gets a value, never a
/// borrow into the sample ring.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignView {
    /// The 15 s average, None when the window holds no read.
    pub average: Option<f64>,
    /// The average of the 15 s before that, None when it holds no read.
    pub previous: Option<f64>,
    pub direction: Direction,
    /// The best average since the serving cell last changed.
    pub peak: Option<PeakHold>,
    /// When the peak was cleared because the serving cell changed, so the tab
    /// can say it instead of showing a number that quietly restarted.
    pub cleared_at: Option<u64>,
}

/// The mean SINR of the reads no more than `window_ms` old as of `now_ms`, or
/// None when the window holds none. Ages, not stamps, so a window that starts
/// before the clock did is not a special case.
pub fn sinr_average(samples: &[AlignSample], now_ms: u64, window_ms: u64) -> Option<f64> {
    mean_of(
        samples
            .iter()
            .filter(|s| in_window(s.at_ms, now_ms, window_ms)),
    )
}

/// A read belongs to a window when it is not from the future and its age is
/// under the window. The edges are exclusive on the old side, so neighbouring
/// windows never count the same read twice.
fn in_window(at_ms: u64, now_ms: u64, window_ms: u64) -> bool {
    at_ms <= now_ms && now_ms - at_ms < window_ms
}

fn mean_of<'a>(reads: impl Iterator<Item = &'a AlignSample>) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0u32;
    for s in reads {
        sum += s.sinr_db;
        n += 1;
    }
    (n > 0).then(|| sum / f64::from(n))
}

/// The view of the samples as of `now_ms`.
///
/// The peak only covers the current serving cell: a cell change starts a new
/// segment, and the best average of the old cell is not this cell's peak.
pub fn align_engine(samples: &[AlignSample], now_ms: u64) -> AlignView {
    let mut sorted: Vec<AlignSample> = samples.to_vec();
    sorted.sort_by_key(|s| s.at_ms);

    let seg_start = last_cell_change(&sorted);
    let segment = &sorted[seg_start..];

    let average = sinr_average(segment, now_ms, WINDOW_MS);
    let previous = mean_of(segment.iter().filter(|s| {
        in_window(s.at_ms, now_ms, 2 * WINDOW_MS) && !in_window(s.at_ms, now_ms, WINDOW_MS)
    }));
    let direction = match (average, previous) {
        (Some(now), Some(was)) if now - was > SINR_DEADBAND_DB => Direction::Rising,
        (Some(now), Some(was)) if was - now > SINR_DEADBAND_DB => Direction::Falling,
        _ => Direction::Steady,
    };

    // The peak is a rolling average, not a single reading: one spike is not a
    // best window, and the tab's own number is an average. Equal averages tie
    // to the earliest stamp, which is when the level was reached.
    let mut peak: Option<PeakHold> = None;
    for s in segment {
        let Some(avg) = sinr_average(segment, s.at_ms, WINDOW_MS) else {
            continue;
        };
        let held = PeakHold {
            sinr_db: avg,
            at_ms: s.at_ms,
            pci: s.pci,
        };
        if peak.is_none_or(|best| held.sinr_db > best.sinr_db) {
            peak = Some(held);
        }
    }

    AlignView {
        average,
        previous,
        direction,
        peak,
        cleared_at: (seg_start > 0).then(|| sorted[seg_start].at_ms),
    }
}

/// The index where the current serving cell begins: the read after the last
/// PCI change, or 0 when the whole slice is one cell.
fn last_cell_change(samples: &[AlignSample]) -> usize {
    let mut start = 0;
    for i in 1..samples.len() {
        if samples[i].pci != samples[i - 1].pci {
            start = i;
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A burst of reads one second apart, all on one value and one cell.
    fn burst(at_ms: u64, n: u64, sinr_db: f64, pci: Option<u32>) -> Vec<AlignSample> {
        (0..n)
            .map(|i| AlignSample {
                at_ms: at_ms + i * 1000,
                sinr_db,
                pci,
            })
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_average_is_the_mean_of_its_window() {
        let samples = vec![
            // Older than both windows: it must not be averaged in either.
            AlignSample {
                at_ms: 0,
                sinr_db: 3.0,
                pci: Some(100),
            },
            AlignSample {
                at_ms: 40_000,
                sinr_db: 2.0,
                pci: Some(100),
            },
            AlignSample {
                at_ms: 41_000,
                sinr_db: 4.0,
                pci: Some(100),
            },
            AlignSample {
                at_ms: 42_000,
                sinr_db: 6.0,
                pci: Some(100),
            },
        ];
        let view = align_engine(&samples, 42_000);
        assert!(close(view.average.unwrap(), 4.0), "{view:?}");
        assert!(
            view.previous.is_none(),
            "the older window is empty: {view:?}"
        );
        assert_eq!(view.direction, Direction::Steady);
    }

    #[test]
    fn a_move_inside_the_deadband_is_steady_and_four_db_is_a_direction() {
        let mut samples = burst(0, 3, 5.0, Some(100));
        samples.extend(burst(20_000, 3, 8.0, Some(100)));
        let view = align_engine(&samples, 22_000);
        assert!(close(view.previous.unwrap(), 5.0), "{view:?}");
        assert!(close(view.average.unwrap(), 8.0), "{view:?}");
        assert_eq!(
            view.direction,
            Direction::Steady,
            "3 dB is inside the 3.6 dB deadband: {view:?}"
        );

        let mut up = burst(0, 3, 5.0, Some(100));
        up.extend(burst(20_000, 3, 9.0, Some(100)));
        assert_eq!(align_engine(&up, 22_000).direction, Direction::Rising);

        let mut down = burst(0, 3, 5.0, Some(100));
        down.extend(burst(20_000, 3, 1.0, Some(100)));
        assert_eq!(align_engine(&down, 22_000).direction, Direction::Falling);
    }

    #[test]
    fn the_peak_clears_when_the_cell_changes() {
        let mut samples = burst(0, 3, 10.0, Some(100));
        samples.extend(burst(20_000, 3, 3.0, Some(200)));
        let view = align_engine(&samples, 22_000);
        let peak = view.peak.unwrap();
        assert!(
            close(peak.sinr_db, 3.0),
            "the old cell's 10 dB is gone: {view:?}"
        );
        assert_eq!(peak.pci, Some(200));
        assert_eq!(peak.at_ms, 20_000, "the first window that averages 3.0");
        assert_eq!(
            view.cleared_at,
            Some(20_000),
            "the tab must be able to say the peak restarted: {view:?}"
        );
    }

    #[test]
    fn the_peak_is_the_best_average_not_the_last() {
        let mut samples = burst(0, 3, 5.0, Some(100));
        samples.extend(burst(20_000, 3, 9.0, Some(100)));
        samples.extend(burst(40_000, 3, 7.0, Some(100)));
        let view = align_engine(&samples, 42_000);
        let peak = view.peak.unwrap();
        assert!(close(peak.sinr_db, 9.0), "{view:?}");
        assert_eq!(peak.at_ms, 20_000, "when 9 dB was first held");
        assert!(close(view.average.unwrap(), 7.0), "{view:?}");
        assert!(view.cleared_at.is_none(), "one cell all along: {view:?}");
    }

    #[test]
    fn a_window_without_reads_has_no_average_and_no_peak() {
        let empty = align_engine(&[], 10_000);
        assert_eq!(empty.average, None);
        assert_eq!(empty.peak, None);
        assert_eq!(empty.direction, Direction::Steady);

        let old = burst(0, 3, 9.0, Some(100));
        let view = align_engine(&old, 100_000);
        assert_eq!(view.average, None, "the reads are 100 s old: {view:?}");
        assert_eq!(view.previous, None);
        assert!(
            view.peak.is_some(),
            "the session peak outlives the window: {view:?}"
        );
    }
}
