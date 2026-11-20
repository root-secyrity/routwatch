//! The dashboard: seven regions over one time window.
//!
//! Presentation only. Every figure here is derived from `App` on the frame it
//! is drawn, and the two plots (the signal map and the throughput strip) read
//! the same clock: both call `chart::sample_slots`, so one sample cannot land in
//! one column above and another below. The window is fixed
//! (`app::WINDOW_SECS`); what adapts to the width is the width of a cell.
//!
//! A cell holds the worst sample it covers, not the average: a heatmap exists
//! to show the dip, and the average erases exactly that. A cell with no sample
//! is blank, never the neighbour's value and never a colour.
//!
//! The signal map encodes three separate channels: the row's identity (a chip
//! and its label, one fixed cool colour per metric), the magnitude (the bar
//! height on that metric's fixed range) and the state (the cell colour alone).
//! Height and colour both read one normalised value, so a cell cannot be tall
//! and red at the same time.

use crate::app::{App, WINDOW_SECS};
use crate::theme::{step_word, RSRP_DOMAIN, RSRQ_DOMAIN, SINR_DOMAIN};
use crate::ui::chart::slot_of;
use crate::ui::tabs::{panel, truncate};
use crate::ui::widgets::{bitrate, bytes, dur, pad};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::VecDeque;

/// Blank columns between a plot and the statistics field beside it.
pub(crate) const GAP_W: usize = 2;
/// The statistics field: "{min}..{max}  avg {avg}  now {state}".
pub(crate) const STATS_W: usize = 28;
/// The gutter of every plot row: " ▍RSRP  ", one blank, the chip, four
/// letters, two blanks. The chip is the identity channel and is drawn even when
/// the row holds no sample.
pub(crate) const GUTTER_W: usize = 8;
/// Left field of a row: the gutter. The throughput rows keep the same field so
/// one sample lands in the same column in both plots.
const LABEL_W: usize = GUTTER_W;

/// The eight heights of the magnitude channel.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Identity colours: a metric keeps its chip and label colour on every frame,
/// whatever its readings say.
const ID_RSRP: Color = Color::Rgb(0x6c, 0xb2, 0xff);
const ID_RSRQ: Color = Color::Rgb(0x31, 0xc8, 0xde);
const ID_SINR: Color = Color::Rgb(0xc4, 0x8f, 0xec);
/// State colours: the cell colour is the only channel that carries state.
const STATE_GOOD: Color = Color::Rgb(0x74, 0xdf, 0x7c);
const STATE_FAIR: Color = Color::Rgb(0xf4, 0xb1, 0x3c);
const STATE_POOR: Color = Color::Rgb(0xff, 0x6a, 0x5a);

/// How a width decides what the dashboard can afford.
#[derive(Debug, Clone, Copy)]
struct Mode {
    /// The min/avg/max column beside the map.
    stats: bool,
}

fn mode_for(area: Rect) -> Mode {
    Mode {
        stats: area.width >= 110 && area.height >= 34,
    }
}

/// Seconds between two stored samples, never below one: the store is filled by
/// the poll loop, and the poll interval is the only thing that spaces them.
fn sample_secs(app: &App) -> u64 {
    (app.refresh_ms / 1000).max(1)
}

/// The cell grid of one plot: every column the width leaves is a cell, and the
/// window is spread over them, so a narrow terminal gets coarser cells instead
/// of a shorter window.
fn grid(width: usize, stats: bool) -> usize {
    let reserve = if stats { GAP_W + STATS_W } else { 0 };
    width.saturating_sub(LABEL_W + reserve).max(4)
}

/// One reading on its metric's fixed range: (bar height 0..7, state 2/1/0).
///
/// This is the only place the map turns a value into a drawing: the height and
/// the state colour both read this one `t`, so they cannot disagree.
fn channel(value: f32, domain: (f32, f32)) -> (usize, u8) {
    let (lo, hi) = domain;
    let span = (hi - lo).max(f32::MIN_POSITIVE);
    let t = ((value - lo) / span).clamp(0.0, 1.0);
    // A reading at the top of the range is the eighth bar, not a ninth one.
    let level = ((t * 8.0).floor() as usize).min(BARS.len() - 1);
    let state = if t < 0.33 {
        0
    } else if t < 0.66 {
        1
    } else {
        2
    };
    (level, state)
}

fn state_color(state: u8) -> Color {
    match state {
        2 => STATE_GOOD,
        1 => STATE_FAIR,
        _ => STATE_POOR,
    }
}

fn state_word_of(state: u8) -> &'static str {
    match state {
        2 => "good",
        1 => "fair",
        _ => "poor",
    }
}

/// Seconds one cell stands for: the window over the columns it landed on. The
/// legend prints this number, so it can never claim a span the columns do not
/// have.
fn cell_label(cells: usize) -> String {
    let secs = WINDOW_SECS as f64 / cells.max(1) as f64;
    if (secs - secs.round()).abs() < 0.05 {
        format!("1 cell = {}s", secs.round() as u64)
    } else {
        format!("1 cell = {secs:.1}s")
    }
}

