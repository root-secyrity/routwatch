//! Local usage ledger and budget baselines.
//!
//! The router exposes a month counter on some firmwares and nothing at all on
//! others. This module keeps the parts that must survive a restart (a per-day
//! byte ledger and the value the budget bar counts from) in one small JSON file
//! under `$XDG_DATA_HOME/routwatch/`. It never touches the network.

// No `use` lines yet: this task ends with `clippy -D warnings`, and an import
// that later tasks need is unused here. 1.3 adds `serde` and the model types,
// 1.4 adds `io` and `Path`.

pub const VERSION: u32 = 1;
/// A sample gap longer than this is recorded as a gap, not spread over the
/// missing minutes: a closed laptop did not download anything.
pub const GAP_SECS: u64 = 300;

/// A local calendar day and its weekday. `num` counts days from 1970-01-01,
/// `wday` is 0 for Sunday, matching `tm_wday`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDay {
    pub num: i64,
    pub wday: u32,
}

/// Howard Hinnant's days_from_civil: exact for the proleptic Gregorian
/// calendar, no libc, no timezone guess.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse, for labels such as `2026-11-01`.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The local calendar day for a unix timestamp, and its weekday.
pub fn local_day(now: u64) -> LocalDay {
    #[cfg(unix)]
    unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut tm).is_null() {
            return LocalDay {
                num: days_from_civil(
                    i64::from(tm.tm_year) + 1900,
                    tm.tm_mon as u32 + 1,
                    tm.tm_mday as u32,
                ),
                wday: tm.tm_wday as u32,
            };
        }
    }
    let days = (now / 86_400) as i64;
    LocalDay {
        num: days,
        wday: (days + 4).rem_euclid(7) as u32,
    }
}

pub fn date_label(ts: u64) -> String {
    // Zero means "no reset yet", not 1970: preview and the first frame pass 0.
    if ts == 0 {
        return "first sample".into();
    }
    let (y, m, d) = civil_from_days(local_day(ts).num);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Day the billing cycle started, clamped to 1..=28 so every month has the day.
pub fn cycle_start_day(day: LocalDay, start_day: u8) -> i64 {
    let (y, m, d) = civil_from_days(day.num);
    let sd = u32::from(start_day.clamp(1, 28));
    if d >= sd {
        days_from_civil(y, m, sd)
    } else if m == 1 {
        days_from_civil(y - 1, 12, sd)
    } else {
        days_from_civil(y, m - 1, sd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trips() {
        for z in [0_i64, 19_000, 20_700, 29_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn a_cycle_that_starts_today_starts_today() {
        let day = LocalDay {
            num: days_from_civil(2026, 11, 12),
            wday: 4,
        };
        assert_eq!(cycle_start_day(day, 12), days_from_civil(2026, 11, 12));
    }

    #[test]
    fn a_cycle_that_starts_later_falls_back_to_last_month() {
        let day = LocalDay {
            num: days_from_civil(2026, 11, 3),
            wday: 2,
        };
        assert_eq!(cycle_start_day(day, 15), days_from_civil(2026, 10, 15));
    }

    #[test]
    fn january_falls_back_to_december() {
        let day = LocalDay {
            num: days_from_civil(2027, 1, 4),
            wday: 1,
        };
        assert_eq!(cycle_start_day(day, 20), days_from_civil(2026, 12, 20));
    }

    #[test]
    fn a_start_day_above_28_is_clamped() {
        let day = LocalDay {
            num: days_from_civil(2026, 11, 30),
            wday: 1,
        };
        assert_eq!(cycle_start_day(day, 31), days_from_civil(2026, 11, 28));
    }

    #[test]
    fn epoch_was_a_thursday() {
        let d = local_day(0);
        assert_eq!(d.num, 0);
        assert_eq!(d.wday, 4);
    }

    #[test]
    fn a_zero_timestamp_is_not_1970() {
        assert_eq!(date_label(0), "first sample");
        let real = date_label(1_700_000_000);
        assert_eq!(
            real.len(),
            10,
            "a real timestamp keeps the date shape: {real}"
        );
        assert_eq!(real.as_bytes()[4], b'-');
    }
}
