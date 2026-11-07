//! The instrument band above the dashboard: four numbers that are always
//! there, plus two that only exist when the router reports them.
//!
//! A tile is never drawn with a placeholder value: `UPTIME` needs a real
//! uptime, `TEMP` needs a firmware that answers `device/temperature`. The band
//! re-flows to 4, 5 or 6 equal cells instead of leaving a hole.

use crate::app::App;
use crate::theme::Theme;
use crate::ui::tabs::panel;
use crate::ui::widgets::dur;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::VecDeque;

/// Rows the band needs. The dashboard drops it below its own minimum.
pub const HEIGHT: u16 = 7;

#[derive(Clone)]
struct Tile {
    title: &'static str,
    value: String,
    note: String,
    /// 0..1 fill for the small meter under the note.
    share: Option<f64>,
    /// A one-row trend strip drawn in the meter's place, in the metric's own
    /// domain so two readings stay comparable.
    trend: Option<(Vec<u64>, u64)>,
    /// Clients per radio, in drawing order: 5 GHz, 2.4 GHz, unknown band.
    /// One square per client, so a reader counts people, not percent.
    clients: Option<[usize; 3]>,
    color: Color,
}

enum Slot {
    Metric(Tile),
    Map,
}

pub fn instruments(f: &mut Frame, area: Rect, app: &App) {
    let slots = slots(app);
    match slots.len() {
        4 => {
            let [a, b, c, d] = Layout::horizontal([Constraint::Ratio(1, 4); 4]).areas(area);
            for (rect, slot) in [a, b, c, d].iter().zip(slots.iter()) {
                paint(f, *rect, slot, app);
            }
        }
        5 => {
            let [a, b, c, d, e] = Layout::horizontal([Constraint::Ratio(1, 5); 5]).areas(area);
            for (rect, slot) in [a, b, c, d, e].iter().zip(slots.iter()) {
                paint(f, *rect, slot, app);
            }
        }
        _ => {
            let [a, b, c, d, e, g] = Layout::horizontal([Constraint::Ratio(1, 6); 6]).areas(area);
            for (rect, slot) in [a, b, c, d, e, g].iter().zip(slots.iter()) {
                paint(f, *rect, slot, app);
            }
        }
    }
}

fn paint(f: &mut Frame, rect: Rect, slot: &Slot, app: &App) {
    match slot {
        Slot::Metric(tile) => draw_tile(f, rect, &app.theme, tile.clone(), app.chart_opts()),
        Slot::Map => draw_map(f, rect, app),
    }
}