/// One series resampled onto the cell grid, worst sample per cell.
fn worst_cells(values: &VecDeque<f64>, cells: usize, spacing: u64) -> Vec<Option<f32>> {
    let mut out = vec![None; cells];
    if values.is_empty() || cells == 0 {
        return out;
    }
    for (i, v) in values.iter().enumerate() {
        let age = (values.len() - 1 - i) as u64 * spacing;
        if age >= WINDOW_SECS {
            continue;
        }
        // The one map every plot uses: seconds from the left edge of the window
        // to the cell that second falls in. Wrapping a sample across every cell
        // its span touches would draw readings that were never taken.
        let slot = slot_of(WINDOW_SECS - age - 1, WINDOW_SECS, cells);
        out[slot] = Some(match out[slot] {
            Some(current) => current.min(*v as f32),
            None => *v as f32,
        });
    }
    out
}

/// One rate series resampled onto the cell grid, peak sample per cell.
fn peak_cells(values: &VecDeque<f64>, cells: usize, spacing: u64) -> Vec<Option<f64>> {
    let mut out = vec![None; cells];
    if values.is_empty() || cells == 0 {
        return out;
    }
    for (i, v) in values.iter().enumerate() {
        let age = (values.len() - 1 - i) as u64 * spacing;
        if age >= WINDOW_SECS {
            continue;
        }
        let slot = slot_of(WINDOW_SECS - age - 1, WINDOW_SECS, cells);
        out[slot] = Some(match out[slot] {
            Some(current) => current.max(*v),
            None => *v,
        });
    }
    out
}

/// A cell index back on the clock: seconds ago from the right edge.
fn age_of(slot: usize, cells: usize) -> u64 {
    (cells.saturating_sub(1 + slot)) as u64 * WINDOW_SECS / cells.max(1) as u64
}

/// The ruler on the cell grid: the ticks are placed over the columns the cells
/// occupy, so a tick sits under the sample it names instead of over the gutter.
fn axis_line(cells: usize, record: u64) -> String {
    format!("{:LABEL_W$}{}", "", five_ticks(cells, record))
}

/// Five ticks across the window, one per quarter plus wherever the record
/// starts. Placed into a cell grid from the oldest tick forward, so a tick that
/// would run into the previous one is dropped and `now` always keeps its edge.
fn five_ticks(width: usize, _record_secs: u64) -> String {
    // The ruler is the window, always: a plot holding ten seconds of samples
    // still shows where those ten seconds sit inside the last four minutes.
    let window = WINDOW_SECS.max(1);
    let step = window / 4;
    let ages = [0u64, step, step * 2, step * 3, window];
    let mut grid = vec![' '; width];
    // Newest first: `now` owns the right edge, and an older tick that would
    // touch a placed one is the one that goes.
    for age in ages {
        let text = if age == 0 {
            "now".to_string()
        } else {
            format!("-{age}s")
        };
        let len = text.chars().count();
        let pos = (window - age) as f32 / window as f32;
        let target = ((width.saturating_sub(len)) as f32 * pos).round() as usize;
        if target + len > width {
            continue;
        }
        // One blank column between labels, or "-240s-180s" reads as one number.
        let from = target.saturating_sub(1);
        let to = (target + len + 1).min(width);
        if grid[from..to].iter().any(|c| *c != ' ') {
            continue;
        }
        for (i, ch) in text.chars().enumerate() {
            grid[target + i] = ch;
        }
    }
    grid.into_iter().collect::<String>()
}

