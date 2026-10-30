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

/// Rows the band needs. The dashboard drops it below its own minimum.
pub const HEIGHT: u16 = 7;

#[derive(Clone)]
struct Tile {
    title: &'static str,
    value: String,
    note: String,
    /// 0..1 fill for the small meter under the note.
    share: Option<f64>,
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
        Slot::Metric(tile) => draw_tile(f, rect, &app.theme, tile.clone()),
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
        color: sinr.map(|v| theme.sinr_color(v)).unwrap_or(theme.dim),
    }));

    let clients = app.snap.as_ref().map(|s| s.clients.len()).unwrap_or(0);
    slots.push(Slot::Metric(Tile {
        title: "CLIENTS",
        value: clients.to_string(),
        note: "2.4/5 GHz".to_string(),
        share: Some((clients as f64 / 16.0).clamp(0.0, 1.0)),
        color: theme.accent,
    }));

    if let Some(secs) = app.snap.as_ref().and_then(|s| s.device.uptime_secs) {
        if secs > 0 {
            slots.push(Slot::Metric(Tile {
                title: "UPTIME",
                value: dur(secs),
                note: "since boot".to_string(),
                share: None,
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
            color,
        }));
    }

    slots.push(Slot::Map);
    slots
}

fn draw_tile(f: &mut Frame, area: Rect, theme: &Theme, tile: Tile) {
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
    if let Some(share) = tile.share {
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
    let samples: Vec<f64> = app.history_rsrp.iter().copied().collect();
    let start = samples.len().saturating_sub(cols);
    let mut line = Vec::with_capacity(cols);
    for sample in &samples[start..] {
        line.push(Span::styled(
            "■",
            Style::default().fg(theme.rsrp_color(*sample as f32)),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(line)),
        Rect { height: 1, ..inner },
    );
    if inner.height >= 3 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate("1 ■ = 1 sample", inner.width as usize),
                Style::default().fg(theme.dim),
            ))),
            Rect {
                y: inner.y + 2,
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
    fn the_tiles_say_what_they_measured() {
        let text = frame(&app());
        for word in ["RSRP", "SINR", "CLIENTS", "SIGNAL MAP"] {
            assert!(text.contains(word), "missing tile {word}");
        }
        assert!(text.contains("-95"), "RSRP value missing");
        assert!(text.contains("12.0"), "SINR value missing");
    }

    #[test]
    fn the_map_states_its_unit() {
        assert!(frame(&app()).contains("1 ■ = 1 sample"));
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
