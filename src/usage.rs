//! Local usage ledger and budget baselines.
//!
//! The router exposes a month counter on some firmwares and nothing at all on
//! others. This module keeps the parts that must survive a restart (a per-day
//! byte ledger and the value the budget bar counts from) in one small JSON file
//! under `$XDG_DATA_HOME/routwatch/`. It never touches the network.

use crate::model::{Quota, Traffic};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;
/// A sample gap longer than this is recorded as a gap, not spread over the
/// missing minutes: a closed laptop did not download anything.
pub const GAP_SECS: u64 = 300;
const KEEP_DAYS: usize = 200;
const KEEP_GAPS: usize = 20;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DayBucket {
    pub day: i64,
    pub dl: u64,
    pub ul: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Gap {
    pub from: u64,
    pub to: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Usage {
    pub version: u32,
    /// The router this ledger belongs to. A change restarts the baseline and
    /// keeps the daily history; see `switch_router`.
    pub router: String,
    /// Source total at the last budget reset, in bytes.
    pub base_bytes: u64,
    pub reset_at: u64,
    /// 0 = lifetime counters from traffic-statistics, 1 = router month counter.
    pub source: u8,
    /// Day number the current billing cycle started.
    pub cycle_start: i64,
    pub last_at: u64,
    pub last_dl: u64,
    pub last_ul: u64,
    pub days: Vec<DayBucket>,
    pub gaps: Vec<Gap>,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            version: VERSION,
            router: String::new(),
            base_bytes: 0,
            reset_at: 0,
            source: 0,
            cycle_start: 0,
            last_at: 0,
            last_dl: 0,
            last_ul: 0,
            days: Vec::new(),
            gaps: Vec::new(),
        }
    }
}

/// The total the budget is measured against: the router's own month counter
/// when it exists, the lifetime counters otherwise.
pub fn source_total(tr: &Traffic, quota: Option<&Quota>) -> u64 {
    match quota {
        Some(q) => q.month_dl_bytes.saturating_add(q.month_ul_bytes),
        None => tr.total_dl_bytes.saturating_add(tr.total_ul_bytes),
    }
}

pub fn used(tr: &Traffic, quota: Option<&Quota>, u: &Usage) -> u64 {
    source_total(tr, quota).saturating_sub(u.base_bytes)
}

/// Manual reset. The caller passes the source total it just sampled so this
/// function never needs the snapshot.
pub fn reset(u: &mut Usage, now: u64, total: u64, has_quota: bool) {
    u.base_bytes = total;
    u.reset_at = now;
    u.source = u8::from(has_quota);
}

/// A different router means different counters, not a different user: keep the
/// daily table, restart the baseline, and record the change as a gap so the
/// panel can say why the bar went back to zero.
pub fn switch_router(u: &mut Usage, now: u64, tr: &Traffic, quota: Option<&Quota>) {
    if u.last_at != 0 {
        u.gaps.push(Gap {
            from: u.last_at,
            to: now,
        });
        while u.gaps.len() > KEEP_GAPS {
            u.gaps.remove(0);
        }
    }
    u.router.clear();
    u.base_bytes = source_total(tr, quota);
    u.reset_at = now;
    u.source = u8::from(quota.is_some());
    u.cycle_start = 0;
    u.last_at = now;
    u.last_dl = tr.total_dl_bytes;
    u.last_ul = tr.total_ul_bytes;
}

fn add(days: &mut Vec<DayBucket>, day: i64, dl: u64, ul: u64) {
    match days.iter_mut().find(|b| b.day == day) {
        Some(b) => {
            b.dl = b.dl.saturating_add(dl);
            b.ul = b.ul.saturating_add(ul);
        }
        None => days.push(DayBucket { day, dl, ul }),
    }
}

/// Record one sample. Returns true when anything changed and the file should be
/// rewritten. The clock is injected so tests never depend on the timezone.
pub fn observe(
    u: &mut Usage,
    now: u64,
    tr: &Traffic,
    quota: Option<&Quota>,
    start_day: u8,
) -> bool {
    observe_day(u, now, local_day(now), tr, quota, start_day)
}