/// ① RADIO: the serving cell, its three quality meters, RSSI and one 5G line.
fn radio(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut lines: Vec<Line> = Vec::new();
    let sig = app.snap.as_ref().map(|s| &s.signal);
    let dim = Style::default().fg(t.dim);

    if let Some(sig) = sig {
        let band = sig.band.clone().unwrap_or_else(|| "-".into());
        let freq = match (sig.dl_mhz, sig.ul_mhz) {
            (Some(dl), Some(ul)) => format!("{dl}/{ul} MHz"),
            (Some(dl), None) => format!("{dl} MHz"),
            _ => "-".into(),
        };
        let mhz = |v: &str| {
            v.trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .to_string()
        };
        let bw = match (&sig.dl_bw, &sig.ul_bw) {
            (Some(dl), Some(ul)) => format!("{}/{} MHz", mhz(dl), mhz(ul)),
            (Some(dl), None) => format!("{} MHz", mhz(dl)),
            _ => "-".into(),
        };
        let duplex = band_duplex(&band).unwrap_or("-");
        let tm = sig.tm.clone().unwrap_or_else(|| "-".into());
        let tm = if tm.starts_with("TM") {
            tm
        } else {
            format!("TM[{tm}]")
        };
        let net = app
            .snap
            .as_ref()
            .map(|s| s.connection.net_type.clone())
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(
            format!(" {band} {net} · {freq} · {bw} · {duplex} · {tm}"),
            Style::default().fg(t.fg),
        )));
        let earfcn = sig.earfcn.clone().unwrap_or_else(|| "-".into());
        let pci = sig.pci.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
        let tac = sig.tac.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
        lines.push(Line::from(Span::styled(
            format!(" PCI {pci} · EARFCN {earfcn} · TAC {tac}"),
            dim,
        )));
        if inner_rows(area) >= 8 {
            lines.push(Line::from(""));
        }
        for (label, value, unit, domain, step) in [
            (
                "RSRP",
                sig.rsrp_dbm,
                "dBm",
                RSRP_DOMAIN,
                t.rsrp_step(sig.rsrp_dbm.unwrap_or(-200.0)),
            ),
            (
                "RSRQ",
                sig.rsrq_db,
                "dB",
                RSRQ_DOMAIN,
                t.rsrq_step(sig.rsrq_db.unwrap_or(-200.0)),
            ),
            (
                "SINR",
                sig.sinr_db,
                "dB",
                SINR_DOMAIN,
                t.sinr_step(sig.sinr_db.unwrap_or(-200.0)),
            ),
        ] {
            let (text, colour) = match value {
                Some(v) => (format!("{v:>4.0} {unit}"), t.step_color(step)),
                None => ("     -".to_string(), t.dim),
            };
            let meter: String = match value {
                Some(v) => meter_bar(v, domain, 8),
                None => "·".repeat(8),
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {label:<5}"), dim),
                Span::styled(format!(" {text}  "), Style::default().fg(colour)),
                Span::styled(meter, Style::default().fg(colour)),
                Span::styled(
                    format!("  {:<5}", step_word(step)),
                    Style::default().fg(colour),
                ),
                Span::styled(
                    format!(" {:.0}..{:.0}", domain.0, domain.1),
                    Style::default().fg(t.dim),
                ),
            ]));
        }
        let rssi = sig
            .rssi_dbm
            .map(|v| format!("{v:.0} dBm"))
            .unwrap_or_else(|| "-".into());
        let bars = app.snap.as_ref().map(|s| s.connection.nr_bars).unwrap_or(0);
        let endc = app
            .snap
            .as_ref()
            .map(|s| if s.connection.endc { "on" } else { "off" })
            .unwrap_or("off");
        let temp = app
            .snap
            .as_ref()
            .and_then(|s| s.temperature)
            .map(|c| format!(" · {c:.0} °C"))
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(
            format!(" RSSI {rssi} · NR bars {bars} · ENDC {endc}{temp}"),
            dim,
        )));
        lines.push(nr_line(app));
    } else {
        lines.push(Line::from(Span::styled(" waiting for the router", dim)));
    }
    let rows = inner_rows(area);
    lines.truncate(rows);
    f.render_widget(Paragraph::new(lines).block(panel("RADIO".into(), t)), area);
}

/// One line for 5G, or the reason there is none: a panel that only says
/// "not exposed" is not worth a rectangle of the screen.
fn nr_line(app: &App) -> Line<'static> {
    let t = &app.theme;
    match app.snap.as_ref().and_then(|s| s.signal.nr.clone()) {
        Some(nr) => Line::from(Span::styled(
            format!(
                " 5G   n{} · RSRP {} dBm · SINR {} dB",
                nr.band.as_deref().unwrap_or("-"),
                nr.rsrp_dbm
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "-".into()),
                nr.sinr_db
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "-".into()),
            ),
            Style::default().fg(t.fg),
        )),
        None => Line::from(Span::styled(
            " 5G   not exposed by this firmware",
            Style::default().fg(t.dim),
        )),
    }
}