/// Built in display order. The conditional tiles are skipped entirely, which is
/// what keeps the cells equal instead of leaving a gap.
fn slots(app: &App) -> Vec<Slot> {
    let theme = &app.theme;
    let mut slots = Vec::new();

    let rsrp = app.snap.as_ref().and_then(|s| s.signal.rsrp_dbm);
    slots.push(Slot::Metric(Tile {
        title: "RSRP",
        value: rsrp
            .map(|v| format!("{v:.0} dBm"))
            .unwrap_or_else(|| "not reported".into()),
        note: app.signal_quality().to_string(),
        share: rsrp.map(|v| f64::from(crate::headless::rsrp_percent(v)) / 100.0),
        trend: Some((
            app.history_rsrp
                .iter()
                .map(|v| (v + 140.0).clamp(0.0, 96.0) as u64)
                .collect(),
            96,
        )),
        clients: None,
        color: rsrp.map(|v| theme.rsrp_color(v)).unwrap_or(theme.dim),
    }));

    let sinr = app.snap.as_ref().and_then(|s| s.signal.sinr_db);
    slots.push(Slot::Metric(Tile {
        title: "SINR",
        value: sinr
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "not reported".into()),
        note: app
            .snap
            .as_ref()
            .map(|s| {
                format!(
                    "{} · NR {} bars",
                    s.connection.net_type, s.connection.nr_bars
                )
            })
            .unwrap_or_else(|| "no sample".into()),
        share: sinr.map(|v| f64::from(((v + 20.0) / 50.0).clamp(0.0, 1.0))),
        trend: Some((
            app.history_sinr
                .iter()
                .map(|v| (v + 20.0).clamp(0.0, 50.0) as u64)
                .collect(),
            50,
        )),
        clients: None,
        color: sinr.map(|v| theme.sinr_color(v)).unwrap_or(theme.dim),
    }));

    // Beside SINR: the map answers "was the signal good while I was looking at
    // these numbers", so it belongs next to the reading, not at the far edge.
    slots.push(Slot::Map);

    // The router gives each client's radio, so the tile can split them instead
    // of drawing a count against an arbitrary denominator.
    let mut per_band = [0usize; 3];
    if let Some(snap) = &app.snap {
        for client in &snap.clients {
            per_band[band_index(client.freq.as_deref())] += 1;
        }
    }
    let clients = app.snap.as_ref().map(|s| s.clients.len()).unwrap_or(0);
    let mut note = format!("5 ×{} · 2.4 ×{}", per_band[0], per_band[1]);
    if per_band[2] > 0 {
        note.push_str(&format!(" · ? ×{}", per_band[2]));
    }
    slots.push(Slot::Metric(Tile {
        title: "CLIENTS",
        value: clients.to_string(),
        note,
        share: None,
        trend: None,
        clients: Some(per_band),
        color: theme.accent,
    }));

    if let Some(secs) = app.snap.as_ref().and_then(|s| s.device.uptime_secs) {
        if secs > 0 {
            slots.push(Slot::Metric(Tile {
                title: "UPTIME",
                value: dur(secs),
                note: "since boot".to_string(),
                share: None,
                trend: None,
                clients: None,
                color: theme.accent,
            }));
        }
    }

    if let Some(temp) = app.snap.as_ref().and_then(|s| s.temperature) {
        let (note, color) = if temp >= 75.0 {
            ("hot", theme.danger)
        } else if temp >= 60.0 {
            ("warm", theme.warn)
        } else {
            ("normal", theme.ok)
        };
        slots.push(Slot::Metric(Tile {
            title: "TEMP",
            value: format!("{temp:.0} °C"),
            note: note.to_string(),
            share: Some((f64::from(temp) / 100.0).clamp(0.0, 1.0)),
            trend: None,
            clients: None,
            color,
        }));
    }

    slots
}

fn draw_tile(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    tile: Tile,
    opts: crate::ui::chart::ChartOpts,
) {
    let block = panel(tile.title.to_string(), theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [head, note, rest] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            tile.value.clone(),
            Style::default().fg(tile.color).add_modifier(Modifier::BOLD),
        ))),
        head,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate(&tile.note, note.width as usize),
            Style::default().fg(theme.dim),
        ))),
        note,
    );
    if let Some(counts) = tile.clients {
        client_dots(f, rest, counts, theme);
    } else if let Some((trend, span)) = &tile.trend {
        let values: VecDeque<f64> = trend.iter().map(|&v| v as f64).collect();
        let line = crate::ui::chart::spark_lines(
            &values,
            area.width.saturating_sub(2) as usize,
            1,
            false,
            tile.color,
            0,
            *span,
            opts,
        );
        f.render_widget(Paragraph::new(line), rest);
    } else if let Some(share) = tile.share {
        meter(f, rest, share, tile.color, theme);
    }
}

fn truncate(text: &str, room: usize) -> String {
    if text.chars().count() <= room {
        return text.to_string();
    }
    text.chars()
        .take(room.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

/// Which radio a client's frequency belongs to: `0` 5 GHz, `1` 2.4 GHz,
/// `2` unknown.
///
/// The firmware writes `"5GHz"` / `"2.4GHz"` on some builds and a bare
/// `"5180"` / `"2437"` on others, so a numeric value is read as MHz.
fn band_index(freq: Option<&str>) -> usize {
    let Some(freq) = freq else { return 2 };
    let lower = freq.to_ascii_lowercase();
    if lower.starts_with("5") {
        return 0;
    }
    if lower.starts_with("2.4") {
        return 1;
    }
    let mhz: u32 = lower
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    match mhz {
        0 => 2,
        3000.. => 0,
        _ => 1,
    }
}

/// One square per client, coloured by radio. When they do not fit, the strip
/// says how many are missing instead of pretending to be proportional: the
/// whole point of this shape is that a square is a person.
fn client_dots(f: &mut Frame, area: Rect, counts: [usize; 3], theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let room = area.width.saturating_sub(1) as usize;
    let total: usize = counts.iter().sum();
    if total == 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no clients",
                Style::default().fg(theme.dim),
            ))),
            Rect { height: 1, ..area },
        );
        return;
    }
    let (dots, tail) = if total <= room {
        (total, String::new())
    } else {
        let shown = room.saturating_sub(4);
        (shown, format!("+{}", total - shown))
    };
    let colours = [theme.ok, theme.accent, theme.dim];
    let mut spans = Vec::with_capacity(4);
    let mut left = dots;
    for (i, &n) in counts.iter().enumerate() {
        let k = n.min(left);
        if k > 0 {
            spans.push(Span::styled("■".repeat(k), Style::default().fg(colours[i])));
            left -= k;
        }
    }
    if !tail.is_empty() {
        spans.push(Span::styled(tail, Style::default().fg(theme.dim)));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { height: 1, ..area },
    );
}