pub fn observe_day(
    u: &mut Usage,
    now: u64,
    day: LocalDay,
    tr: &Traffic,
    quota: Option<&Quota>,
    start_day: u8,
) -> bool {
    let mut dirty = false;
    let cycle = cycle_start_day(day, start_day);
    let src = source_total(tr, quota);
    let kind = u8::from(quota.is_some());

    if u.cycle_start == 0 {
        u.cycle_start = cycle;
        u.base_bytes = src;
        u.reset_at = now;
        u.source = kind;
        dirty = true;
    } else if cycle != u.cycle_start {
        // Automatic reset, first sample of the new billing cycle.
        u.cycle_start = cycle;
        u.base_bytes = src;
        u.reset_at = now;
        u.source = kind;
        dirty = true;
    }

    if kind != u.source {
        // The firmware gained or lost its month counter; the old baseline was
        // measured on a different number, so start again from the new one.
        u.source = kind;
        u.base_bytes = src;
        u.reset_at = now;
        dirty = true;
    } else if src < u.base_bytes {
        // The router cleared its own counter, or the lifetime counters wrapped.
        u.base_bytes = src;
        u.reset_at = now;
        dirty = true;
    }

    if u.last_at != 0 {
        let dt = now.saturating_sub(u.last_at);
        if dt <= GAP_SECS && tr.total_dl_bytes >= u.last_dl && tr.total_ul_bytes >= u.last_ul {
            let ddl = tr.total_dl_bytes - u.last_dl;
            let dul = tr.total_ul_bytes - u.last_ul;
            if ddl > 0 || dul > 0 {
                add(&mut u.days, day.num, ddl, dul);
                dirty = true;
            }
        } else if dt > GAP_SECS {
            u.gaps.push(Gap {
                from: u.last_at,
                to: now,
            });
            if u.gaps.len() > KEEP_GAPS {
                u.gaps.remove(0);
            }
            dirty = true;
        }
    }

    u.last_at = now;
    u.last_dl = tr.total_dl_bytes;
    u.last_ul = tr.total_ul_bytes;

    u.days.sort_by_key(|b| b.day);
    while u.days.len() > KEEP_DAYS {
        u.days.remove(0);
    }
    dirty
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub today: (u64, u64),
    pub week: (u64, u64),
    pub month: (u64, u64),
    /// First day the ledger covers, so the panel can say how much of the month
    /// it actually watched.
    pub first_day: Option<i64>,
}

/// Sums for the three rows. Week starts Sunday, month starts at the billing
/// cycle, both by local day number.
pub fn totals(u: &Usage, today: LocalDay) -> Totals {
    let week_start = today.num - i64::from(today.wday);
    let month_start = if u.cycle_start == 0 {
        today.num
    } else {
        u.cycle_start
    };
    let mut out = Totals::default();
    for b in &u.days {
        // A day after "today" means the clock moved back (a fixture replayed, a
        // timezone flip). Counting it would put tomorrow's bytes in this week.
        if b.day > today.num {
            continue;
        }
        if b.day == today.num {
            out.today.0 = out.today.0.saturating_add(b.dl);
            out.today.1 = out.today.1.saturating_add(b.ul);
        }
        if b.day >= week_start {
            out.week.0 = out.week.0.saturating_add(b.dl);
            out.week.1 = out.week.1.saturating_add(b.ul);
        }
        if b.day >= month_start {
            out.month.0 = out.month.0.saturating_add(b.dl);
            out.month.1 = out.month.1.saturating_add(b.ul);
        }
        out.first_day = Some(out.first_day.map_or(b.day, |f| f.min(b.day)));
    }
    out
}

/// `$XDG_DATA_HOME/routwatch/usage.json`, the same base `record::default_dir`
/// uses.
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("routwatch/usage.json")
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
}

/// One writer per machine. The first TUI takes an exclusive lock on
/// `usage.lock`; a second one gets `None` and runs read-only instead of
/// overwriting the first one's day. The kernel drops the lock when the process
/// exits, so a crash never leaves a stale claim.
pub fn claim(path: &Path) -> io::Result<Option<std::fs::File>> {
    // The crate is already unix-only (`src/output.rs:10` imports `os::unix`
    // unconditionally), so a `cfg(unix)` guard here would be noise.
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let lock = lock_path(path);
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // The lock file has no content worth keeping or cutting: the claim is
        // the flock on its inode, not the bytes.
        .truncate(false)
        .mode(0o600)
        .open(&lock)?;
    let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
    // SAFETY: `fd` is owned by `file` for the length of this call.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    Ok((rc == 0).then_some(file))
}

/// A missing or unreadable file is an empty ledger, never an error: the TUI must
/// start on a fresh machine. A file from another version is not read either;
/// `save` refuses to overwrite it, so the user sees "usage not saved" instead of
/// silently losing a ledger this build does not understand.
pub fn load(path: &Path) -> Usage {
    let raw = match std::fs::read(path) {
        Ok(r) => r,
        Err(_) => return Usage::default(),
    };
    match serde_json::from_slice::<Usage>(&raw) {
        Ok(u) if u.version == VERSION => u,
        _ => Usage::default(),
    }
}