/// ② LINK / SESSION: who we are attached to, and what has moved.
fn link(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let dim = Style::default().fg(t.dim);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(snap) = &app.snap {
        let c = &snap.connection;
        let endc = if c.endc { "ENDC on" } else { "ENDC off" };
        lines.push(Line::from(Span::styled(
            format!(
                " {} ({}) · {} · {} · {endc}",
                snap.plmn.name, snap.plmn.numeric, c.status, c.net_type
            ),
            Style::default().fg(t.fg),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " wan {} · uptime {} · since boot",
                snap.device.wan_ip.as_deref().unwrap_or("-"),
                snap.device
                    .uptime_secs
                    .map(dur)
                    .unwrap_or_else(|| "-".into())
            ),
            Style::default().fg(t.fg),
        )));
        lines.push(Line::from(""));
        let tr = &snap.traffic;
        let cell = |label: &str, dl: String, ul: String, note: String| {
            Line::from(vec![
                Span::styled(format!(" {label:<8}"), dim),
                Span::styled(format!("↓ {dl}"), Style::default().fg(t.fg)),
                Span::raw("        "),
                Span::styled(format!("↑ {ul}"), Style::default().fg(t.fg)),
                Span::styled(format!("     {note}"), dim),
            ])
        };
        lines.push(cell(
            "now",
            bitrate(tr.dl_rate_bytes as f64 * 8.0),
            bitrate(tr.ul_rate_bytes as f64 * 8.0),
            String::new(),
        ));
        let (peak_dl, peak_ul) = peak_rates(app);
        lines.push(cell(
            "peak",
            bitrate(peak_dl),
            bitrate(peak_ul),
            "in window".into(),
        ));
        lines.push(cell(
            "session",
            bytes(tr.session_dl_bytes),
            bytes(tr.session_ul_bytes),
            format!("({})", dur(tr.session_secs)),
        ));
        lines.push(cell(
            "total",
            bytes(tr.total_dl_bytes),
            bytes(tr.total_ul_bytes),
            format!("({})", dur(tr.total_secs)),
        ));
        if let Some(speed) = &app.speed {
            let ago = crate::app::unix_now().saturating_sub(speed.at_unix);
            lines.push(Line::from(Span::styled(
                format!(
                    " test    ↓ {}   ↑ {}   ({ago}s ago)",
                    bitrate(speed.download_bps),
                    bitrate(speed.upload_bps)
                ),
                Style::default().fg(t.accent),
            )));
        }
        let dns = snap
            .dns
            .as_ref()
            .and_then(|d| d.primary.clone())
            .unwrap_or_else(|| "-".into());
        let clients = snap.clients.len();
        lines.push(Line::from(Span::styled(
            format!(" dns {dns} · clients {clients}"),
            dim,
        )));
    } else {
        lines.push(Line::from(Span::styled(" waiting for the router", dim)));
    }
    let rows = inner_rows(area);
    lines.truncate(rows);
    f.render_widget(
        Paragraph::new(lines).block(panel("LINK / SESSION".into(), t)),
        area,
    );
}

/// Peak rate inside the window the plots draw, so "peak" and the strip agree.
fn peak_rates(app: &App) -> (f64, f64) {
    let window = WINDOW_SECS as usize;
    let dl = app
        .history_dl
        .iter()
        .rev()
        .take(window)
        .copied()
        .fold(0.0f64, f64::max);
    let ul = app
        .history_ul
        .iter()
        .rev()
        .take(window)
        .copied()
        .fold(0.0f64, f64::max);
    (dl * 8.0, ul * 8.0)
}

fn meter_bar(value: f32, domain: (f32, f32), cells: usize) -> String {
    let span = (domain.1 - domain.0).max(1.0);
    let filled = (((value - domain.0) / span) * cells as f32).round() as isize;
    let filled = filled.clamp(0, cells as isize) as usize;
    let mut out = String::new();
    for i in 0..cells {
        out.push(if i < filled { '▅' } else { '·' });
    }
    out
}

fn inner_rows(area: Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

/// The statistics field of one map row: the range the cells cover, their
/// average, and the state of the newest reading. It is derived from the cells,
/// so it describes what the row draws and nothing else.
fn stats_field(cells: &[Option<f32>], domain: (f32, f32)) -> Option<String> {
    let vals: Vec<f32> = cells.iter().filter_map(|v| *v).collect();
    if vals.is_empty() {
        return None;
    }
    let min = vals.iter().copied().fold(f32::INFINITY, f32::min);
    let max = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let avg = vals.iter().sum::<f32>() / vals.len() as f32;
    let now = cells.iter().rev().find_map(|v| *v)?;
    let (_, state) = channel(now, domain);
    Some(format!(
        "{min:.0}..{max:.0}  avg {avg:.0}  now {}",
        state_word_of(state)
    ))
}

/// The legend of the map, in the widest form that fits: the three channel words
/// are the point of this panel, so they are the last thing dropped, and a word
/// is never cut in half by the panel edge.
fn map_legend(width: usize, cells: usize, dim: Style) -> Line<'static> {
    let head = format!(" last {WINDOW_SECS}s · {} · ", cell_label(cells));
    let build = |range_words: &'static str,
                 names: [&'static str; 3],
                 tail: &'static str|
     -> Line<'static> {
        let colours = [STATE_GOOD, STATE_FAIR, STATE_POOR];
        let mut spans = vec![
            Span::styled(head.clone(), dim),
            Span::styled("▁▂▃▄▅▆▇█", dim),
        ];
        if !range_words.is_empty() {
            spans.push(Span::styled(range_words, dim));
        }
        for (i, name) in names.iter().enumerate() {
            if name.is_empty() {
                continue;
            }
            spans.push(Span::styled(" · ", dim));
            spans.push(Span::styled(*name, Style::default().fg(colours[i])));
        }
        if !tail.is_empty() {
            spans.push(Span::styled(tail, dim));
        }
        Line::from(spans)
    };
    let variants = [
        build(
            " level in its own range",
            ["green good", "amber fair", "red poor"],
            " · blank = no sample",
        ),
        build(
            " level in its own range",
            ["good", "fair", "poor"],
            " · blank = no sample",
        ),
        build(" level", ["good", "fair", "poor"], " · blank"),
        build("", ["good", "fair", "poor"], ""),
    ];
    let mut last = build("", ["good", "fair", "poor"], "");
    for line in variants {
        if line.width() <= width {
            return line;
        }
        last = line;
    }
    last
}