fn meter(f: &mut Frame, area: Rect, share: f64, color: Color, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let cells = area.width.saturating_sub(1) as usize;
    let filled = (share.clamp(0.0, 1.0) * cells as f64).round() as usize;
    let mut spans = Vec::with_capacity(cells);
    for i in 0..cells {
        spans.push(Span::styled(
            "■",
            Style::default().fg(if i < filled { color } else { theme.border }),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { height: 1, ..area },
    );
}

/// Every sample of RSRP in the history, one square each, oldest on the left.
/// The unit is always stated, because a bare map is a guess.
fn draw_map(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let block = panel("SIGNAL MAP".to_string(), theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let cols = inner.width.saturating_sub(1) as usize;
    let refresh = app.refresh_ms.max(1);
    let per_sample = (refresh / 1000).max(1);
    let secs = (app.history_rsrp.len() as u64 * refresh / 1000).max(1);

    // The same window as the throughput plot, bucketed by the worst reading: a
    // four-second dip is the thing worth seeing, not the best sample inside an
    // eleven-second slot.
    let shifted: Vec<u64> = app
        .history_rsrp
        .iter()
        .map(|v| (v + 140.0).clamp(0.0, 96.0) as u64)
        .collect();
    let slots = crate::ui::chart::resample_min_to_window(&shifted, per_sample, secs, cols);
    let mut line = Vec::with_capacity(slots.len());
    for slot in &slots {
        match slot {
            Some(v) => line.push(Span::styled(
                "■",
                Style::default().fg(theme.rsrp_color(*v as f32 - 140.0)),
            )),
            None => line.push(Span::raw(" ")),
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(line)),
        Rect { height: 1, ..inner },
    );

    // Two facts the squares cannot carry themselves: how much time one of them
    // covers, and what the three colours mean.
    let per_cell = (secs as f64 / cols as f64).max(1.0);
    let caption = if app.history_rsrp.is_empty() {
        // The strip is empty, so there is no span to name yet.
        truncate("no samples yet", inner.width as usize)
    } else {
        truncate(
            &format!("last {secs}s · 1■ = {per_cell:.0}s"),
            inner.width as usize,
        )
    };
    if inner.height >= 4 {
        let legend = Line::from(vec![
            Span::styled("■", Style::default().fg(theme.rsrp_color(-70.0))),
            Span::styled("good ", Style::default().fg(theme.dim)),
            Span::styled("■", Style::default().fg(theme.rsrp_color(-95.0))),
            Span::styled("fair ", Style::default().fg(theme.dim)),
            Span::styled("■", Style::default().fg(theme.rsrp_color(-110.0))),
            Span::styled("poor", Style::default().fg(theme.dim)),
        ]);
        f.render_widget(
            Paragraph::new(legend),
            Rect {
                y: inner.y + 1,
                height: 1,
                ..inner
            },
        );
    }
    if inner.height >= 3 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                caption,
                Style::default().fg(theme.dim),
            ))),
            Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{App, Tab, View};
    use crate::model::Snapshot;
    use crate::theme::{by_name, GraphStyle};

    fn app() -> App {
        let mut app = App::new(by_name("dark"), GraphStyle::Dots, View::Full, 100, 2000);
        let mut snap = Snapshot::default();
        snap.signal.rsrp_dbm = Some(-95.0);
        snap.signal.sinr_db = Some(12.0);
        snap.connection.net_type = "LTE".into();
        app.snap = Some(snap);
        app.history_rsrp = (-100..-80).map(f64::from).collect();
        app.select_tab(Tab::Dashboard);
        app
    }

    fn frame(app: &App) -> String {
        crate::ui::render_to_string(app, 130, 40)
    }

    #[test]
    fn the_signal_tiles_draw_a_trend_strip() {
        let mut app = app();
        app.history_sinr = (0..20).map(|i| 5.0 + i as f64 * 0.5).collect();
        let text = frame(&app);
        let band: Vec<&str> = text.lines().skip(2).take(7).collect();
        let braille = band
            .iter()
            .flat_map(|l| l.chars())
            .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            .count();
        assert!(braille >= 40, "RSRP and SINR each draw a strip: {braille}");
        assert!(
            band.iter().any(|l| l.contains('■')),
            "CLIENTS keeps its meter"
        );
    }

    #[test]
    fn the_map_waits_for_its_first_sample() {
        let mut app = app();
        app.history_rsrp.clear();
        let text = frame(&app);
        assert!(text.contains("no samples yet"), "empty map must say so");
    }

    #[test]
    fn the_clients_tile_draws_one_dot_per_client() {
        let mut app = app();
        let client = |freq: &str| crate::model::Client {
            freq: Some(freq.to_string()),
            ..Default::default()
        };
        if let Some(snap) = app.snap.as_mut() {
            snap.clients = vec![client("5GHz"), client("5GHz"), client("2.4GHz")];
        }
        let text = frame(&app);
        assert!(text.contains("5 ×2 · 2.4 ×1"), "band split missing: {text}");
        assert!(text.contains('■'), "one square per client");
    }

    #[test]
    fn a_full_band_says_how_many_clients_are_not_shown() {
        let mut app = app();
        let clients = vec![crate::model::Client::default(); 40];
        if let Some(snap) = app.snap.as_mut() {
            snap.clients = clients;
        }
        let text = frame(&app);
        assert!(text.contains("40"), "the count is still stated");
        assert!(
            text.contains('+'),
            "overflow is stated, not hidden:
{text}"
        );
    }

    #[test]
    fn the_map_sits_beside_sinr() {
        let text = frame(&app());
        let band = text.lines().nth(2).unwrap_or_default();
        let sinr = band.find("SINR").expect("SINR tile");
        let map = band.find("SIGNAL MAP").expect("map tile");
        let clients = band.find("CLIENTS").expect("clients tile");
        assert!(
            sinr < map && map < clients,
            "band order must be RSRP | SINR | SIGNAL MAP | CLIENTS: {band}"
        );
    }

    #[test]
    fn the_tiles_say_what_they_measured() {
        let text = frame(&app());
        for word in ["RSRP", "SINR", "CLIENTS", "SIGNAL MAP"] {
            assert!(text.contains(word), "missing tile {word}");
        }
        assert!(text.contains("-95"), "RSRP value missing");
        assert!(text.contains("12.0"), "SINR value missing");
    }

    #[test]
    fn the_map_states_its_unit_and_its_span() {
        // 20 samples of 2 s: the strip covers the dashboard's own window and
        // names it, instead of silently showing the newest `cols` samples.
        let text = frame(&app());
        assert!(text.contains("1■"), "the unit is stated: {text}");
        assert!(text.contains("last 40s"), "the span is named");
    }

    #[test]
    fn uptime_appears_only_when_the_router_reports_it() {
        let mut app = app();
        assert!(!frame(&app).contains("UPTIME"), "uptime invented");

        if let Some(snap) = app.snap.as_mut() {
            snap.device.uptime_secs = Some(0);
        }
        assert!(!frame(&app).contains("UPTIME"), "zero uptime is not a fact");

        if let Some(snap) = app.snap.as_mut() {
            snap.device.uptime_secs = Some(3 * 86_400);
        }
        assert!(frame(&app).contains("UPTIME"));
    }

    #[test]
    fn temperature_appears_only_when_reported_and_carries_its_unit() {
        let mut app = app();
        assert!(!frame(&app).contains("TEMP"), "temperature invented");

        if let Some(snap) = app.snap.as_mut() {
            snap.temperature = Some(41.0);
        }
        let text = frame(&app);
        assert!(text.contains("TEMP"), "temperature tile missing");
        assert!(text.contains("41 °C"), "unit missing: {text}");
    }

    #[test]
    fn a_missing_conditional_tile_leaves_no_hole() {
        // Five tiles (no sensor) must still fill the width: CLIENTS is the last
        // permanent metric and must be inside the band, not squeezed out.
        let mut app = app();
        if let Some(snap) = app.snap.as_mut() {
            snap.device.uptime_secs = Some(60);
        }
        let text = frame(&app);
        assert!(text.contains("UPTIME") && !text.contains("TEMP"));
        assert!(text.contains("CLIENTS"));
    }
}