/// Write via a sibling temp file and a rename, with 0600 on the file and 0700
/// on the directory. A half-written ledger is worse than a stale one.
pub fn save(path: &Path, u: &Usage) -> io::Result<()> {
    // Never destroy a ledger written by another build: the shape may differ.
    let found = std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("version").and_then(|n| n.as_u64()));
    if let Some(found) = found {
        if found != u64::from(VERSION) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "usage file is version {found}, this build writes {VERSION}; left untouched"
                ),
            ));
        }
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(u).map_err(io::Error::other)?;
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
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

    fn tr(dl: u64, ul: u64) -> Traffic {
        Traffic {
            total_dl_bytes: dl,
            total_ul_bytes: ul,
            ..Default::default()
        }
    }

    fn day(num: i64, wday: u32) -> LocalDay {
        LocalDay { num, wday }
    }

    #[test]
    fn the_first_sample_sets_the_baseline_and_counts_nothing() {
        let mut u = Usage::default();
        assert!(observe_day(
            &mut u,
            1_000,
            day(20_700, 4),
            &tr(1_000, 500),
            None,
            1
        ));
        assert_eq!(u.base_bytes, 1_500);
        assert_eq!(used(&tr(1_000, 500), None, &u), 0);
        assert_eq!(u.last_dl, 1_000);
    }

    #[test]
    fn a_delta_lands_in_todays_bucket() {
        let mut u = Usage::default();
        observe_day(&mut u, 1_000, day(20_700, 4), &tr(1_000, 500), None, 1);
        observe_day(&mut u, 1_200, day(20_700, 4), &tr(1_500, 700), None, 1);
        assert_eq!(u.days.len(), 1);
        assert_eq!(u.days[0].dl, 500);
        assert_eq!(u.days[0].ul, 200);
        assert_eq!(used(&tr(1_500, 700), None, &u), 700);
    }

    #[test]
    fn a_gap_contributes_nothing_and_is_remembered() {
        let mut u = Usage::default();
        observe_day(&mut u, 1_000, day(20_700, 4), &tr(1_000, 0), None, 1);
        observe_day(
            &mut u,
            1_000 + GAP_SECS + 1,
            day(20_700, 4),
            &tr(9_000, 0),
            None,
            1,
        );
        assert!(u.days.is_empty());
        assert_eq!(u.gaps.len(), 1);
    }

    #[test]
    fn a_counter_that_goes_backwards_adds_nothing() {
        let mut u = Usage::default();
        observe_day(&mut u, 1_000, day(20_700, 4), &tr(1_000, 0), None, 1);
        observe_day(&mut u, 1_200, day(20_700, 4), &tr(10, 0), None, 1);
        assert!(u.days.is_empty());
        assert_eq!(u.last_dl, 10);
    }

    #[test]
    fn the_cycle_boundary_resets_the_budget_once() {
        let mut u = Usage::default();
        let start = days_from_civil(2026, 11, 1);
        observe_day(&mut u, 1_000, day(start, 0), &tr(1_000, 0), None, 1);
        observe_day(&mut u, 2_000, day(start + 1, 1), &tr(2_000, 0), None, 1);
        assert_eq!(used(&tr(2_000, 0), None, &u), 1_000);
        observe_day(&mut u, 2_200, day(start + 30, 1), &tr(9_000, 0), None, 1);
        assert_eq!(used(&tr(9_000, 0), None, &u), 0);
        assert_eq!(u.reset_at, 2_200);
    }

    #[test]
    fn a_sample_older_than_the_ledger_counts_no_future_days() {
        let mut u = Usage::default();
        let today = days_from_civil(2026, 2, 2);
        u.cycle_start = days_from_civil(2026, 2, 1);
        u.days = vec![
            DayBucket {
                day: today,
                dl: 10,
                ul: 1,
            },
            DayBucket {
                day: days_from_civil(2026, 11, 12),
                dl: 9_999,
                ul: 0,
            },
        ];
        let t = totals(&u, day(today, 1));
        assert_eq!(t.today, (10, 1));
        assert_eq!(t.week, (10, 1));
        assert_eq!(t.month, (10, 1));
        assert_eq!(t.first_day, Some(today));
    }

    #[test]
    fn the_week_starts_on_sunday() {
        let mut u = Usage::default();
        let sunday = days_from_civil(2026, 11, 8);
        observe_day(&mut u, 1_000, day(sunday - 1, 6), &tr(0, 0), None, 1);
        observe_day(&mut u, 1_100, day(sunday, 0), &tr(100, 0), None, 1);
        observe_day(&mut u, 1_200, day(sunday + 1, 1), &tr(300, 0), None, 1);
        let t = totals(&u, day(sunday + 1, 1));
        assert_eq!(t.week, (300, 0));
        assert_eq!(t.today, (200, 0));
    }

    #[test]
    fn a_delta_across_midnight_lands_in_the_new_day() {
        let mut u = Usage::default();
        observe_day(&mut u, 86_100, day(20_700, 3), &tr(0, 0), None, 1);
        observe_day(&mut u, 86_400, day(20_701, 4), &tr(200, 0), None, 1);
        assert_eq!(totals(&u, day(20_701, 4)).today, (200, 0));
        assert_eq!(totals(&u, day(20_700, 3)).today, (0, 0));
    }

    #[test]
    fn a_router_switch_keeps_the_days_and_restarts_the_baseline() {
        let mut u = Usage::default();
        observe_day(&mut u, 1_000, day(20_700, 4), &tr(1_000, 0), None, 1);
        observe_day(&mut u, 1_200, day(20_700, 4), &tr(1_200, 0), None, 1);
        switch_router(&mut u, 1_300, &tr(50_000, 0), None);
        assert_eq!(u.days.len(), 1);
        assert_eq!(used(&tr(50_000, 0), None, &u), 0);
        assert_eq!(u.gaps.len(), 1);
        assert_eq!(u.router, "");
    }

    #[test]
    fn the_month_row_follows_the_cycle_not_the_calendar() {
        let mut u = Usage::default();
        let cycle = days_from_civil(2026, 10, 15);
        u.cycle_start = cycle;
        u.days = vec![
            DayBucket {
                day: days_from_civil(2026, 10, 14),
                dl: 999,
                ul: 0,
            },
            DayBucket {
                day: cycle,
                dl: 10,
                ul: 1,
            },
        ];
        let t = totals(&u, day(days_from_civil(2026, 11, 2), 1));
        assert_eq!(t.month, (10, 1));
    }

    #[test]
    fn a_router_month_counter_becomes_the_source() {
        let mut u = Usage::default();
        let q = Quota {
            month_dl_bytes: 4_000,
            month_ul_bytes: 1_000,
            start_day: 1,
            ..Default::default()
        };
        observe_day(
            &mut u,
            1_000,
            day(20_700, 4),
            &tr(50_000, 9_000),
            Some(&q),
            1,
        );
        assert_eq!(u.source, 1);
        assert_eq!(used(&tr(50_000, 9_000), Some(&q), &u), 0);
        let q2 = Quota {
            month_dl_bytes: 5_000,
            month_ul_bytes: 1_000,
            start_day: 1,
            ..Default::default()
        };
        observe_day(
            &mut u,
            1_200,
            day(20_700, 4),
            &tr(50_000, 9_000),
            Some(&q2),
            1,
        );
        assert_eq!(used(&tr(50_000, 9_000), Some(&q2), &u), 1_000);
    }

    #[test]
    fn losing_the_router_counter_starts_a_new_baseline() {
        let mut u = Usage::default();
        let q = Quota {
            month_dl_bytes: 4_000,
            month_ul_bytes: 0,
            start_day: 1,
            ..Default::default()
        };
        observe_day(&mut u, 1_000, day(20_700, 4), &tr(90_000, 0), Some(&q), 1);
        observe_day(&mut u, 1_200, day(20_700, 4), &tr(90_000, 0), None, 1);
        assert_eq!(u.source, 0);
        assert_eq!(used(&tr(90_000, 0), None, &u), 0);
    }

    #[test]
    fn a_manual_reset_zeros_the_bar() {
        let mut u = Usage::default();
        observe_day(&mut u, 1_000, day(20_700, 4), &tr(1_000, 0), None, 1);
        reset(&mut u, 5_000, 4_000, false);
        assert_eq!(used(&tr(4_000, 0), None, &u), 0);
        assert_eq!(u.reset_at, 5_000);
    }

    #[test]
    fn save_and_load_round_trip_at_0600() {
        let dir = std::env::temp_dir().join(format!("routwatch-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("usage.json");
        let mut u = Usage::default();
        u.days.push(DayBucket {
            day: 20_700,
            dl: 10,
            ul: 2,
        });
        save(&path, &u).unwrap();
        assert_eq!(load(&path), u);
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_loads_as_empty() {
        let dir = std::env::temp_dir().join(format!("routwatch-usage-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.json");
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(load(&path), Usage::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_foreign_version_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("routwatch-usage-v-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.json");
        let foreign = br#"{"version":999,"days":[{"day":1,"dl":7,"ul":0}]}"#;
        std::fs::write(&path, foreign).unwrap();
        let err = save(&path, &Usage::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&path).unwrap(), foreign);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_writer_is_refused_the_lock() {
        let dir = std::env::temp_dir().join(format!("routwatch-usage-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("usage.json");
        let first = claim(&path).unwrap();
        assert!(first.is_some(), "the first claim must win");
        assert!(
            claim(&path).unwrap().is_none(),
            "the second claim must be refused"
        );
        drop(first);
        assert!(
            claim(&path).unwrap().is_some(),
            "the lock must die with the handle"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