/// ③ SIGNAL MAP: three metrics over the full width, one window, one clock.
fn signal_map(f: &mut Frame, area: Rect, app: &App, mode: Mode) {
    let t = &app.theme;
    let spacing = sample_secs(app);
    let width = area.width.saturating_sub(2) as usize;
    let cells = grid(width, mode.stats);
    let rows = inner_rows(area);

    let rsrp = worst_cells(&app.history_rsrp, cells, spacing);
    let rsrq = worst_cells(&app.history_rsrq, cells, spacing);
    let sinr = worst_cells(&app.history_sinr, cells, spacing);
    let filled = rsrp.iter().filter(|v| v.is_some()).count();
    let record = (filled as u64 * WINDOW_SECS.div_ceil(cells as u64)).min(WINDOW_SECS);

    let mut lines: Vec<Line> = Vec::new();
    let dim = Style::default().fg(t.dim);
    let legend = if filled == 0 {
        Line::from(Span::styled(
            format!(" waiting for the first sample · {}", cell_label(cells)),
            dim,
        ))
    } else {
        map_legend(width, cells, dim)
    };
    // A narrow panel drops SINR, then RSRQ, before it drops the ruler: a metric
    // with no row is honest, an axis with no row is not.
    let metrics = if rows >= 6 {
        3
    } else if rows >= 5 {
        2
    } else {
        1
    };
    let metric = |label: &'static str,
                  id: Color,
                  domain: (f32, f32),
                  cells: &[Option<f32>]|
     -> Line<'static> {
        // The gutter is the identity channel: it is drawn on every frame, even
        // when the row has no sample and every cell is blank.
        let mut spans = vec![Span::styled(
            format!(" ▍{label}  "),
            Style::default().fg(id),
        )];
        for cell in cells {
            match cell {
                Some(v) => {
                    let (level, state) = channel(*v, domain);
                    spans.push(Span::styled(
                        BARS[level].to_string(),
                        Style::default().fg(state_color(state)),
                    ));
                }
                None => spans.push(Span::raw(" ")),
            }
        }
        if mode.stats {
            let field = stats_field(cells, domain)
                .map(|s| s.chars().take(STATS_W).collect::<String>())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("  {field:<STATS_W$}"),
                Style::default().fg(t.dim),
            ));
        }
        Line::from(spans)
    };
    lines.push(legend);
    if rows >= metrics + 3 {
        lines.push(Line::from(""));
    }
    lines.push(metric("RSRP", ID_RSRP, RSRP_DOMAIN, &rsrp));
    if metrics >= 2 {
        lines.push(metric("RSRQ", ID_RSRQ, RSRQ_DOMAIN, &rsrq));
    }
    if metrics >= 3 {
        lines.push(metric("SINR", ID_SINR, SINR_DOMAIN, &sinr));
    }
    if rows >= metrics + 4 {
        lines.push(Line::from(""));
    }
    if rows >= metrics + 2 {
        lines.push(Line::from(Span::styled(
            axis_line(cells, record),
            Style::default().fg(t.dim),
        )));
    }
    if rows >= metrics + 5 {
        if let Some(insight) = insight_line(&rsrp, &sinr, cells) {
            lines.push(Line::from(Span::styled(
                format!(" {insight}"),
                Style::default().fg(t.fg),
            )));
        }
    }
    lines.truncate(rows);
    f.render_widget(
        Paragraph::new(lines).block(panel(format!("SIGNAL MAP · {WINDOW_SECS}s"), t)),
        area,
    );
}

/// The conclusion the samples actually support: the worst reading, how long the
/// SINR sat under zero, and how many separate poor runs there were. Short of
/// samples there is no line, because a window of three readings supports none.
fn insight_line(rsrp: &[Option<f32>], sinr: &[Option<f32>], cells: usize) -> Option<String> {
    let filled = rsrp.iter().filter(|v| v.is_some()).count();
    if filled < 10 {
        return None;
    }
    let (slot, worst) = rsrp
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (i, v)))
        .fold((0usize, f32::INFINITY), |acc, (i, v)| {
            if v < acc.1 {
                (i, v)
            } else {
                acc
            }
        });
    let age = age_of(slot, cells);
    let below = sinr.iter().filter(|v| v.is_some_and(|v| v < 0.0)).count() as u64 * WINDOW_SECS
        / cells.max(1) as u64;
    let mut dips = 0;
    let mut poor = false;
    for cell in rsrp.iter() {
        let is_poor = cell.is_some_and(|v| v <= -100.0);
        if is_poor && !poor {
            dips += 1;
        }
        poor = is_poor;
    }
    Some(format!(
        "worst {worst:.0} dBm {age}s ago · SINR < 0 for {below}s · {dips} dips"
    ))
}

/// ④ THROUGHPUT: the same window, the same columns, the peak per cell.
fn throughput(f: &mut Frame, area: Rect, app: &App, mode: Mode) {
    let t = &app.theme;
    let spacing = sample_secs(app);
    let width = area.width.saturating_sub(2) as usize;
    // The same grid as the map, statistics column included, so a sample cannot
    // stand one column higher in one plot than in the other.
    let cells = grid(width, mode.stats);
    let down = peak_cells(&app.history_dl, cells, spacing);
    let up = peak_cells(&app.history_ul, cells, spacing);
    let top = down
        .iter()
        .chain(up.iter())
        .filter_map(|v| *v)
        .fold(0.0f64, f64::max);
    let filled = down.iter().filter(|v| v.is_some()).count();
    let record = (filled as u64 * WINDOW_SECS.div_ceil(cells as u64)).min(WINDOW_SECS);

    let mut lines: Vec<Line> = Vec::new();
    let peak_dl = down.iter().filter_map(|v| *v).fold(0.0f64, f64::max);
    let peak_ul = up.iter().filter_map(|v| *v).fold(0.0f64, f64::max);
    let avg = |series: &[Option<f64>]| -> f64 {
        let vals: Vec<f64> = series.iter().filter_map(|v| *v).collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };
    if filled == 0 {
        lines.push(Line::from(Span::styled(
            format!(" waiting for the first sample · {}", cell_label(cells)),
            Style::default().fg(t.dim),
        )));
        lines.truncate(inner_rows(area));
        f.render_widget(
            Paragraph::new(lines).block(panel("THROUGHPUT".into(), t)),
            area,
        );
        return;
    }
    let rows = inner_rows(area);
    if rows >= 4 {
        lines.push(Line::from(Span::styled(
            format!(
                " {} · same axis as the map above · scale 0..{} auto",
                cell_label(cells),
                bitrate(top * 8.0)
            ),
            Style::default().fg(t.dim),
        )));
    }
    if rows >= 6 {
        lines.push(Line::from(""));
    }
    let bar = |label: &str, series: &[Option<f64>], peak: f64, avg: f64| -> Line<'static> {
        let glyphs: String = series
            .iter()
            .map(|cell| match cell {
                Some(v) => level(*v, top),
                None => ' ',
            })
            .collect();
        let note = format!(
            "pk {} · avg {}",
            compact_bps(peak * 8.0),
            compact_bps(avg * 8.0)
        );
        let note = if mode.stats {
            // The same reserved field the map uses, so both plots start their
            // bar on the same column.
            let body: String = note.chars().take(STATS_W).collect();
            format!("  {body:>STATS_W$}")
        } else {
            format!("  {note}")
        };
        Line::from(vec![
            Span::styled(format!(" {label:<7}"), Style::default().fg(t.dim)),
            Span::styled(glyphs, Style::default().fg(t.accent)),
            Span::styled(note, Style::default().fg(t.dim)),
        ])
    };
    lines.push(bar("↓ down", &down, peak_dl, avg(&down)));
    if rows >= 2 {
        lines.push(bar("↑ up  ", &up, peak_ul, avg(&up)));
    }
    if rows >= 3 {
        lines.push(Line::from(Span::styled(
            axis_line(cells, record),
            Style::default().fg(t.dim),
        )));
    }
    let last_traffic = app
        .history_dl
        .iter()
        .rev()
        .position(|v| *v > 0.0)
        .map(|i| (i as u64 * spacing).min(WINDOW_SECS));
    let footer = match last_traffic {
        Some(0) => "traffic now · window follows the map above".to_string(),
        Some(age) => format!("idle now · last traffic {age}s ago · window follows the map above"),
        None => "no traffic in the window".to_string(),
    };
    if rows >= 5 {
        lines.push(Line::from(Span::styled(
            format!(" {footer}"),
            Style::default().fg(t.dim),
        )));
    }
    lines.truncate(inner_rows(area));
    f.render_widget(
        Paragraph::new(lines).block(panel("THROUGHPUT".into(), t)),
        area,
    );
}

/// A rate in the shortest honest form: the statistics column is narrow and a
/// truncated number is worse than a coarser unit.
fn compact_bps(bps: f64) -> String {
    let units = [(1e9, "G"), (1e6, "M"), (1e3, "K")];
    for (scale, unit) in units {
        if bps >= scale {
            let v = bps / scale;
            return if v >= 100.0 {
                format!("{v:.0}{unit}")
            } else {
                format!("{v:.1}{unit}")
            };
        }
    }
    format!("{bps:.0}")
}

/// Eight fill levels, scaled to the window's own peak. Zero draws the floor,
/// because a measured zero is a reading and a blank is the absence of one.
fn level(v: f64, top: f64) -> char {
    const RAMP: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if top <= 0.0 {
        return '▁';
    }
    let idx = ((v / top) * 7.0).round() as usize;
    RAMP[idx.min(7)]
}

/// ⑤ CLIENTS: the summary became the panel title; the table gained nothing the
/// model does not carry.
fn clients(f: &mut Frame, area: Rect, app: &App, wide: bool) {
    let t = &app.theme;
    let rows = app.dash_clients();
    let fives = rows
        .iter()
        .filter(|c| c.freq.as_deref().is_some_and(|f| f.contains('5')))
        .count();
    let twos = rows
        .iter()
        .filter(|c| c.freq.as_deref().is_some_and(|f| f.contains("2.4")))
        .count();
    let title = format!("CLIENTS · {} · 5G ×{fives} · 2.4G ×{twos}", rows.len());
    let inner = inner_rows(area);
    let mut lines: Vec<Line> = Vec::new();
    if inner == 0 {
        f.render_widget(Paragraph::new("").block(panel(title, t)), area);
        return;
    }
    if inner >= 3 {
        // The header names only the columns the rows carry: at the height that
        // drops the durations, a "connected" column would sit over blanks.
        lines.push(Line::from(Span::styled(
            if wide {
                format!(
                    " {} {} {} connected",
                    pad("name", 26),
                    pad("ip", 15),
                    pad("band", 7)
                )
            } else {
                format!(" {} {} {}", pad("name", 26), pad("ip", 15), pad("band", 7))
            },
            Style::default().fg(t.dim),
        )));
    }
    let header = usize::from(inner >= 3);
    // The footer is a nicety; a client row is the data. When the panel is one
    // row too short for both, the footer goes, so the title never claims a
    // count the panel does not show.
    let mut room = inner.saturating_sub(header + 1);
    let mut footer = inner >= 3;
    if rows.len() > room && inner.saturating_sub(header) >= rows.len() {
        room = inner - header;
        footer = false;
    }
    let visible = rows.len().min(room);
    let sel = app.dash_sel.min(rows.len().saturating_sub(1));
    for (i, c) in rows.iter().enumerate().take(visible) {
        let selected = i == sel;
        let style = if selected {
            Style::default()
                .bg(t.selected)
                .fg(t.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} {} {}",
                if selected { "▶" } else { " " },
                pad(c.name.as_deref().unwrap_or("(unnamed)"), 26),
                pad(c.ip.as_deref().unwrap_or("-"), 15),
                if wide {
                    format!(
                        "{} {}",
                        pad(c.freq.as_deref().unwrap_or("-"), 7),
                        dur(c.associated_secs)
                    )
                } else {
                    pad(c.freq.as_deref().unwrap_or("-"), 7)
                }
            ),
            style,
        )));
    }
    if rows.len() > visible && !footer {
        lines.push(Line::from(Span::styled(
            format!(" +{} more", rows.len() - visible),
            Style::default().fg(t.dim),
        )));
    }
    if footer {
        lines.push(Line::from(Span::styled(
            format!(
                " j/k move · enter opens a client · s sort · f filter · {visible} of {} shown",
                rows.len()
            ),
            Style::default().fg(t.dim),
        )));
    }
    lines.truncate(inner);
    f.render_widget(Paragraph::new(lines).block(panel(title, t)), area);
}

/// ⑥ EVENTS / ERRORS: the errors panel folded into one line, the log beside it.
fn events(f: &mut Frame, area: Rect, app: &App, lines_wanted: usize) {
    let t = &app.theme;
    let errors = app.error_log.len();
    let mut warns = 0;
    for e in app.event_log.iter() {
        if matches!(e.level, crate::app::LogLevel::Warn) {
            warns += 1;
        }
    }
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!(
            " errors {errors} · warnings {warns} · events {}",
            app.event_log.len()
        ),
        Style::default().fg(if errors > 0 { t.danger } else { t.dim }),
    ))];
    let room = lines_wanted.saturating_sub(2);
    if room > 0 {
        lines.push(Line::from(""));
    }
    // The newest incidents first: an incident is why this log reads the way it
    // does. A window, a row count and a confidence, never a cause.
    let mut shown = 0;
    for inc in app
        .incidents
        .iter()
        .rev()
        .take(2.min(room.saturating_sub(1)))
    {
        let conf = match inc.confidence {
            crate::incidents::Confidence::Low => "low",
            crate::incidents::Confidence::Medium => "med",
            crate::incidents::Confidence::High => "high",
        };
        lines.push(Line::from(vec![
            Span::styled(" INC ", Style::default().fg(t.warn)),
            Span::styled(
                truncate(
                    // Short by design: the pane is about 39 cells wide, and an
                    // incident line that loses its "no cause" clause has lost the
                    // point of being drawn.
                    &format!("{} {}r {conf} · no cause", inc.id, inc.evidence),
                    area.width.saturating_sub(16) as usize,
                ),
                Style::default().fg(t.warn),
            ),
        ]));
        shown += 1;
    }
    for entry in app.event_log.iter().take(room.saturating_sub(1 + shown)) {
        let colour = match entry.level {
            crate::app::LogLevel::Error => t.danger,
            crate::app::LogLevel::Warn => t.warn,
            crate::app::LogLevel::Info => t.fg,
        };
        let word = match entry.level {
            crate::app::LogLevel::Error => "error",
            crate::app::LogLevel::Warn => "warn ",
            crate::app::LogLevel::Info => "info ",
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {:>5}s ", entry.at), Style::default().fg(t.dim)),
            Span::styled(format!("{word} "), Style::default().fg(colour)),
            Span::styled(
                truncate(&entry.msg, area.width.saturating_sub(16) as usize),
                Style::default().fg(colour),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).block(panel("EVENTS / ERRORS".into(), t)),
        area,
    );
}

/// The frequency and duplex of a band, from the one table the bands tab keeps.
fn band_duplex(band: &str) -> Option<&'static str> {
    let n: u8 = band.trim_start_matches('B').parse().ok()?;
    crate::ui::tabs::band_radio(n).map(|(_, duplex)| duplex)
}

/// The dashboard body: ①② on top, ③ over ④, ⑤ over ⑥.
pub fn dashboard(f: &mut Frame, area: Rect, app: &App) {
    let mode = mode_for(area);
    if area.width < 60 || area.height < 12 {
        narrow(f, area, app, mode);
        return;
    }
    // The two-column tree needs 10 + 7 + 6 + 6 rows; below that it would clip
    // the panels instead of shrinking them, so a short terminal gets the stack.
    if area.width >= 100 && area.height >= 30 {
        let [top, map, thr, bottom] = Layout::vertical([
            Constraint::Length(10),
            Constraint::Length(if mode.stats { 10 } else { 8 }),
            Constraint::Length(if mode.stats { 8 } else { 7 }),
            Constraint::Min(if mode.stats { 6 } else { 5 }),
        ])
        .areas(area);
        let [left, _, right] = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(top);
        radio(f, left, app);
        link(f, right, app);
        signal_map(f, map, app, mode);
        throughput(f, thr, app, mode);
        let [c, _, e] = Layout::horizontal([
            Constraint::Percentage(64),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(bottom);
        clients(f, c, app, mode.stats);
        events(f, e, app, inner_rows(bottom));
    } else {
        narrow(f, area, app, mode);
    }
}

/// One column: the same seven regions stacked, each with what fits.
fn narrow(f: &mut Frame, area: Rect, app: &App, mode: Mode) {
    // One column. Every zone keeps a home: the radio and the link that feeds it,
    // the two plots, then the clients line. A taller terminal gives the map the
    // surplus, because the map is the zone that reads the window.
    if area.height < 16 {
        // Below this the frames would hold nothing: one line for the cell, one
        // for the clients, and no box that cannot show a value.
        let [top, rest] = Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).areas(area);
        radio(f, top, app);
        clients_line(f, rest, app);
        return;
    }
    let map_h = 5 + area.height.saturating_sub(20).min(4);
    let [radio_a, link_a, map_a, thr_a, list_a] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(4),
        Constraint::Length(map_h),
        Constraint::Length(4),
        Constraint::Min(1),
    ])
    .areas(area);
    radio(f, radio_a, app);
    link(f, link_a, app);
    signal_map(f, map_a, app, mode);
    throughput(f, thr_a, app, mode);
    clients_line(f, list_a, app);
}

/// ⑤ and ⑥ at their narrowest: one line each, no border, because a border
/// around a single row costs two rows the terminal does not have.
fn clients_line(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let rows = app.dash_clients();
    let text = match rows.get(app.dash_sel.min(rows.len().saturating_sub(1))) {
        Some(c) if !rows.is_empty() => {
            let more = rows.len().saturating_sub(1);
            format!(
                " ▶ {}  {}  {}  · {more} more · err {} · i log",
                c.name.as_deref().unwrap_or("(unnamed)"),
                c.ip.as_deref().unwrap_or("-"),
                c.freq.as_deref().unwrap_or("-"),
                app.error_log.len()
            )
        }
        _ => format!(" no clients · err {} · i log", app.error_log.len()),
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, Style::default().fg(t.fg)))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        App::new(
            crate::theme::Theme::get("terminal"),
            crate::theme::GraphStyle::Braille,
            crate::app::View::Full,
            0,
            2000,
        )
    }

    #[test]
    fn one_sample_lands_in_the_same_column_above_and_below() {
        let mut app = test_app();
        for i in 0..40u64 {
            app.history_dl.push_back(i as f64 * 1000.0);
            app.history_rsrp.push_back(-80.0 - i as f64);
        }
        let cells = grid(100, true);
        let dl = peak_cells(&app.history_dl, cells, sample_secs(&app));
        let rsrp = worst_cells(&app.history_rsrp, cells, sample_secs(&app));
        let a = dl.iter().position(|v| v.is_some()).unwrap();
        let b = rsrp.iter().position(|v| v.is_some()).unwrap();
        assert_eq!(a, b);
    }
}
